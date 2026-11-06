package main

import (
	"encoding/json/v2"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

type servedWindowTransport func(*http.Request) (*http.Response, error)

func (f servedWindowTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestGatewayRunEnvironmentScopesServedCapacity(t *testing.T) {
	base := []string{runconfig.ContextWindowEnvironment + "=64000", runconfig.CacheTTLEnvironment + "=1h"}
	for _, test := range []struct {
		name, provider string
		api            cockpit.API
		window         int64
		emptyOwner     bool
		wantHint       bool
	}{
		{name: "hosted gateway", api: cockpit.APIGateway, window: 1_000_000, wantHint: true},
		{name: "direct endpoint", api: cockpit.APIResponses, window: 1_000_000},
		{name: "explicit provider flag", api: cockpit.APIGateway, provider: "openai", window: 1_000_000},
		{name: "old helper without capacity", api: cockpit.APIGateway},
		{name: "negative capacity", api: cockpit.APIGateway, window: -1},
		{name: "ownerless capacity", api: cockpit.APIGateway, window: 1_000_000, emptyOwner: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			settingsFile := filepath.Join(t.TempDir(), "settings.json")
			settings := cockpit.Settings{API: test.api, Model: "served-only-alias"}
			if settings.API == cockpit.APIResponses {
				settings.BaseURL, settings.APIKey = "http://direct.invalid/v1", "sk-fixture"
			}
			if err := cockpit.SaveSettings(settingsFile, settings); err != nil {
				t.Fatal(err)
			}
			peerOwner := "fixture-generation"
			if test.emptyOwner {
				peerOwner = ""
			}
			calls := 0
			peer := &gatewayPeer{description: gatewayHostDescription{Token: "fixture-token", Gateway: cockpit.Gateway{Owner: peerOwner}}}
			peer.client = &http.Client{Transport: servedWindowTransport(func(r *http.Request) (*http.Response, error) {
				calls++
				if r.Method != "POST" || r.URL.Path != "/internal/run-environment" || r.Header.Get("Authorization") != "Bearer fixture-token" || r.Header.Get("X-Kou-Gateway-Owner") != peerOwner {
					t.Fatalf("unexpected metadata transport: %s %s", r.Method, r.URL.Path)
				}
				var request struct {
					Model string `json:"model"`
				}
				if err := json.UnmarshalRead(r.Body, &request); err != nil || request.Model != "served-only-alias" {
					t.Fatalf("wrong model lookup: %+v %v", request, err)
				}
				data, err := json.Marshal(gatewayRunEnvironment{Env: base, ServedWindow: test.window})
				if err != nil {
					t.Fatal(err)
				}
				return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(string(data)))}, nil
			})}
			s := &server{gatewayPeer: peer}
			s.opt.SettingsFile, s.opt.Provider = settingsFile, test.provider
			env, owner, err := s.gatewayRunEnv(t.Context(), "served-only-alias")
			want, wantOwner := slices.Clone(base), ""
			if test.wantHint {
				want = append(want, runconfig.ServedContextWindowEnvironment+"=1000000")
				wantOwner = peerOwner
			}
			if err != nil || !slices.Equal(env, want) || owner != wantOwner || calls != 1 {
				t.Fatalf("env=%v owner=%q calls=%d err=%v; want %v/%q and the existing single RPC", env, owner, calls, err, want, wantOwner)
			}
		})
	}
}

func TestGatewayRunEnvironmentDoesNotCacheModelsOrGenerations(t *testing.T) {
	settingsFile := filepath.Join(t.TempDir(), "settings.json")
	if err := cockpit.SaveSettings(settingsFile, cockpit.Settings{API: cockpit.APIGateway}); err != nil {
		t.Fatal(err)
	}
	newPeer := func(owner string, windows map[string]int64) *gatewayPeer {
		return &gatewayPeer{description: gatewayHostDescription{Gateway: cockpit.Gateway{Owner: owner}}, client: &http.Client{Transport: servedWindowTransport(func(r *http.Request) (*http.Response, error) {
			var request struct {
				Model string `json:"model"`
			}
			if err := json.UnmarshalRead(r.Body, &request); err != nil {
				t.Fatal(err)
			}
			data, err := json.Marshal(gatewayRunEnvironment{ServedWindow: windows[request.Model]})
			if err != nil {
				t.Fatal(err)
			}
			return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(string(data)))}, nil
		})}}
	}
	s := &server{gatewayPeer: newPeer("generation-A", map[string]int64{"alias": 1_000_000, "other": 32_000})}
	s.opt.SettingsFile = settingsFile
	for _, test := range []struct{ model, want string }{
		{"alias", "1000000"}, {"other", "32000"}, {"unknown", ""},
	} {
		env, owner, err := s.gatewayRunEnv(t.Context(), test.model)
		var want []string
		wantOwner := ""
		if test.want != "" {
			want = []string{runconfig.ServedContextWindowEnvironment + "=" + test.want}
			wantOwner = "generation-A"
		}
		if err != nil || !slices.Equal(env, want) || owner != wantOwner {
			t.Fatalf("model %s: %v owner=%q %v; want %v/%q", test.model, env, owner, err, want, wantOwner)
		}
	}
	s.gatewayPeer = newPeer("generation-B", map[string]int64{"alias": 64_000})
	env, owner, err := s.gatewayRunEnv(t.Context(), "alias")
	if err != nil || !slices.Equal(env, []string{runconfig.ServedContextWindowEnvironment + "=64000"}) || owner != "generation-B" {
		t.Fatalf("new generation inherited the former capacity: %v owner=%q %v", env, owner, err)
	}
}

func TestGatewayRunEnvironmentNeverHintsFromReadOnlyBooks(t *testing.T) {
	for _, failure := range []string{"offline", "malformed", "unauthorized"} {
		t.Run(failure, func(t *testing.T) {
			dir := t.TempDir()
			settingsFile := filepath.Join(dir, "settings.json")
			if err := cockpit.SaveSettings(settingsFile, cockpit.Settings{API: cockpit.APIGateway}); err != nil {
				t.Fatal(err)
			}
			path := filepath.Join(dir, "context-windows.json")
			book := accounts.NewContextBook(path)
			if _, err := book.Save(accounts.ContextWindow{Model: "alias", Tokens: 64_000}); err != nil {
				t.Fatal(err)
			}
			before, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			peer := &gatewayPeer{description: gatewayHostDescription{Dir: dir, Gateway: cockpit.Gateway{Owner: "offline-generation"}}}
			peer.client = &http.Client{Transport: servedWindowTransport(func(r *http.Request) (*http.Response, error) {
				if failure == "offline" {
					return nil, errors.New("helper is offline")
				}
				status := http.StatusOK
				body := `{"served_window":1000000,"env":[`
				if failure == "unauthorized" {
					status, body = http.StatusUnauthorized, `{"served_window":1000000}`
				}
				return &http.Response{StatusCode: status, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(body))}, nil
			})}
			s := &server{gatewayPeer: peer}
			s.opt.SettingsFile = settingsFile
			env, owner, err := s.gatewayRunEnv(t.Context(), "alias")
			if err != nil || owner != "" || !slices.Equal(env, []string{runconfig.ContextWindowEnvironment + "=64000"}) {
				t.Fatalf("untrusted metadata survived the fallback: %v owner=%q %v", env, owner, err)
			}
			after, err := os.ReadFile(path)
			if err != nil || string(before) != string(after) {
				t.Fatalf("read-only fallback rewrote its book: %v", err)
			}
		})
	}
}
