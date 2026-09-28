package accounts

import (
	"context"
	"math"
	"path/filepath"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/usage"
)

func TestTokensOf(t *testing.T) {
	for _, tt := range []struct {
		name     string
		provider string
		detail   usage.Detail
		want     Tokens
	}{
		{
			// Anthropic reports its caches apart from the input, and the
			// thinking within the output, which the gateway's Claude
			// executor breaks down.
			name: "claude", provider: "claude",
			detail: usage.Detail{InputTokens: 2, OutputTokens: 341, ReasoningTokens: 124, CacheReadTokens: 608, CacheCreationTokens: 788,
				TokenBreakdown: usage.NewIndependentTokenBreakdown(2, 608, 788, 341-124, 124, 0)},
			want: Tokens{Input: 2, CacheRead: 608, CacheWrite: 788, Output: 341, Reasoning: 124},
		},
		{
			name: "claude without a breakdown", provider: "claude",
			detail: usage.Detail{InputTokens: 2, OutputTokens: 341, CacheReadTokens: 608, CacheCreationTokens: 788},
			want:   Tokens{Input: 2, CacheRead: 608, CacheWrite: 788, Output: 341},
		},
		{
			// OpenAI counts the cached tokens within the input.
			name: "codex", provider: "codex",
			detail: usage.Detail{InputTokens: 1000, CachedTokens: 800, CacheReadTokens: 800, OutputTokens: 50, ReasoningTokens: 20},
			want:   Tokens{Input: 200, CacheRead: 800, Output: 50, Reasoning: 20},
		},
		{
			// Gemini reports the thoughts beside the output.
			name: "gemini", provider: "gemini",
			detail: usage.Detail{InputTokens: 100, OutputTokens: 30, ReasoningTokens: 12, CacheReadTokens: 40},
			want:   Tokens{Input: 60, CacheRead: 40, Output: 42, Reasoning: 12},
		},
		{
			// A provider the gateway does not know: cached tokens that fit in
			// the input are taken to be part of it.
			name: "unknown", provider: "devin",
			detail: usage.Detail{InputTokens: 100, OutputTokens: 30, CachedTokens: 40, CacheReadTokens: 40},
			want:   Tokens{Input: 60, CacheRead: 40, Output: 30},
		},
		{
			name: "only a total", provider: "meta",
			detail: usage.Detail{TotalTokens: 77},
			want:   Tokens{Other: 77},
		},
		{name: "nothing", provider: "claude"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := tokensOf(tt.detail, tt.provider, ""); got != tt.want {
				t.Errorf("tokens = %+v, want %+v", got, tt.want)
			}
		})
	}
}

func TestPriceResolution(t *testing.T) {
	book := NewPriceBook("")
	for _, tt := range []struct {
		model  string
		source string
		input  float64
		output float64
	}{
		{"claude-opus-4-5-20251101", PriceList, 5, 25},
		{"claude-opus-4-1-20250805", PriceList, 15, 75},
		{"claude-opus-4-20250514", PriceList, 15, 75},
		{"claude-sonnet-4-5-20250929", PriceList, 3, 15},
		{"claude-3-7-sonnet-20250219", PriceList, 3, 15},
		{"claude-haiku-4-5-20251001", PriceList, 1, 5},
		{"claude-3-5-haiku-20241022", PriceList, 0.8, 4},
		{"anthropic/claude-opus-4.5", PriceList, 5, 25},
		{"gemini-claude-sonnet-4-5-thinking", PriceList, 3, 15},
		{"claude-opus-5-5", PriceEstimate, 5, 25},
		{"claude-sonnet-4-6", PriceEstimate, 3, 15},
		{"claude-fable-5-1", PriceNone, 0, 0},
		{"gpt-5", PriceList, 1.25, 10},
		{"gpt-5-codex", PriceList, 1.25, 10},
		{"gpt-5.1-codex-mini", PriceList, 0.25, 2},
		{"gpt-5-mini-2025-08-07", PriceList, 0.25, 2},
		{"gpt-5.5", PriceEstimate, 1.25, 10},
		{"gpt-5.6-luna-mini", PriceEstimate, 0.25, 2},
		{"gpt-6-sol", PriceNone, 0, 0},
		{"gpt-4.1-mini", PriceList, 0.4, 1.6},
		{"o3", PriceList, 2, 8},
		{"o4-mini", PriceList, 1.1, 4.4},
		{"gemini-2.5-pro", PriceList, 1.25, 10},
		{"gemini-2.5-flash-lite", PriceList, 0.1, 0.4},
		{"gemini-3-pro-high", PriceList, 2, 12},
		{"grok-code-fast-1", PriceList, 0.2, 1.5},
		{"grok-4-fast-reasoning", PriceList, 0.2, 0.5},
		{"grok-4.7", PriceEstimate, 3, 15},
		{"grok-4.7-build-fast", PriceEstimate, 0.2, 0.5},
		{"grok-3-mini", PriceList, 0.3, 0.5},
		{"kimi-k2-0905-preview", PriceList, 0.6, 2.5},
		{"deepseek-chat", PriceList, 0.28, 0.42},
		{"grok-imagine-image", PriceNone, 0, 0},
		{"", PriceNone, 0, 0},
	} {
		got := book.Resolve(tt.model)
		if got.Source != tt.source || got.Input != tt.input || got.Output != tt.output {
			t.Errorf("%s = %+v, want %s %v/%v", tt.model, got, tt.source, tt.input, tt.output)
		}
		if got.Source == PriceEstimate && got.Like == "" {
			t.Errorf("%s: an estimate names the model it borrows from", tt.model)
		}
	}
	// Every list price names its models; every estimate the model it borrows from.
	for _, b := range BuiltinPrices() {
		if b.Source == PriceList && b.Name == "" || b.Source == PriceEstimate && b.Like == "" {
			t.Errorf("built-in %v: name %q, like %q", b.Match, b.Name, b.Like)
		}
	}
	// Claude's caches: reads a tenth of the input, writes for an hour twice.
	if p := book.Resolve("claude-opus-4-5"); p.CacheRead != 0.5 || p.CacheWrite != 10 {
		t.Errorf("opus 4.5 caches = %+v", p)
	}
	// Other providers' cache writes cost the input.
	if p := book.Resolve("gpt-5"); p.CacheRead != 0.125 || p.CacheWrite != 1.25 {
		t.Errorf("gpt-5 caches = %+v", p)
	}
}

func TestPriceBook(t *testing.T) {
	path := filepath.Join(t.TempDir(), "prices.json")
	book := NewPriceBook(path)
	for _, bad := range []PriceRule{
		{Match: ""}, {Match: "*"}, {Match: "a b"}, {Match: "m", Price: Price{Input: -1}}, {Match: "m", Price: Price{Output: math.Inf(1)}},
	} {
		if _, err := book.Set(bad); err == nil {
			t.Errorf("%+v was accepted", bad)
		}
	}
	if _, err := book.Set(PriceRule{Match: "GPT-6-*", Price: Price{Input: 2, Output: 16, CacheRead: 0.2, CacheWrite: 2}}); err != nil {
		t.Fatal(err)
	}
	if _, err := book.Set(PriceRule{Match: "gpt-6-sol*", Price: Price{Input: 3, Output: 24}}); err != nil {
		t.Fatal(err)
	}
	if _, err := book.Set(PriceRule{Match: "claude-opus-5-5", Price: Price{Input: 6, Output: 30}, Note: "my guess"}); err != nil {
		t.Fatal(err)
	}
	check := func(book *PriceBook, model, source, rule string, input float64) {
		t.Helper()
		if p := book.Resolve(model); p.Source != source || p.Rule != rule || p.Input != input {
			t.Errorf("%s = %+v, want %s by %q at %v", model, p, source, rule, input)
		}
	}
	check(book, "gpt-6-luna", PriceCustom, "gpt-6-*", 2)
	check(book, "gpt-6-sol", PriceCustom, "gpt-6-sol*", 3) // the more specific pattern
	check(book, "claude-opus-5-5", PriceCustom, "claude-opus-5-5", 6)
	check(book, "claude-opus-5", PriceEstimate, "claude-opus-*", 5)
	if _, err := book.SetUnit(Unit{Name: "credits", USD: 0.01}); err != nil {
		t.Fatal(err)
	}
	if _, err := book.SetUnit(Unit{Name: "credits", USD: 0}); err == nil {
		t.Error("a unit worth nothing was accepted")
	}

	again := NewPriceBook(path)
	if err := again.Err(); err != nil {
		t.Fatal(err)
	}
	check(again, "gpt-6-luna", PriceCustom, "gpt-6-*", 2)
	if p := again.Resolve("claude-opus-5-5"); p.Note != "my guess" {
		t.Errorf("note = %q", p.Note)
	}
	if u := again.Unit(); u.Name != "credits" || u.USD != 0.01 {
		t.Errorf("unit = %+v", u)
	}
	if rules := again.Rules(); len(rules) != 3 || rules[0].Match != "claude-opus-5-5" || rules[2].Match != "gpt-6-*" {
		t.Errorf("rules, most specific first = %+v", rules)
	}
	if err := again.Remove("gpt-6-*"); err != nil {
		t.Fatal(err)
	}
	if err := again.Remove("gpt-6-*"); err == nil {
		t.Error("removing a rule twice")
	}
	check(NewPriceBook(path), "gpt-6-luna", PriceNone, "", 0)
	if u, _ := again.SetUnit(Unit{Name: "usd"}); u != Dollars {
		t.Errorf("dollars = %+v", u)
	}
}

func TestGlobMatch(t *testing.T) {
	for _, tt := range []struct {
		pattern, s string
		want       bool
	}{
		{"gpt-6-*", "gpt-6-sol", true},
		{"gpt-6-*", "gpt-6", false},
		{"gpt-5*-mini*", "gpt-5-5-mini", true},
		{"gpt-5*-mini*", "gpt-5-mini", true}, // * stands for nothing too
		{"gpt-5*-mini*", "gpt-5-nano", false},
		{"*-mini", "gpt-5-mini", true},
		{"a*b*c", "abc", true},
		{"a*b*c", "acb", false},
		{"exact", "exact", true},
		{"exact", "exactly", false},
		{"*", "", true},
	} {
		if got := globMatch(tt.pattern, tt.s); got != tt.want {
			t.Errorf("globMatch(%q, %q) = %v", tt.pattern, tt.s, got)
		}
	}
}

func TestLabelOf(t *testing.T) {
	for id, want := range map[string]string{
		"claude-03595452-someone@gmail.com.json":   "someone@gmail.com",
		"xai-someone@gmail.com.json":               "someone@gmail.com",
		"mite_grained4h@icloud.com.json":           "mite_grained4h@icloud.com",
		"john-doe@example.com.json":                "john-doe@example.com",
		"openai-compatibility:fakeai:0a1b2c3d4e5f": "openai-compatibility:fakeai:0a1b2c3d4e5f",
		"": "No account",
	} {
		if got := labelOf(id); got != want {
			t.Errorf("labelOf(%q) = %q, want %q", id, got, want)
		}
	}
}

// ledgerAt is a ledger in memory whose clock stands at now.
func ledgerAt(now time.Time) *Ledger {
	l := NewLedger("")
	l.now = func() time.Time { return now }
	return l
}

func TestLedgerReport(t *testing.T) {
	moscow := time.FixedZone("MSK", 3*3600)
	now := time.Date(2026, 9, 28, 20, 30, 0, 0, moscow) // a Monday
	l := ledgerAt(now)
	ctx := context.Background()
	claude := func(at time.Time, model string, in, read, write, out int64) usage.Record {
		return usage.Record{AuthID: "claude-a@example.com.json", AuthIndex: "ia", Provider: "claude", Model: model, RequestedAt: at, Latency: time.Second,
			Detail: usage.Detail{InputTokens: in, CacheReadTokens: read, CacheCreationTokens: write, OutputTokens: out}}
	}
	codex := func(at time.Time, model string, in, cached, out int64) usage.Record {
		return usage.Record{AuthID: "codex-b@example.com.json", AuthIndex: "ib", Provider: "codex", Model: model, RequestedAt: at, Latency: 3 * time.Second,
			Detail: usage.Detail{InputTokens: in, CachedTokens: cached, CacheReadTokens: cached, OutputTokens: out}}
	}
	// This hour, and the hours before.
	l.HandleUsage(ctx, claude(now.Add(-10*time.Minute), "claude-opus-4-5", 1_000, 1_000_000, 100_000, 10_000))
	l.HandleUsage(ctx, claude(now.Add(-20*time.Minute), "claude-opus-4-5", 1_000, 1_000_000, 100_000, 10_000))
	l.HandleUsage(ctx, codex(now.Add(-3*time.Hour), "gpt-5", 2_000_000, 1_500_000, 100_000))
	l.HandleUsage(ctx, codex(now.Add(-3*time.Hour), "gpt-6-sol", 1_000_000, 0, 10_000)) // no price
	failed := claude(now.Add(-5*time.Hour), "claude-opus-4-5", 0, 0, 0, 0)
	failed.Failed = true
	l.HandleUsage(ctx, failed)
	// Yesterday, three days ago, and the day before the week.
	l.HandleUsage(ctx, claude(now.Add(-26*time.Hour), "claude-sonnet-4-5", 1_000_000, 0, 0, 1_000_000))
	l.HandleUsage(ctx, codex(now.Add(-3*24*time.Hour), "gpt-5", 1_000_000, 0, 0))
	l.HandleUsage(ctx, codex(now.Add(-8*24*time.Hour), "gpt-5", 1_000_000, 0, 0))
	// A request that reached no account and used nothing is not recorded.
	l.HandleUsage(ctx, usage.Record{Provider: "claude", Failed: true})

	price := NewPriceBook("").Resolve
	day, err := l.Report(UsageQuery{Range: "24h", Location: moscow}, price)
	if err != nil {
		t.Fatal(err)
	}
	if len(day.Starts) != 24 || day.Slot != 3600 {
		t.Fatalf("slots = %d of %ds", len(day.Starts), day.Slot)
	}
	// The slots end with the hour in progress.
	if want := time.Date(2026, 9, 28, 21, 0, 0, 0, moscow); !day.To.Equal(want) || !day.From.Equal(want.Add(-24*time.Hour)) {
		t.Errorf("span = %v – %v", day.From, day.To)
	}
	tot := day.Totals
	if tot.Requests != 5 || tot.Failed != 1 {
		t.Errorf("requests = %d, failed %d", tot.Requests, tot.Failed)
	}
	// Opus 4.5: 2k input at $5, 2M cache reads at $0.50, 200k writes at
	// $10, 20k output at $25; GPT-5: 500k input at $1.25, 1.5M cached at
	// $0.125, 100k output at $10; GPT-6 has no price.
	wantCost := (2_000*5+2_000_000*0.5+200_000*10+20_000*25)/1e6 + (500_000*1.25+1_500_000*0.125+100_000*10)/1e6
	if math.Abs(tot.Cost.Total()-wantCost) > 1e-9 {
		t.Errorf("cost = %v, want %v", tot.Cost.Total(), wantCost)
	}
	if tot.Unpriced != 1_010_000 {
		t.Errorf("unpriced = %d", tot.Unpriced)
	}
	if tot.Tokens.CacheRead != 3_500_000 || tot.Tokens.CacheWrite != 200_000 || tot.Tokens.Input != 1_502_000 {
		t.Errorf("tokens = %+v", tot.Tokens)
	}
	// Reads saved their price below the input; the hour's writes cost more.
	wantSaved := (2_000_000*(5-0.5)-200_000*(10-5))/1e6 + 1_500_000*(1.25-0.125)/1e6
	if math.Abs(tot.Saved-wantSaved) > 1e-9 {
		t.Errorf("saved = %v, want %v", tot.Saved, wantSaved)
	}
	if tot.Latency != 2000 { // (2×1s + 2×3s) / 4 successful
		t.Errorf("latency = %d", tot.Latency)
	}
	// The day before holds yesterday's Sonnet.
	if day.Previous.Requests != 1 || math.Abs(day.Previous.Cost.Total()-18) > 1e-9 {
		t.Errorf("previous = %+v", day.Previous)
	}
	// The Opus requests of the last hour are one row of the last slot.
	var last *UsageRow
	for i, r := range day.Rows {
		if r.Slot == 23 && r.Model == "claude-opus-4-5" {
			last = &day.Rows[i]
		}
	}
	if last == nil || last.Requests != 2 || last.Account != "claude-a@example.com.json" {
		t.Fatalf("rows = %+v", day.Rows)
	}
	// The heat of Monday 20:00, and of 17:00.
	if cell := day.Heat[20]; cell.Requests != 2 {
		t.Errorf("Monday 20:00 = %+v", cell)
	}
	if cell := day.Heat[17]; cell.Requests != 2 || cell.Cost == 0 {
		t.Errorf("Monday 17:00 = %+v", cell)
	}
	// Facets, the costliest first: Claude, then Codex.
	if f := day.Facets.Providers; len(f) != 2 || f[0].ID != "claude" || f[1].ID != "codex" || f[1].Unpriced != 1_010_000 {
		t.Errorf("providers = %+v", f)
	}
	if len(day.Models) != 3 || day.Models[0].ID != "claude-opus-4-5" || day.Models[0].Price.Source != PriceList {
		t.Errorf("models = %+v", day.Models)
	}

	// A filter narrows everything but the facets.
	codexOnly, _ := l.Report(UsageQuery{Range: "24h", Location: moscow, Filter: UsageFilter{Provider: "codex"}}, price)
	if codexOnly.Totals.Requests != 2 || len(codexOnly.Facets.Providers) != 2 || codexOnly.Heat[20].Requests != 0 {
		t.Errorf("codex only = %+v", codexOnly.Totals)
	}
	model, _ := l.Report(UsageQuery{Range: "24h", Location: moscow, Filter: UsageFilter{Model: "gpt-6-sol"}}, price)
	if model.Totals.Requests != 1 || model.Totals.Cost.Total() != 0 || model.Totals.Unpriced == 0 {
		t.Errorf("gpt-6 only = %+v", model.Totals)
	}

	// A week in slots of four hours; a month and a quarter by the day.
	week, _ := l.Report(UsageQuery{Range: "7d", Location: moscow}, price)
	if len(week.Starts) != 42 || week.Totals.Requests != 7 || week.Previous.Requests != 1 {
		t.Errorf("week = %d slots, %d requests, previous %d", len(week.Starts), week.Totals.Requests, week.Previous.Requests)
	}
	month, _ := l.Report(UsageQuery{Range: "30d", Location: moscow}, price)
	if len(month.Starts) != 30 || month.Totals.Requests != 8 || month.Slot != 86400 {
		t.Errorf("month = %d slots, %d requests", len(month.Starts), month.Totals.Requests)
	}
	// Days start at midnight where the viewer is, today last.
	if first, lastDay := month.Starts[0].In(moscow), month.Starts[29].In(moscow); first.Hour() != 0 || lastDay.Day() != 28 || first.Day() != 30 {
		t.Errorf("days = %v … %v", first, lastDay)
	}
	if _, err := l.Report(UsageQuery{Range: "1y"}, price); err == nil {
		t.Error("an unknown range")
	}
	if !day.Since.Equal(now.Add(-8 * 24 * time.Hour).UTC()) {
		t.Errorf("since = %v", day.Since)
	}

	// What an account's requests came to, by its ID or its index.
	spent := l.spendByAuth(now.Add(-24*time.Hour), now, price)
	a := spent.of([]string{"claude-a@example.com.json"}, nil)
	b := spent.of(nil, []string{"ib"})
	if a.Requests != 3 || b.Requests != 2 || b.Unpriced != 1_010_000 || math.Abs(a.Cost+b.Cost-wantCost) > 1e-9 {
		t.Errorf("spend = %+v, %+v", a, b)
	}
	if none := spent.of(nil, nil); none.Requests != 0 {
		t.Errorf("of nothing = %+v", none)
	}
	if all := spent.total(); all.Requests != 5 {
		t.Errorf("total = %+v", all)
	}
}

func TestLedgerPersists(t *testing.T) {
	path := filepath.Join(t.TempDir(), "usage.json")
	now := time.Now()
	l := NewLedger(path)
	ctx := context.Background()
	record := usage.Record{AuthID: "a.json", AuthIndex: "ia", Provider: "claude", Model: "claude-opus-4-5", RequestedAt: now,
		Detail: usage.Detail{InputTokens: 10, CacheReadTokens: 20, CacheCreationTokens: 30, OutputTokens: 40}}
	l.HandleUsage(ctx, record)
	l.HandleUsage(ctx, record)
	old := record
	old.AuthID, old.AuthIndex, old.RequestedAt = "old.json", "iold", now.Add(-LedgerRetention-2*time.Hour)
	l.HandleUsage(ctx, old)
	l.learn("", "ia", ledgerAuth{Label: "someone@example.com", Name: "Claude", Kind: "account"})
	// Tokens used by a request that reached no account are kept too.
	l.HandleUsage(ctx, usage.Record{Provider: "claude", Model: "claude-opus-4-5", RequestedAt: now, Detail: usage.Detail{InputTokens: 5}})
	if err := l.Save(); err != nil {
		t.Fatal(err)
	}
	again := NewLedger(path)
	report, err := again.Report(UsageQuery{Range: "24h"}, NewPriceBook("").Resolve)
	if err != nil {
		t.Fatal(err)
	}
	if report.Totals.Requests != 3 || report.Totals.Tokens != (Tokens{Input: 25, CacheRead: 40, CacheWrite: 60, Output: 80}) {
		t.Errorf("totals = %+v", report.Totals)
	}
	if f := report.Facets.Accounts; len(f) != 2 || f[0].ID != "a.json" || f[1].ID != "" || f[1].Tokens != 5 {
		t.Errorf("accounts = %+v", f)
	}
	if _, kept := again.auths["old.json"]; kept {
		t.Error("what is older than the ledger keeps is dropped")
	}
	accounts := again.accountsOf([]string{"a.json", "claude-someone@example.org.json"})
	if accounts[0].Label != "someone@example.com" || accounts[0].ProviderName != "Claude" || accounts[0].Kind != "account" {
		t.Errorf("a = %+v", accounts[0])
	}
	if accounts[1].Label != "someone@example.org" {
		t.Errorf("a gone account = %+v", accounts[1])
	}
	if models := again.usedModels(); len(models) != 1 || models[0] != "claude-opus-4-5" {
		t.Errorf("used = %v", models)
	}
}
