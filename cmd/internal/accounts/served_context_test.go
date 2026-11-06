package accounts

import (
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync/atomic"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v8/sdk/cliproxy"
)

func TestServedContextWindowReadsRawRegistry(t *testing.T) {
	reg := fakeRegistry{
		clients: map[string][]*cliproxy.ModelInfo{
			"client": {
				{ID: "served-only-alias", ContextLength: 1_000_000},
				{ID: "input-limit-alias", InputTokenLimit: 750_000},
				{ID: "unknown-capacity"},
				{ID: "negative-capacity", ContextLength: -1},
			},
		},
		available: []map[string]any{{"id": "available-only-alias", "context_length": float64(500_000)}},
	}
	for model, want := range map[string]int64{
		"served-only-alias":    1_000_000,
		"input-limit-alias":    750_000,
		"available-only-alias": 500_000,
		"unknown-capacity":     0,
		"negative-capacity":    0,
		"unregistered":         0,
		"SERVED-ONLY-ALIAS":    0,
	} {
		if got := servedContextWindow(reg, []string{"client"}, model); got != want {
			t.Errorf("%s capacity = %d, want %d", model, got, want)
		}
	}
	other := fakeRegistry{available: []map[string]any{{"id": "served-only-alias", "context_length": float64(64_000)}}}
	if got := servedContextWindow(other, nil, "served-only-alias"); got != 64_000 {
		t.Fatalf("a different registry inherited the first one's capacity: %d", got)
	}
	if got := servedContextWindow(fakeRegistry{}, nil, "served-only-alias"); got != 0 {
		t.Fatalf("missing metadata inherited a previous capacity: %d", got)
	}
}

func TestServedContextWindowDoesNotFetchOrUseSavedWindows(t *testing.T) {
	var requests atomic.Int64
	management := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		http.Error(w, "capacity lookup must not ask for account attribution", http.StatusInternalServerError)
	}))
	defer management.Close()
	host := &Host{
		client:   newClient(management.URL, "fixture"),
		clients:  newClientWatch(),
		contexts: NewContextBook(filepath.Join(t.TempDir(), "windows.json")),
	}
	const model = "saved-but-not-in-live-registry"
	if _, err := host.SetContextWindow(ContextWindow{Model: model, Tokens: 1_000_000}); err != nil {
		t.Fatal(err)
	}
	if got := host.ContextWindow(model); got != 1_000_000 {
		t.Fatalf("saved window changed: %d", got)
	}
	if got := host.ServedContextWindow(model); got != 0 {
		t.Fatalf("saved advice was promoted to serving metadata: %d", got)
	}
	if got := requests.Load(); got != 0 {
		t.Fatalf("capacity lookup made %d management requests", got)
	}
	host.client = nil
	if got := host.ServedContextWindow(model); got != 0 {
		t.Fatalf("a stopped host supplied capacity: %d", got)
	}
}
