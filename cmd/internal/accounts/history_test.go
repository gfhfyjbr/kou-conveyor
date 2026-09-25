package accounts

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/usage"
)

func TestHistoryUptime(t *testing.T) {
	now := time.Date(2026, 9, 24, 12, 10, 0, 0, time.UTC)
	h := NewHistory("")
	h.now = func() time.Time { return now }
	ok := func(at time.Time, in, out int64) usage.Record {
		return usage.Record{AuthID: "codex-a.json", AuthIndex: "idx-a", Provider: "codex", RequestedAt: at,
			Latency: 2 * time.Second, Detail: usage.Detail{InputTokens: in, OutputTokens: out}}
	}
	failed := func(at time.Time, status int, body string) usage.Record {
		return usage.Record{AuthID: "codex-a.json", Provider: "codex", Model: "gpt-5", RequestedAt: at, Failed: true,
			Fail: usage.Failure{StatusCode: status, Body: body}}
	}
	ctx := context.Background()
	h.HandleUsage(ctx, ok(now.Add(-5*time.Minute), 100, 10))
	h.HandleUsage(ctx, ok(now.Add(-6*time.Minute), 50, 5))
	h.HandleUsage(ctx, failed(now.Add(-2*time.Hour), 429, `{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}`))
	h.HandleUsage(ctx, failed(now.Add(-time.Minute), 500, "upstream\n  exploded"))
	h.HandleUsage(ctx, ok(now.Add(-30*time.Hour), 1, 1)) // outside a day
	h.HandleUsage(ctx, usage.Record{Provider: "codex"})  // no account: not recorded

	day := h.Uptime("codex-a.json", "", 24*time.Hour, 48)
	if len(day.Slots) != 48 || day.Slot != 1800 {
		t.Fatalf("slots = %d of %ds", len(day.Slots), day.Slot)
	}
	if day.OK != 2 || day.Failed != 2 || day.Input != 150 || day.Output != 15 || day.Latency != 2000 {
		t.Errorf("day = %+v", day)
	}
	// The newest slot, 12:00–12:30, holds the requests of the last minutes.
	if last := day.Slots[47]; last.OK != 2 || last.Failed != 1 {
		t.Errorf("newest slot = %+v", last)
	}
	if slot := day.Slots[43]; slot.Failed != 1 { // 10:00–10:30
		t.Errorf("slot of the rate limit = %+v", slot)
	}
	if !day.From.Equal(time.Date(2026, 9, 23, 12, 30, 0, 0, time.UTC)) {
		t.Errorf("from = %v", day.From)
	}
	week := h.Uptime("", "idx-a", 7*24*time.Hour, 42)
	if week.OK != 3 || week.Slot != 4*3600 {
		t.Errorf("week, found by index = %+v", week)
	}

	errs := h.Errors("codex-a.json", "", 5)
	if len(errs) != 2 || errs[0].Status != 500 || errs[0].Message != "upstream exploded" {
		t.Fatalf("errors = %+v", errs)
	}
	if errs[1].Message != "rate_limit_error: Number of requests has exceeded your rate limit" || errs[1].Model != "gpt-5" {
		t.Errorf("rate limit = %+v", errs[1])
	}
	if got := h.Uptime("nobody", "", 24*time.Hour, 48); got.OK != 0 || len(got.Slots) != 48 {
		t.Errorf("unknown account = %+v", got)
	}
}

func TestHistoryPersists(t *testing.T) {
	path := filepath.Join(t.TempDir(), "history.json")
	now := time.Now()
	h := NewHistory(path)
	h.HandleUsage(context.Background(), usage.Record{AuthID: "a", RequestedAt: now})
	h.HandleUsage(context.Background(), usage.Record{AuthID: "old", RequestedAt: now.Add(-8 * 24 * time.Hour)})
	h.HandleUsage(context.Background(), usage.Record{AuthID: "b", RequestedAt: now, Failed: true, Fail: usage.Failure{StatusCode: 401}})
	if err := h.Save(); err != nil {
		t.Fatal(err)
	}
	again := NewHistory(path)
	if got := again.Uptime("a", "", 24*time.Hour, 48); got.OK != 1 {
		t.Errorf("a = %+v", got)
	}
	if got := again.Errors("b", "", 5); len(got) != 1 || got[0].Status != 401 {
		t.Errorf("b = %+v", got)
	}
	if _, kept := again.accounts["old"]; kept {
		t.Error("history older than a week is dropped")
	}
	again.Forget("a")
	if err := again.Save(); err != nil {
		t.Fatal(err)
	}
	if got := NewHistory(path).Uptime("a", "", 24*time.Hour, 48); got.OK != 0 {
		t.Error("a forgotten account stays forgotten")
	}
}

func TestErrorMessage(t *testing.T) {
	for body, want := range map[string]string{
		`{"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}`: "insufficient_quota: You exceeded your current quota",
		`{"error":{"code":429,"message":"Resource exhausted","status":"RESOURCE_EXHAUSTED"}}`: "RESOURCE_EXHAUSTED: Resource exhausted",
		`{"detail":"Unauthorized"}`: "Unauthorized",
		`{"error":"invalid_grant","error_description":"Refresh token revoked"}`: "Refresh token revoked",
		"plain\ttext": "plain text",
		"":            "",
	} {
		if got := errorMessage(body); got != want {
			t.Errorf("errorMessage(%q) = %q, want %q", body, got, want)
		}
	}
}
