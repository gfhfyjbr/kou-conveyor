package main

import (
	"net/http"
	"slices"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestCacheTTLs(t *testing.T) {
	h := newHarness(t)
	if res, body := h.do("POST", "/api/cache-ttls", `{"model":"fake-model","ttl":"5m"}`); res.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("set without a gateway = %d %v", res.StatusCode, body)
	}
	if env := h.server.cacheEnv("fake-model"); env != nil {
		t.Errorf("a run's env without a gateway = %v", env)
	}

	gateway, err := accounts.New(accounts.Options{Dir: t.TempDir(), Address: "127.0.0.1:8318"})
	if err != nil {
		t.Fatal(err)
	}
	h.server.gateway = gateway
	for _, bad := range []string{
		`{"model":"fake-model","ttl":"30m"}`,
		`{"model":"fake-*","ttl":"5m"}`,
		`{"ttl":"5m"}`,
	} {
		if res, body := h.do("POST", "/api/cache-ttls", bad); res.StatusCode != http.StatusBadRequest {
			t.Errorf("%s = %d %v", bad, res.StatusCode, body)
		}
	}

	res, body := h.do("POST", "/api/cache-ttls", `{"model":"Fake-Model","ttl":"5m"}`)
	models, _ := body["models"].([]any)
	if res.StatusCode != http.StatusOK || len(models) != 1 {
		t.Fatalf("set = %d %v", res.StatusCode, body)
	}
	if m, _ := models[0].(map[string]any); m["id"] != "fake-model" || m["ttl"] != "5m" || m["served"] != nil {
		t.Errorf("listed = %v", m)
	}
	if env := h.server.cacheEnv("FAKE-MODEL"); !slices.Equal(env, []string{runconfig.CacheTTLEnvironment + "=5m"}) {
		t.Errorf("a run's env = %v", env)
	}
	if env := h.server.cacheEnv("other-model"); env != nil {
		t.Errorf("another model's run's env = %v", env)
	}

	res, body = h.do("POST", "/api/cache-ttls", `{"model":"fake-model","ttl":"1h"}`)
	if res.StatusCode != http.StatusOK || h.server.gateway.CacheTTL("fake-model") != "1h" {
		t.Fatalf("replace = %d %v", res.StatusCode, body)
	}
	res, body = h.do("DELETE", "/api/cache-ttls?model=FAKE-MODEL", "")
	if models, _ := body["models"].([]any); res.StatusCode != http.StatusOK || len(models) != 0 {
		t.Errorf("remove = %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("DELETE", "/api/cache-ttls?model=fake-model", ""); res.StatusCode != http.StatusNotFound {
		t.Errorf("remove again = %d", res.StatusCode)
	}
	if env := h.server.cacheEnv("fake-model"); env != nil {
		t.Errorf("a run's env after the removal = %v", env)
	}
}
