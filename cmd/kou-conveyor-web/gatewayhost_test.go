//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func gatewayFixtureOptions(t *testing.T) options {
	t.Helper()
	dir := t.TempDir()
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == "/v1/models" {
			io.WriteString(w, `{"data":[{"id":"fake-model"}]}`)
			return
		}
		if r.URL.Path != "/v1/chat/completions" {
			http.NotFound(w, r)
			return
		}
		io.WriteString(w, `{"id":"c1","object":"chat.completion","created":1,"model":"fake-model","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}`)
	}))
	t.Cleanup(upstream.Close)
	accountsDir := filepath.Join(dir, strings.Repeat("long-config-", 12), "accounts")
	if err := os.MkdirAll(accountsDir, 0o700); err != nil {
		t.Fatal(err)
	}
	config := fmt.Sprintf("openai-compatibility:\n  - name: fakeai\n    base-url: %q\n    api-key-entries:\n      - api-key: sk-fixture\n    models:\n      - name: fake-model\n        alias: fake-model\n", upstream.URL+"/v1")
	if err := os.WriteFile(filepath.Join(accountsDir, "config.yaml"), []byte(config), 0o600); err != nil {
		t.Fatal(err)
	}
	settingsFile := filepath.Join(dir, "settings.json")
	if err := cockpit.SaveSettings(settingsFile, cockpit.Settings{API: cockpit.APIGateway, Model: "fake-model"}); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveAutoTitles(cockpit.PreferencesPath(settingsFile), false); err != nil {
		t.Fatal(err)
	}
	return options{Options: cockpit.Options{Workspace: dir, SessionDir: filepath.Join(dir, "sessions"), SettingsFile: settingsFile}, accounts: shutdownAddress(t), accountsDir: accountsDir, detach: true}
}

func startFixturePeer(t *testing.T, o options) *gatewayPeer {
	t.Helper()
	p, err := startGatewayPeer(t.Context(), o)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { p.lease.Close() })
	return p
}

func waitGatewayGone(t *testing.T, p *gatewayPeer) {
	t.Helper()
	d := p.description
	waitFor(t, func() bool {
		if _, err := os.Stat(gatewayHostPath(d.Dir)); !os.IsNotExist(err) {
			return false
		}
		if _, err := os.Stat(d.Gateway.LeaseSocket); !os.IsNotExist(err) {
			return false
		}
		lock, err := gatewayHostLock(d.Dir, "host-owner", true)
		if err != nil {
			return false
		}
		lock.Close()
		return true
	})
}

func fixturePeerRequest(t *testing.T, p *gatewayPeer, method, path string, body, into any) {
	t.Helper()
	if err := p.request(t.Context(), method, path, body, into); err != nil {
		t.Fatal(err)
	}
}

func TestGatewayHostSharesStateAndDrainsItsLastLease(t *testing.T) {
	o := gatewayFixtureOptions(t)
	first := startFixturePeer(t, o)
	second := startFixturePeer(t, o)
	if first.description.Gateway != second.description.Gateway {
		t.Fatal("a second front-end replaced the live gateway")
	}
	for _, path := range []string{gatewayHostPath(o.accountsDir), first.description.Gateway.LeaseSocket, cockpit.GatewayPath(o.SettingsFile)} {
		info, err := os.Stat(path)
		if err != nil || info.Mode().Perm() != 0o600 {
			t.Fatalf("private mode for %s: %v %v", path, info, err)
		}
	}
	if info, err := os.Stat(filepath.Dir(first.description.Gateway.LeaseSocket)); err != nil || info.Mode().Perm() != 0o700 {
		t.Fatalf("private socket directory: %v %v", info, err)
	}
	if len(first.description.Gateway.LeaseSocket) >= 104 {
		t.Fatal("the helper socket does not fit Darwin's sockaddr_un")
	}
	fixturePeerRequest(t, first, "POST", "/api/context-windows", map[string]any{"model": "fake-model", "tokens": 64000}, nil)
	fixturePeerRequest(t, second, "POST", "/api/cache-ttls", map[string]string{"model": "fake-model", "ttl": "1h"}, nil)
	first.lease.Close()
	front := &server{ctx: t.Context(), gatewayPeer: second}
	env, _, err := front.gatewayRunEnv(t.Context(), "FAKE-MODEL")
	if err != nil || !slices.Equal(env, []string{runconfig.ContextWindowEnvironment + "=64000", runconfig.CacheTTLEnvironment + "=1h"}) {
		t.Fatalf("shared model settings: %v %v", env, err)
	}
	models, err := front.gatewayModels(t.Context())
	if err != nil || len(models) != 1 || models[0].ID != "fake-model" || models[0].Context != 64000 {
		t.Fatalf("the host's model registry: %+v %v", models, err)
	}
	var overview map[string]any
	fixturePeerRequest(t, second, "GET", "/api/accounts", nil, &overview)
	if overview["connection"] != nil || overview["gateway"] != nil || overview["models"] == nil {
		t.Fatalf("accounts-only overview leaked a front-end envelope or lost models: %v", overview)
	}
	g := second.description.Gateway
	lease, err := g.Lease(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	defer lease.Close()
	second.lease.Close()
	published, err := cockpit.LoadGateway(cockpit.GatewayPath(o.SettingsFile))
	if err != nil || published != g {
		t.Fatalf("a model client lost the publication: %+v %v", published, err)
	}
	req, _ := http.NewRequest("POST", g.URL+"/v1/chat/completions", strings.NewReader(`{"model":"fake-model","messages":[{"role":"user","content":"hello"}]}`))
	req.Header.Set("Authorization", "Bearer "+g.APIKey)
	req.Header.Set("Content-Type", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	io.Copy(io.Discard, res.Body)
	res.Body.Close()
	if res.StatusCode != http.StatusOK {
		t.Fatalf("model transport without either front-end: %s", res.Status)
	}
	lease.Close()
	waitGatewayGone(t, second)
	if _, err := cockpit.LoadGateway(cockpit.GatewayPath(o.SettingsFile)); !errors.Is(err, cockpit.ErrGatewayOff) {
		t.Fatalf("the last lease did not withdraw its publication: %v", err)
	}
}

func TestGatewayHostPrivateAuthentication(t *testing.T) {
	o := gatewayFixtureOptions(t)
	p := startFixturePeer(t, o)
	for _, test := range []struct{ token, owner, path string }{
		{"", "", "/internal/status"},
		{p.description.Gateway.APIKey, p.description.Gateway.Owner, "/internal/status"},
		{p.description.Token, "other-generation", "/internal/status"},
		{p.description.Gateway.APIKey, "other-generation", "/internal/lease"},
	} {
		req, _ := http.NewRequest("GET", "http://accounts"+test.path, nil)
		req.Header.Set("Authorization", "Bearer "+test.token)
		req.Header.Set("X-Kou-Gateway-Owner", test.owner)
		res, err := p.client.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		if res.StatusCode != http.StatusUnauthorized {
			t.Fatalf("%s accepted the wrong identity: %s", test.path, res.Status)
		}
	}
	p.lease.Close()
	waitGatewayGone(t, p)
}

func TestGatewayHostAttachRacesLastLease(t *testing.T) {
	o := gatewayFixtureOptions(t)
	for i := 0; i < 8; i++ {
		first := startFixturePeer(t, o)
		var wg sync.WaitGroup
		wg.Go(func() { first.lease.Close() })
		second := startFixturePeer(t, o)
		wg.Wait()
		var status accounts.Status
		fixturePeerRequest(t, second, "GET", "/internal/status", nil, &status)
		if status.State != accounts.GatewayRunning {
			t.Fatalf("attach found a drained gateway: %+v", status)
		}
		second.lease.Close()
		waitGatewayGone(t, second)
	}
}

func TestGatewayHostSeparatesDirectoriesAndRejectsAddressChanges(t *testing.T) {
	a, b := gatewayFixtureOptions(t), gatewayFixtureOptions(t)
	first, second := startFixturePeer(t, a), startFixturePeer(t, b)
	if first.description.Gateway.Owner == second.description.Gateway.Owner || first.description.Gateway.URL == second.description.Gateway.URL {
		t.Fatal("different accounts directories share an owner")
	}
	a.accounts = shutdownAddress(t)
	if p, err := startGatewayPeer(t.Context(), a); err == nil {
		p.lease.Close()
		t.Fatal("a new address silently replaced a live managed gateway")
	}
	first.lease.Close()
	waitGatewayGone(t, first)
	fixturePeerRequest(t, second, "GET", "/internal/status", nil, nil)
	second.lease.Close()
	waitGatewayGone(t, second)
}

func TestCommonStartLeasesManagedDirectConnectionAndReleasesOnFailure(t *testing.T) {
	o := gatewayFixtureOptions(t)
	p := startFixturePeer(t, o)
	o.Runner = cockpittest.Runner(t)
	if err := cockpit.SaveSettings(o.SettingsFile, cockpit.Settings{API: cockpit.APIResponses, BaseURL: p.description.Gateway.URL + "/v1", APIKey: p.description.Gateway.APIKey, Model: "fake-model"}); err != nil {
		t.Fatal(err)
	}
	bad := o.Options
	bad.Workspace = filepath.Join(o.Workspace, "missing")
	if job, err := cockpit.Start(t.Context(), bad, cockpit.Request{SessionID: "start-failure", MessageID: uuid.New().String(), Prompt: "hello"}); err == nil {
		job.Stop("the test ended")
		t.Fatal("a runner in a missing working directory started")
	}
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	job, err := cockpit.Start(ctx, o.Options, cockpit.Request{SessionID: "direct-client", MessageID: uuid.New().String(), Prompt: "gate request-model"})
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		for range job.Lines() {
		}
	}()
	waitFor(t, func() bool {
		tr, err := cockpit.LoadSession(o.SessionDir, "direct-client")
		return err == nil && len(tr.Entries) != 0
	})
	p.lease.Close()
	if err := os.WriteFile(filepath.Join(o.Workspace, "gate"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	<-job.Done()
	if job.Err() != nil {
		t.Fatalf("an attached managed direct client lost its gateway: %v", job.Err())
	}
	waitGatewayGone(t, p)
}
