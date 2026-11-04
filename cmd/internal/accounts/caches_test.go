package accounts

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestCacheBook(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache-ttls.json")
	book := NewCacheBook(path)
	if book.Err() != nil || len(book.TTLs()) != 0 || book.Resolve("claude-opus-5-5") != "" {
		t.Fatalf("a new book = %v, %+v", book.Err(), book.TTLs())
	}
	got, err := book.Set(CacheTTL{Model: " Claude-Opus-5-5 ", TTL: " 5M "})
	if err != nil || got != (CacheTTL{Model: "claude-opus-5-5", TTL: CacheTTL5m}) {
		t.Fatalf("set = %+v, %v", got, err)
	}
	if ttl := book.Resolve("CLAUDE-OPUS-5-5"); ttl != CacheTTL5m {
		t.Fatalf("resolve = %q", ttl)
	}
	if data, err := os.ReadFile(path); err != nil || !strings.Contains(string(data), `"version":1`) || !strings.Contains(string(data), `"ttl":"5m"`) {
		t.Fatalf("file = %s, %v", data, err)
	}

	// The file, not just the running book, carries the setting to the next
	// cockpit. Setting the same model again replaces rather than duplicates it.
	reloaded := NewCacheBook(path)
	if ttl := reloaded.Resolve("claude-opus-5-5"); ttl != CacheTTL5m {
		t.Fatalf("reloaded TTL = %q", ttl)
	}
	if _, err := reloaded.Set(CacheTTL{Model: "claude-opus-5-5", TTL: CacheTTL1h}); err != nil {
		t.Fatal(err)
	}
	if ttls := reloaded.TTLs(); !slices.Equal(ttls, []CacheTTL{{Model: "claude-opus-5-5", TTL: CacheTTL1h}}) {
		t.Fatalf("after replacement = %+v", ttls)
	}

	for _, ttl := range []CacheTTL{
		{TTL: CacheTTL5m},
		{Model: "claude-*", TTL: CacheTTL5m},
		{Model: "claude opus", TTL: CacheTTL5m},
		{Model: "claude-opus-5-5", TTL: "30m"},
		{Model: "claude-opus-5-5", TTL: "0"},
	} {
		if _, err := reloaded.Set(ttl); err == nil {
			t.Errorf("Set(%+v) succeeded", ttl)
		}
	}
	if ttl := reloaded.Resolve("claude-opus-5-5"); ttl != CacheTTL1h {
		t.Fatalf("an invalid set changed the TTL to %q", ttl)
	}
	if err := reloaded.Remove("CLAUDE-OPUS-5-5"); err != nil {
		t.Fatal(err)
	}
	if reloaded.Resolve("claude-opus-5-5") != "" || len(NewCacheBook(path).TTLs()) != 0 {
		t.Fatal("the removal was not saved")
	}
	if err := reloaded.Remove("claude-opus-5-5"); err == nil {
		t.Fatal("removing an unset TTL succeeded")
	}
}

func TestCacheBookReadError(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache-ttls.json")
	if err := os.WriteFile(path, []byte("not JSON"), 0o600); err != nil {
		t.Fatal(err)
	}
	book := NewCacheBook(path)
	if book.Err() == nil || book.Resolve("claude-opus-5-5") != "" {
		t.Fatalf("a malformed book = %v, %q", book.Err(), book.Resolve("claude-opus-5-5"))
	}
}

func TestCacheTTLAppearsInModelListing(t *testing.T) {
	h := &Host{
		market:   NewOpenRouter(""),
		contexts: NewContextBook(""),
		caches:   NewCacheBook(""),
	}
	if _, err := h.SetCacheTTL(CacheTTL{Model: "Cached-Model", TTL: CacheTTL5m}); err != nil {
		t.Fatal(err)
	}
	if _, err := h.SetContextWindow(ContextWindow{Model: "Window-Model", Tokens: 32_000}); err != nil {
		t.Fatal(err)
	}
	listing := h.ContextWindows(t.Context())
	want := []ContextModel{{ID: "cached-model", TTL: CacheTTL5m}, {ID: "window-model", Window: 32_000}}
	if !slices.Equal(listing.Models, want) {
		t.Fatalf("models = %+v, want %+v", listing.Models, want)
	}
	if err := h.RemoveCacheTTL("cached-model"); err != nil {
		t.Fatal(err)
	}
	if models := h.ContextWindows(t.Context()).Models; len(models) != 1 || models[0].ID != "window-model" {
		t.Fatalf("after removal = %+v", models)
	}
}
