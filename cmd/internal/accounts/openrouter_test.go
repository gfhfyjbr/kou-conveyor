package accounts

import (
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

// The tests never ask OpenRouter itself.
func init() { os.Setenv("KOU_CONVEYOR_OPENROUTER_PRICES", "off") }

func TestOpenRouter(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"data":[
			{"id":"anthropic/claude-opus-5.5","pricing":{"prompt":"0.000004","completion":"0.00002","input_cache_read":"0.0000004","input_cache_write":"0.000005","input_cache_write_1h":"0.000008"}},
			{"id":"anthropic/claude-opus-5.5:batch","pricing":{"prompt":"0.000002","completion":"0.00001"}},
			{"id":"anthropic/claude-opus-4.5","pricing":{"prompt":"0.000005","completion":"0.000025","input_cache_read":"0.0000005","input_cache_write":"0.00000625"}},
			{"id":"openai/gpt-6-sol","pricing":{"prompt":"0.000002","completion":"0.00001","input_cache_read":"0.0000002"}},
			{"id":"~anthropic/claude-latest","pricing":{"prompt":"0.1","completion":"0.1"}},
			{"id":"meta/free-model","pricing":{"prompt":"0","completion":"0"}},
			{"id":"odd/model","pricing":{"prompt":"n/a","completion":"1"}}
		]}`)
	}))
	defer srv.Close()
	path := filepath.Join(t.TempDir(), "openrouter-prices.json")
	o := NewOpenRouter(path)
	o.url, o.enabled = srv.URL, true
	if err := o.Refresh(t.Context()); err != nil {
		t.Fatal(err)
	}
	check := func(o *OpenRouter, model string, want Price) {
		t.Helper()
		p, ok := o.Lookup(model)
		if !ok || p.Price != want || p.Source != PriceOpenRouter {
			t.Errorf("%s = %+v, %v; want %+v", model, p, ok, want)
		}
	}
	// Claude's cache writes at the hour's price.
	check(o, "claude-opus-5-5", Price{Input: 4, Output: 20, CacheRead: 0.4, CacheWrite: 8})
	check(o, "claude-opus-4-5-20251101", Price{Input: 5, Output: 25, CacheRead: 0.5, CacheWrite: 6.25})
	// No cache write listed: it costs the input.
	check(o, "gpt-6-sol", Price{Input: 2, Output: 10, CacheRead: 0.2, CacheWrite: 2})
	for _, missing := range []string{"claude-latest", "free-model", "model", "gpt-6-luna"} {
		if _, ok := o.Lookup(missing); ok {
			t.Errorf("%s is listed", missing)
		}
	}
	if s := o.Status(); s.Models != 3 || s.Fetched.IsZero() || s.Error != "" {
		t.Errorf("status = %+v", s)
	}
	// Kept for the next start, before it fetches again.
	check(NewOpenRouter(path), "claude-opus-5-5", Price{Input: 4, Output: 20, CacheRead: 0.4, CacheWrite: 8})

	// The user's prices come first, then OpenRouter's, then those built in.
	book := NewPriceBook("")
	book.market = o
	if p := book.Resolve("claude-opus-5-5"); p.Source != PriceOpenRouter || p.Input != 4 {
		t.Errorf("opus 5.5 = %+v", p)
	}
	if p := book.Resolve("claude-sonnet-4-5"); p.Source != PriceList {
		t.Errorf("not on the list = %+v", p)
	}
	if _, err := book.Set(PriceRule{Match: "claude-opus-5*", Price: Price{Input: 1, Output: 1}}); err != nil {
		t.Fatal(err)
	}
	if p := book.Resolve("claude-opus-5-5"); p.Source != PriceCustom {
		t.Errorf("yours = %+v", p)
	}

	o.url = srv.URL + "/gone"
	srv.Config.Handler = http.NotFoundHandler()
	if err := o.Refresh(t.Context()); err == nil || o.Status().Error == "" || o.Status().Models != 3 {
		t.Errorf("a failed fetch keeps the list: %v, %+v", err, o.Status())
	}
}
