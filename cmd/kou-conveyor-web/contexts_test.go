package main

import (
	"net/http"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestContextWindows(t *testing.T) {
	h := newHarness(t)
	// Without the gateway no window is set, and runs take the runner's.
	if res, body := h.do("GET", "/api/context-windows", ""); res.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("context windows without a gateway = %d %v", res.StatusCode, body)
	}
	if env := h.server.contextEnv("claude-opus-5-5"); env != nil {
		t.Errorf("a run's env without a gateway = %v", env)
	}

	// The gateway keeps the windows, and needs not run for that. (It is
	// never started: TestAccountsGateway is the package's only test that
	// starts one.)
	gateway, err := accounts.New(accounts.Options{Dir: t.TempDir(), Address: "127.0.0.1:8318"})
	if err != nil {
		t.Fatal(err)
	}
	h.server.gateway = gateway
	for _, bad := range []string{`{"model":"fake-model","tokens":10}`, `{"model":"fake-*","tokens":64000}`, `{"tokens":64000}`} {
		if res, _ := h.do("POST", "/api/context-windows", bad); res.StatusCode != http.StatusBadRequest {
			t.Errorf("%s = %d", bad, res.StatusCode)
		}
	}
	res, body := h.do("POST", "/api/context-windows", `{"model":"Fake-Model","tokens":64000}`)
	models, _ := body["models"].([]any)
	if res.StatusCode != http.StatusOK || len(models) != 1 {
		t.Fatalf("set = %d %v", res.StatusCode, body)
	}
	if m, _ := models[0].(map[string]any); m["id"] != "fake-model" || m["window"] != 64000.0 || m["served"] != nil {
		t.Errorf("listed = %v", m)
	}
	if problem, _ := body["error"].(string); !strings.Contains(problem, "not running") {
		t.Errorf("the listing's error = %q", problem)
	}
	// The model's runs take it; another's take the runner's.
	if env := h.server.contextEnv("fake-model"); !slices.Equal(env, []string{runconfig.ContextWindowEnvironment + "=64000"}) {
		t.Errorf("a run's env = %v", env)
	}
	if env := h.server.contextEnv("other-model"); env != nil {
		t.Errorf("another model's run's env = %v", env)
	}
	// The tests never ask OpenRouter.
	if res, body := h.do("GET", "/api/context-windows/openrouter?model=fake-model", ""); res.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("OpenRouter off = %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("GET", "/api/context-windows/openrouter", ""); res.StatusCode != http.StatusBadRequest {
		t.Errorf("OpenRouter's window of no model = %d", res.StatusCode)
	}
	res, body = h.do("DELETE", "/api/context-windows?model=FAKE-MODEL", "")
	if models, _ := body["models"].([]any); res.StatusCode != http.StatusOK || len(models) != 0 {
		t.Errorf("remove = %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("DELETE", "/api/context-windows?model=fake-model", ""); res.StatusCode != http.StatusNotFound {
		t.Errorf("remove again = %d", res.StatusCode)
	}
	if env := h.server.contextEnv("fake-model"); env != nil {
		t.Errorf("a run's env after the removal = %v", env)
	}
}
