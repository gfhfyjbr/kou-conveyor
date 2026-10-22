package accounts

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestOpenRouterWindows(t *testing.T) {
	var windows atomic.Bool
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !windows.Load() {
			// As the list was kept before the windows were.
			_, _ = io.WriteString(w, `{"data":[{"id":"anthropic/claude-opus-5.5","pricing":{"prompt":"0.000004","completion":"0.00002"}}]}`)
			return
		}
		_, _ = io.WriteString(w, `{"data":[
			{"id":"anthropic/claude-opus-5.5","context_length":200000,"pricing":{"prompt":"0.000004","completion":"0.00002"}},
			{"id":"anthropic/claude-opus-5.5:extended","context_length":1000000,"pricing":{"prompt":"0.000008","completion":"0.00004"}},
			{"id":"google/gemini-4-pro","context_length":"1048576","pricing":{"prompt":"0.000002","completion":"0.00001"}},
			{"id":"openai/gpt-6-sol","context_length":null,"top_provider":{"context_length":400000},"pricing":{"prompt":"0.000002","completion":"0.00001"}},
			{"id":"odd/window","context_length":"lots","top_provider":{"context_length":-1},"pricing":{"prompt":"0.000001","completion":"0.000001"}},
			{"id":"odd/shape","context_length":{"max":5},"top_provider":"first","pricing":{"prompt":"0.000001","completion":"0.000001"}}
		]}`)
	}))
	defer srv.Close()
	path := filepath.Join(t.TempDir(), "openrouter-prices.json")
	o := NewOpenRouter(path)
	o.url, o.enabled = srv.URL, true
	if err := o.Refresh(t.Context()); err != nil {
		t.Fatal(err)
	}
	if got, id, ok := o.Window("claude-opus-5-5"); ok {
		t.Errorf("a list without windows gives %d (%s)", got, id)
	}
	// A list kept without the windows is fetched again at once.
	if kept := NewOpenRouter(path); kept.fresh(time.Hour) || !kept.stale {
		t.Error("the list kept without windows is taken as fresh")
	}

	windows.Store(true)
	if err := o.Refresh(t.Context()); err != nil {
		t.Fatal(err)
	}
	check := func(o *OpenRouter, model string, want int64, wantID string) {
		t.Helper()
		if got, id, ok := o.Window(model); !ok || got != want || id != wantID {
			t.Errorf("%s = %d (%s), %v; want %d (%s)", model, got, id, ok, want, wantID)
		}
	}
	// Not the variant's.
	check(o, "claude-opus-5-5", 200_000, "anthropic/claude-opus-5.5")
	check(o, "Claude-Opus-5-5-20260301", 200_000, "anthropic/claude-opus-5.5")
	// As a string.
	check(o, "gemini-4-pro", 1_048_576, "google/gemini-4-pro")
	// The first provider's, where the model lists none.
	check(o, "gpt-6-sol", 400_000, "openai/gpt-6-sol")
	// A window of an odd shape is none, and costs neither the model's price
	// nor the list.
	for _, model := range []string{"window", "shape", "gpt-6-luna"} {
		if got, id, ok := o.Window(model); ok {
			t.Errorf("%s = %d (%s)", model, got, id)
		}
	}
	for _, model := range []string{"window", "shape"} {
		if _, ok := o.Lookup(model); !ok {
			t.Errorf("%s is not priced", model)
		}
	}
	if s := o.Status(); s.Models != 5 || s.Error != "" {
		t.Errorf("status = %+v", s)
	}
	// Kept for the next start with the windows, and not fetched again
	// before its time.
	kept := NewOpenRouter(path)
	if !kept.fresh(time.Hour) || kept.stale {
		t.Error("the list kept with windows is taken as stale")
	}
	check(kept, "gpt-6-sol", 400_000, "openai/gpt-6-sol")
}

// openRouterWithWindows is an OpenRouter that lists Claude Opus 5.5's
// window, counting the times it is asked, and failing while down is set.
func openRouterWithWindows(t *testing.T) (o *OpenRouter, asked *atomic.Int32, down *atomic.Bool) {
	asked, down = new(atomic.Int32), new(atomic.Bool)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		asked.Add(1)
		if down.Load() {
			http.Error(w, "down", http.StatusInternalServerError)
			return
		}
		_, _ = io.WriteString(w, `{"data":[{"id":"anthropic/claude-opus-5.5","context_length":200000,"pricing":{"prompt":"0.000004","completion":"0.00002"}}]}`)
	}))
	t.Cleanup(srv.Close)
	o = NewOpenRouter("")
	o.url, o.enabled = srv.URL, true
	return o, asked, down
}

func TestOpenRouterWindow(t *testing.T) {
	o, asked, down := openRouterWithWindows(t)
	h := &Host{market: o, contexts: NewContextBook("")}
	got, err := h.OpenRouterWindow(t.Context(), " claude-opus-5-5 ")
	if err != nil || got.Model != "claude-opus-5-5" || got.Tokens != 200_000 || got.OpenRouter != "anthropic/claude-opus-5.5" || got.Fetched.IsZero() || got.Note != "" {
		t.Fatalf("opus 5.5 = %+v, %v", got, err)
	}
	// The list fetched minutes ago answers.
	if got, err := h.OpenRouterWindow(t.Context(), "claude-opus-5-5"); err != nil || got.Tokens != 200_000 || asked.Load() != 1 {
		t.Errorf("again = %+v, %v; asked %d times", got, err, asked.Load())
	}
	if _, err := h.OpenRouterWindow(t.Context(), "gpt-6-sol"); !errors.Is(err, ErrNotOnOpenRouter) || asked.Load() != 1 {
		t.Errorf("not listed = %v; asked %d times", err, asked.Load())
	}
	// Getting the window sets none.
	if w := h.ContextWindow("claude-opus-5-5"); w != 0 {
		t.Errorf("getting OpenRouter's window set %d", w)
	}

	// An older list is fetched again. When OpenRouter cannot be reached, the
	// list kept answers, and says so.
	o.mu.Lock()
	o.fetched = time.Now().Add(-time.Hour)
	o.mu.Unlock()
	down.Store(true)
	if got, err := h.OpenRouterWindow(t.Context(), "claude-opus-5-5"); err != nil || got.Tokens != 200_000 || got.Note == "" || asked.Load() != 2 {
		t.Errorf("OpenRouter down = %+v, %v; asked %d times", got, err, asked.Load())
	}
	// Where the list kept lacks the model, OpenRouter's failure is the answer.
	if _, err := h.OpenRouterWindow(t.Context(), "gpt-6-sol"); err == nil || errors.Is(err, ErrNotOnOpenRouter) {
		t.Errorf("OpenRouter down, not listed = %v", err)
	}

	o.enabled = false
	if _, err := h.OpenRouterWindow(t.Context(), "claude-opus-5-5"); !errors.Is(err, ErrOpenRouterOff) {
		t.Errorf("off = %v", err)
	}
}

func TestContextWindows(t *testing.T) {
	o, _, _ := openRouterWithWindows(t)
	if err := o.Refresh(t.Context()); err != nil {
		t.Fatal(err)
	}
	h := &Host{market: o, contexts: NewContextBook(filepath.Join(t.TempDir(), "context-windows.json"))}
	if _, err := h.SetContextWindow(ContextWindow{Model: "Local-Model", Tokens: 32_000}); err != nil {
		t.Fatal(err)
	}
	if _, err := h.SetContextWindow(ContextWindow{Model: "claude-opus-5-5", Tokens: 200_000, OpenRouter: "anthropic/claude-opus-5.5"}); err != nil {
		t.Fatal(err)
	}
	if w := h.ContextWindow("Claude-Opus-5-5"); w != 200_000 {
		t.Errorf("opus 5.5's runs take %d", w)
	}
	// The gateway is not running: the windows set are listed all the same,
	// with OpenRouter's, and the listing says why no model is served.
	listing := h.ContextWindows(t.Context())
	want := []ContextModel{
		{ID: "claude-opus-5-5", Window: 200_000, From: "anthropic/claude-opus-5.5", OpenRouter: 200_000, OpenRouterID: "anthropic/claude-opus-5.5"},
		{ID: "local-model", Window: 32_000},
	}
	if !slices.Equal(listing.Models, want) || !strings.Contains(listing.Error, ErrNotRunning.Error()) || !listing.OpenRouter.Enabled || listing.OpenRouter.Models != 1 {
		t.Errorf("listing = %+v", listing)
	}
	if err := h.RemoveContextWindow("local-model"); err != nil {
		t.Fatal(err)
	}
	if models := h.ContextWindows(t.Context()).Models; len(models) != 1 || models[0].ID != "claude-opus-5-5" {
		t.Errorf("after the removal = %+v", models)
	}
}

func TestContextBook(t *testing.T) {
	path := filepath.Join(t.TempDir(), "context-windows.json")
	b := NewContextBook(path)
	if b.Err() != nil || len(b.Windows()) != 0 || b.Resolve("claude-opus-5-5") != 0 {
		t.Fatalf("a new book = %v, %v", b.Err(), b.Windows())
	}
	if w, err := b.Set(ContextWindow{Model: " Claude-Opus-5-5 ", Tokens: 200_000, OpenRouter: " anthropic/claude-opus-5.5 "}); err != nil ||
		w != (ContextWindow{Model: "claude-opus-5-5", Tokens: 200_000, OpenRouter: "anthropic/claude-opus-5.5"}) {
		t.Fatalf("set = %+v, %v", w, err)
	}
	if _, err := b.Set(ContextWindow{Model: "gpt-6-sol", Tokens: 400_000}); err != nil {
		t.Fatal(err)
	}
	// Set again, it replaces the window.
	if _, err := b.Set(ContextWindow{Model: "GPT-6-SOL", Tokens: 272_000}); err != nil {
		t.Fatal(err)
	}
	for model, want := range map[string]int64{"gpt-6-sol": 272_000, "CLAUDE-OPUS-5-5": 200_000, "claude-opus-5": 0, "claude-opus-5-5-20260301": 0} {
		if got := b.Resolve(model); got != want {
			t.Errorf("%s = %d, want %d", model, got, want)
		}
	}
	for _, bad := range []ContextWindow{
		{Tokens: 200_000},
		{Model: "claude-*", Tokens: 200_000},
		{Model: "claude opus", Tokens: 200_000},
		{Model: strings.Repeat("m", 201), Tokens: 200_000},
		{Model: "m", Tokens: MinContextWindow - 1},
		{Model: "m", Tokens: MaxContextWindow + 1},
		{Model: "m", Tokens: 200_000, OpenRouter: "anthropic/claude opus"},
	} {
		if _, err := b.Set(bad); err == nil {
			t.Errorf("%+v is taken", bad)
		}
	}
	// Kept for the next start.
	again := NewContextBook(path)
	want := []ContextWindow{{Model: "claude-opus-5-5", Tokens: 200_000, OpenRouter: "anthropic/claude-opus-5.5"}, {Model: "gpt-6-sol", Tokens: 272_000}}
	if again.Err() != nil || !slices.Equal(again.Windows(), want) {
		t.Errorf("kept = %+v, %v", again.Windows(), again.Err())
	}
	if err := again.Remove(" GPT-6-SOL "); err != nil {
		t.Fatal(err)
	}
	if err := again.Remove("gpt-6-sol"); err == nil {
		t.Error("a window removed is removed again")
	}
	if got := NewContextBook(path).Resolve("gpt-6-sol"); got != 0 {
		t.Errorf("removed, kept: %d", got)
	}

	// A file of another version is not read; saving replaces it.
	if err := os.WriteFile(path, []byte(`{"version":2,"windows":[{"model":"m","tokens":5000}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	odd := NewContextBook(path)
	if odd.Err() == nil || odd.Resolve("m") != 0 {
		t.Errorf("version 2 = %v, %d", odd.Err(), odd.Resolve("m"))
	}
	if _, err := odd.Set(ContextWindow{Model: "m", Tokens: 5000}); err != nil || odd.Err() != nil {
		t.Errorf("saving over version 2 = %v, %v", err, odd.Err())
	}
	if got := NewContextBook(path).Resolve("m"); got != 5000 {
		t.Errorf("saved over version 2: %d", got)
	}

	// The windows kept are bounded.
	many := NewContextBook("")
	for i := range maxContextWindows {
		if _, err := many.Set(ContextWindow{Model: fmt.Sprintf("model-%d", i), Tokens: 8_000}); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := many.Set(ContextWindow{Model: "one-more", Tokens: 8_000}); err == nil {
		t.Error("a window over the bound is taken")
	}
	if _, err := many.Set(ContextWindow{Model: "model-0", Tokens: 16_000}); err != nil {
		t.Errorf("a window replaced at the bound: %v", err)
	}

	// No book, no window.
	var none *ContextBook
	if none.Resolve("m") != 0 {
		t.Error("no book gives a window")
	}
}
