package accounts

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	sdkconfig "github.com/router-for-me/CLIProxyAPI/v7/sdk/config"
)

// upstream is an OpenAI-compatible provider the gateway is configured with,
// failing while fail is set.
type upstream struct {
	*httptest.Server
	fail atomic.Bool
}

func newUpstream(t *testing.T) *upstream {
	u := &upstream{}
	u.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		if strings.HasSuffix(r.URL.Path, "/models") {
			if !strings.HasPrefix(r.Header.Get("Authorization"), "Bearer sk-upstream") {
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			_, _ = io.WriteString(w, `{"object":"list","data":[{"id":"fake-model"},{"id":"other-model"}]}`)
			return
		}
		if u.fail.Load() {
			w.WriteHeader(http.StatusServiceUnavailable)
			_, _ = io.WriteString(w, `{"error":{"message":"the model is overloaded","type":"overloaded_error"}}`)
			return
		}
		_, _ = io.WriteString(w, `{"id":"c1","object":"chat.completion","created":1,"model":"fake-model",
			"choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
			"usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}`)
	}))
	t.Cleanup(u.Close)
	return u
}

func freeAddress(t *testing.T) string {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	return listener.Addr().String()
}

// TestHost runs the gateway as the cockpit does. The gateway keeps
// process-wide state, so this is the package's only test that starts one.
func TestHost(t *testing.T) {
	// CLIProxyAPI replaces its configuration while it serves requests,
	// without a lock (its server reads the pointer as the watcher swaps it),
	// which the race detector reports at random. make test runs this test
	// again without the detector.
	if raceDetector {
		t.Skip("CLIProxyAPI swaps its configuration without a lock; run without -race")
	}
	dir := t.TempDir()
	up := newUpstream(t)
	config := fmt.Sprintf(`openai-compatibility:
  - name: "fakeai"
    base-url: %q
    api-key-entries:
      - api-key: "sk-upstream"
    models:
      - name: "fake-model"
        alias: "fake-model"
`, up.URL+"/v1")
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(config), 0o600); err != nil {
		t.Fatal(err)
	}
	address := freeAddress(t)
	h, err := New(Options{Dir: dir, Address: address})
	if err != nil {
		t.Fatal(err)
	}
	h.callbacks = false
	if _, err := h.Accounts(t.Context(), "24h"); !errors.Is(err, ErrNotRunning) {
		t.Errorf("before it runs: %v", err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	go h.Run(ctx)
	t.Cleanup(func() {
		cancel()
		wait, stop := context.WithTimeout(context.Background(), 20*time.Second)
		defer stop()
		if !h.Wait(wait) {
			t.Error("the gateway did not stop")
		}
		if s := h.Status(); s.State != GatewayStopped {
			t.Errorf("after it stopped: %+v", s)
		}
	})
	deadline := time.Now().Add(20 * time.Second)
	for h.Status().State == GatewayStarting && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	status := h.Status()
	if status.State != GatewayRunning {
		t.Fatalf("status = %+v", status)
	}
	if status.URL != "http://"+address || status.KeyHint == "" || status.Version == "" && testing.Verbose() {
		t.Logf("status = %+v", status)
	}
	if !h.Serves("http://localhost:"+strings.Split(address, ":")[1]+"/v1") || h.Serves("http://127.0.0.1:1/v1") {
		t.Error("Serves does not recognize the gateway")
	}

	t.Run("keys of the configuration are not listed", func(t *testing.T) {
		// The gateway lists accounts with a credential file; keys in its
		// configuration are managed there.
		list, err := h.Accounts(t.Context(), "24h")
		if err != nil || len(list) != 0 {
			t.Fatalf("accounts = %+v, %v", list, err)
		}
	})

	t.Run("requests make history", func(t *testing.T) {
		send := func() int {
			req, _ := http.NewRequestWithContext(t.Context(), http.MethodPost, h.URL()+"/v1/chat/completions",
				strings.NewReader(`{"model":"fake-model","messages":[{"role":"user","content":"hello"}]}`))
			req.Header.Set("Authorization", "Bearer "+h.APIKey())
			req.Header.Set("Content-Type", "application/json")
			res, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			_, _ = io.Copy(io.Discard, res.Body)
			res.Body.Close()
			return res.StatusCode
		}
		if code := send(); code != http.StatusOK {
			t.Fatalf("request = %d", code)
		}
		// A model that just failed cools down, and the list leaves it out.
		models, err := h.Models(t.Context())
		if err != nil || len(models) == 0 || models[0].ID != "fake-model" {
			t.Errorf("models = %+v, %v", models, err)
		}
		up.fail.Store(true)
		if code := send(); code == http.StatusOK {
			t.Fatalf("a failing upstream answered %d", code)
		}
		up.fail.Store(false)
		// A model that failed may cool down; it stays listed all the same.
		if models, err := h.Models(t.Context()); err != nil || !slices.ContainsFunc(models, func(m Model) bool { return m.ID == "fake-model" }) {
			t.Errorf("after a failure, models = %+v, %v", models, err)
		}
		// The gateway reports requests to its plugins asynchronously.
		var id string
		var uptime Uptime
		for deadline := time.Now().Add(10 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
			h.history.mu.Lock()
			for key := range h.history.accounts {
				id = key
			}
			h.history.mu.Unlock()
			if uptime = h.history.Uptime(id, "", 24*time.Hour, 48); uptime.OK >= 1 && uptime.Failed >= 1 {
				break
			}
		}
		if uptime.OK != 1 || uptime.Failed < 1 || uptime.Input != 11 || uptime.Output != 3 {
			t.Fatalf("uptime of %q = %+v", id, uptime)
		}
		errs := h.history.Errors(id, "", 5)
		if len(errs) == 0 || errs[0].Status != http.StatusServiceUnavailable || !strings.Contains(errs[0].Message, "the model is overloaded") {
			t.Errorf("errors = %+v", errs)
		}
		// The configuration's endpoint shows with its requests.
		o, err := h.Overview(t.Context(), "24h")
		if err != nil || len(o.Accounts) != 0 || len(o.Endpoints) != 1 {
			t.Fatalf("overview = %+v, %v", o, err)
		}
		e := o.Endpoints[0]
		if e.Kind != "openai-compatible" || e.Name != "fakeai" || e.KeyHint != "set" ||
			e.Uptime.OK != 1 || e.Uptime.Failed < 1 || len(e.Errors) == 0 || e.State != StateError || len(e.Models) != 1 {
			t.Errorf("endpoint = %+v", e)
		}
		if s := o.Summary; s.Accounts != 0 || s.Endpoints != 1 || s.FailingEndpoints != 1 || s.OK != 1 || s.Failed < 1 {
			t.Errorf("summary = %+v", s)
		}
	})

	t.Run("endpoints", func(t *testing.T) {
		key := "sk-upstream-key-0001"
		models, err := h.ProbeEndpoint(t.Context(), "", EndpointInput{Kind: "openai-compatible", BaseURL: up.URL + "/v1", APIKey: &key})
		if err != nil || strings.Join(models, ",") != "fake-model,other-model" {
			t.Fatalf("probe = %v, %v", models, err)
		}
		wrong := "sk-wrong"
		if _, err := h.ProbeEndpoint(t.Context(), "", EndpointInput{Kind: "openai-compatible", BaseURL: up.URL + "/v1", APIKey: &wrong}); err == nil || !strings.Contains(err.Error(), "rejected") {
			t.Errorf("a wrong key: %v", err)
		}
		for _, bad := range []EndpointInput{
			{Kind: "openai-compatible", Name: "bad name", BaseURL: up.URL, APIKey: &key, Models: []EndpointModel{{Name: "m"}}},
			{Kind: "openai-compatible", Name: "nomodels", BaseURL: up.URL, APIKey: &key},
			{Kind: "openai-compatible", Name: "fakeai", BaseURL: up.URL, APIKey: &key, Models: []EndpointModel{{Name: "m"}}},
			{Kind: "openai-compatible", Name: "ftp", BaseURL: "ftp://example.com", APIKey: &key, Models: []EndpointModel{{Name: "m"}}},
			{Kind: "anthropic"},
			{Kind: "carrier-pigeon", APIKey: &key},
		} {
			if _, err := h.AddEndpoint(t.Context(), bad); err == nil {
				t.Errorf("%+v was accepted", bad)
			}
		}
		added, err := h.AddEndpoint(t.Context(), EndpointInput{Kind: "openai-compatible", Name: "second", BaseURL: up.URL + "/v1/", APIKey: &key,
			Models: []EndpointModel{{Name: "other-model", Alias: "second-model"}}})
		if err != nil {
			t.Fatal(err)
		}
		claudeKey := "sk-ant-api03-test-key"
		anthropicEndpoint, err := h.AddEndpoint(t.Context(), EndpointInput{Kind: "anthropic", BaseURL: up.URL, APIKey: &claudeKey})
		if err != nil {
			t.Fatal(err)
		}
		if _, err := h.AddEndpoint(t.Context(), EndpointInput{Kind: "anthropic", BaseURL: up.URL, APIKey: &claudeKey}); err == nil {
			t.Error("the same key twice was accepted")
		}
		find := func(id string) (Endpoint, bool) {
			o, err := h.Overview(t.Context(), "24h")
			if err != nil {
				t.Fatal(err)
			}
			for _, e := range o.Endpoints {
				if e.ID == id {
					return e, true
				}
			}
			return Endpoint{}, false
		}
		e, ok := find(added.ID)
		if !ok || e.BaseURL != up.URL+"/v1" || len(e.Models) != 1 || e.Models[0].Alias != "second-model" || e.KeyHint != "…0001" {
			t.Fatalf("added = %+v (found %v)", e, ok)
		}
		a, ok := find(anthropicEndpoint.ID)
		if !ok || a.Kind != "anthropic" || a.KeyHint != "…-key" || len(a.Models) != 0 || a.Disabled {
			client, _ := h.running()
			raw, _ := client.rawList(t.Context(), "claude-api-key")
			t.Fatalf("anthropic = %+v (found %v); added %+v; raw %v", a, ok, anthropicEndpoint, raw)
		}
		// The gateway serves what the endpoints add, under their aliases.
		deadline := time.Now().Add(10 * time.Second)
		for time.Now().Before(deadline) {
			served, _ := h.Models(t.Context())
			if slices.ContainsFunc(served, func(m Model) bool { return m.ID == "second-model" }) {
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		if served, _ := h.Models(t.Context()); !slices.ContainsFunc(served, func(m Model) bool { return m.ID == "second-model" }) {
			t.Errorf("served = %+v", served)
		}
		// An edit without a key keeps the key.
		updated, err := h.UpdateEndpoint(t.Context(), added.ID, EndpointInput{Name: "second", BaseURL: up.URL + "/v1",
			Models: []EndpointModel{{Name: "other-model"}, {Name: "fake-model"}}})
		if err != nil || updated.ID != added.ID || len(updated.Models) != 2 {
			t.Fatalf("updated = %+v, %v", updated, err)
		}
		if models, err := h.ProbeEndpoint(t.Context(), added.ID, EndpointInput{BaseURL: up.URL + "/v1"}); err != nil || len(models) != 2 {
			t.Errorf("probe with the saved key = %v, %v", models, err)
		}
		if _, err := h.UpdateEndpoint(t.Context(), added.ID, EndpointInput{Name: "fakeai", BaseURL: up.URL + "/v1", Models: updated.Models}); err == nil {
			t.Error("a name taken by another endpoint was accepted")
		}
		for _, id := range []string{added.ID, anthropicEndpoint.ID} {
			if err := h.SetEndpointDisabled(t.Context(), id, true); err != nil {
				t.Fatal(err)
			}
			if e, _ := find(id); !e.Disabled || e.State != StateDisabled {
				t.Errorf("disabled %s = %+v", id, e)
			}
			if err := h.SetEndpointDisabled(t.Context(), id, false); err != nil {
				t.Fatal(err)
			}
			if e, _ := find(id); e.Disabled {
				t.Errorf("enabled %s = %+v", id, e)
			}
			if err := h.RemoveEndpoint(t.Context(), id); err != nil {
				t.Fatal(err)
			}
			if _, ok := find(id); ok {
				t.Errorf("removed %s is still listed", id)
			}
		}
		if err := h.RemoveEndpoint(t.Context(), "anthropic-0000000000"); !errors.Is(err, ErrNoEndpoint) {
			t.Errorf("an unknown endpoint: %v", err)
		}
		// What was added and removed is gone from the file too.
		cfg, err := sdkconfig.LoadConfigOptional(filepath.Join(dir, "config.yaml"), false)
		if err != nil || len(cfg.OpenAICompatibility) != 1 || cfg.OpenAICompatibility[0].Name != "fakeai" || len(cfg.ClaudeKey) != 0 {
			t.Errorf("config after the round trip: %+v, %v", cfg, err)
		}
	})

	t.Run("imported credential", func(t *testing.T) {
		credential := []byte(`{"type":"claude","email":"someone@example.com","access_token":"sk-ant-oat01-test",
			"refresh_token":"sk-ant-ort01-test","expired":"2099-01-01T00:00:00Z","last_refresh":"2026-09-24T10:00:00Z"}`)
		for _, bad := range []string{"../x.json", "x.txt", ".hidden.json", ""} {
			if err := h.Import(t.Context(), bad, credential); err == nil {
				t.Errorf("Import(%q) was accepted", bad)
			}
		}
		if err := h.Import(t.Context(), "claude-someone@example.com.json", credential); err != nil {
			t.Fatal(err)
		}
		find := func() (Account, bool) {
			list, err := h.Accounts(t.Context(), "7d")
			if err != nil {
				t.Fatal(err)
			}
			for _, a := range list {
				if a.Name == "claude-someone@example.com.json" {
					return a, true
				}
			}
			return Account{}, false
		}
		var a Account
		var ok bool
		for deadline := time.Now().Add(10 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
			if a, ok = find(); ok {
				break
			}
		}
		if !ok || a.Provider != "claude" || a.Label != "someone@example.com" || a.State != StateReady || a.Config || !a.QuotaSupported {
			t.Fatalf("imported = %+v (listed %v)", a, ok)
		}
		if len(a.Uptime.Slots) != 42 {
			t.Errorf("a week has %d slots", len(a.Uptime.Slots))
		}
		// The gateway takes the account up a moment after it is saved, and
		// then the account lists the models it serves, as the gateway does.
		for deadline := time.Now().Add(10 * time.Second); len(a.Models) == 0 && time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
			a, _ = find()
		}
		if !slices.Contains(a.Models, "claude-opus-5-5") {
			t.Errorf("the imported account serves %v", a.Models)
		}
		served, err := h.Models(t.Context())
		if err != nil {
			t.Fatal(err)
		}
		opus := slices.IndexFunc(served, func(m Model) bool { return m.ID == "claude-opus-5-5" })
		if opus < 0 || !slices.Contains(served[opus].Clients, a.ID) || !slices.Contains(served[opus].Via, "Claude · someone@example.com") || served[opus].Name == "" {
			t.Errorf("the gateway's models do not name the account: %+v", served)
		}
		if err := h.SetDisabled(t.Context(), a.Name, true); err != nil {
			t.Fatal(err)
		}
		if a, _ = find(); a.State != StateDisabled {
			t.Errorf("disabled: %q", a.State)
		}
		if err := h.SetDisabled(t.Context(), a.Name, false); err != nil {
			t.Fatal(err)
		}
		if a, _ = find(); a.State != StateReady {
			t.Errorf("enabled again: %q", a.State)
		}
		if err := h.Remove(t.Context(), a.Name); err != nil {
			t.Fatal(err)
		}
		if _, ok := find(); ok {
			t.Error("the removed account is still listed")
		}
		if _, err := os.Stat(filepath.Join(dir, "auths", a.Name)); !errors.Is(err, os.ErrNotExist) {
			t.Errorf("the credential is still there: %v", err)
		}
		if err := h.SetDisabled(t.Context(), "nobody.json", true); !errors.Is(err, ErrNoAccount) {
			t.Errorf("unknown account: %v", err)
		}
	})

	t.Run("sign-in", func(t *testing.T) {
		login, err := h.Login(t.Context(), "codex")
		if err != nil {
			t.Fatal(err)
		}
		if !strings.HasPrefix(login.URL, "https://auth.openai.com/") || login.State == "" || login.Flow != FlowRedirect || !login.Manual {
			t.Fatalf("login = %+v", login)
		}
		state, err := h.LoginState(t.Context(), login.State)
		if err != nil || state.Status != "wait" {
			t.Fatalf("state = %+v, %v", state, err)
		}
		for _, bad := range []string{"not a url", "http://localhost:1455/auth/callback?code=x&state=other", "http://localhost:1455/auth/callback?state=" + login.State} {
			if err := h.FinishLogin(t.Context(), login.State, bad); err == nil {
				t.Errorf("FinishLogin(%q) was accepted", bad)
			}
		}
		if _, err := h.LoginState(t.Context(), "someone-elses"); !errors.Is(err, ErrNoLogin) {
			t.Errorf("a sign-in started elsewhere: %v", err)
		}
		if err := h.CancelLogin(t.Context(), login.State); err != nil {
			t.Fatal(err)
		}
		if _, err := h.LoginState(t.Context(), login.State); !errors.Is(err, ErrNoLogin) {
			t.Errorf("a cancelled sign-in: %v", err)
		}
		if _, err := h.Login(t.Context(), "myspace"); err == nil {
			t.Error("an unknown provider was accepted")
		}
	})

	t.Run("history is saved", func(t *testing.T) {
		if err := h.history.Save(); err != nil {
			t.Fatal(err)
		}
		data, err := os.ReadFile(filepath.Join(dir, "history.json"))
		if err != nil {
			t.Fatal(err)
		}
		var saved historyFile
		if err := json.Unmarshal(data, &saved); err != nil || len(saved.Accounts) != 1 {
			t.Errorf("history = %s, %v", data, err)
		}
	})
}
