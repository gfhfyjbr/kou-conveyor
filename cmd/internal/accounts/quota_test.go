package accounts

import (
	"encoding/json/v2"
	"testing"
	"time"
)

func decode(t *testing.T, text string) record {
	t.Helper()
	var m map[string]any
	if err := json.Unmarshal([]byte(text), &m); err != nil {
		t.Fatal(err)
	}
	return record(m)
}

func TestCodexQuota(t *testing.T) {
	now := time.Date(2026, 9, 24, 12, 0, 0, 0, time.UTC)
	q := codexQuota(decode(t, `{
		"plan_type": "pro",
		"rate_limit": {"allowed": true, "limit_reached": false,
			"primary_window": {"used_percent": 62.5, "limit_window_seconds": 18000, "reset_after_seconds": 3600},
			"secondary_window": {"used_percent": 12, "limit_window_seconds": 604800, "reset_at": 1790500000}},
		"code_review_rate_limit": {"primary_window": {"used_percent": 0, "limit_window_seconds": 604800, "reset_at": 1790500000}},
		"additional_rate_limits": [{"limit_name": "GPT-5.3-Codex-Spark", "rate_limit": {"limit_reached": true,
			"primary_window": {"limit_window_seconds": 18000, "reset_after_seconds": 60}}}],
		"credits": {"has_credits": true, "unlimited": false, "balance": "12.5"}
	}`), now)
	if q.Plan != "Pro" {
		t.Errorf("plan = %q", q.Plan)
	}
	want := []struct {
		label string
		used  float64
		reset time.Time
	}{
		{"5h", 62.5, now.Add(time.Hour)},
		{"Weekly", 12, time.Unix(1790500000, 0).UTC()},
		{"Code review · Weekly", 0, time.Unix(1790500000, 0).UTC()},
		{"GPT-5.3-Codex-Spark · 5h", 100, now.Add(time.Minute)},
	}
	if len(q.Windows) != len(want) {
		t.Fatalf("windows = %+v", q.Windows)
	}
	for i, w := range want {
		got := q.Windows[i]
		if got.Label != w.label || got.Used != w.used || !got.ResetsAt.Equal(w.reset) {
			t.Errorf("window %d = %+v, want %+v", i, got, w)
		}
	}
	if !q.Windows[3].Reached || q.Windows[0].Reached {
		t.Errorf("reached = %v, %v", q.Windows[3].Reached, q.Windows[0].Reached)
	}
	if len(q.Notes) != 1 || q.Notes[0] != "Credits: 12.5" {
		t.Errorf("notes = %v", q.Notes)
	}
	if monthly := codexQuota(decode(t, `{"rate_limit":{"secondary_window":{"used_percent":5,"limit_window_seconds":2592000}}}`), now); monthly.Windows[0].Label != "Monthly" {
		t.Errorf("a 30-day window is %q", monthly.Windows[0].Label)
	}
}

func TestClaudeQuota(t *testing.T) {
	q := claudeQuota(decode(t, `{
		"five_hour": {"utilization": 41.3, "resets_at": "2026-09-24T15:00:00.123456+00:00"},
		"seven_day": {"utilization": 8, "resets_at": "2026-09-29T10:00:00Z"},
		"seven_day_opus": null,
		"seven_day_sonnet": {"utilization": 2, "resets_at": null},
		"iguana_necktie": {"utilization": 99, "resets_at": "2026-09-29T10:00:00Z"},
		"limits": [{"kind": "weekly_scoped", "percent": 55, "resets_at": "2026-09-29T10:00:00Z", "is_active": true, "scope": {"model": {"display_name": "Fable"}}}],
		"extra_usage": {"is_enabled": true, "utilization": 3.5}
	}`))
	labels := []string{}
	for _, w := range q.Windows {
		labels = append(labels, w.Label)
	}
	want := []string{"5h", "Weekly", "Weekly · Sonnet", "Weekly · Fable", "Extra usage"}
	if len(labels) != len(want) {
		t.Fatalf("labels = %v, want %v", labels, want)
	}
	for i := range want {
		if labels[i] != want[i] {
			t.Fatalf("labels = %v, want %v", labels, want)
		}
	}
	if q.Windows[0].Used != 41.3 || q.Windows[0].ResetsAt.IsZero() {
		t.Errorf("5h = %+v", q.Windows[0])
	}
	if q.Windows[3].Used != 55 {
		t.Errorf("the scoped Fable limit wins over the legacy key: %+v", q.Windows[3])
	}
	for _, c := range []struct{ profile, plan string }{
		{`{"account":{"has_claude_max":true,"has_claude_pro":false}}`, "Max"},
		{`{"account":{"has_claude_max":false,"has_claude_pro":true}}`, "Pro"},
		{`{"account":{"has_claude_max":false,"has_claude_pro":false}}`, "Free"},
		{`{"organization":{"organization_type":"claude_team"},"account":{"has_claude_max":true}}`, "Team"},
		{`{}`, ""},
	} {
		if got := claudePlan(decode(t, c.profile)); got != c.plan {
			t.Errorf("claudePlan(%s) = %q, want %q", c.profile, got, c.plan)
		}
	}
}

func TestAntigravityQuota(t *testing.T) {
	q := antigravityQuota(decode(t, `{"groups": [
		{"displayName": "Gemini Pro", "buckets": [
			{"bucketId": "a", "window": "5h", "remainingFraction": 0.75, "resetTime": "2026-09-24T15:00:00Z"},
			{"bucketId": "b", "window": "weekly", "remaining_fraction": "0.2"}]},
		{"displayName": "Claude", "buckets": [{"displayName": "Daily", "remainingFraction": 1}]},
		{"displayName": "Empty", "buckets": [{"window": "5h"}]}
	]}`))
	if len(q.Windows) != 3 {
		t.Fatalf("windows = %+v", q.Windows)
	}
	if w := q.Windows[0]; w.Label != "Gemini Pro · 5h" || w.Used != 25 || w.ResetsAt.IsZero() {
		t.Errorf("first = %+v", w)
	}
	if w := q.Windows[1]; w.Label != "Gemini Pro · Weekly" || w.Used != 80 {
		t.Errorf("second = %+v", w)
	}
	if w := q.Windows[2]; w.Label != "Claude · Daily" || w.Used != 0 {
		t.Errorf("third = %+v", w)
	}
}

func TestKimiQuota(t *testing.T) {
	now := time.Date(2026, 9, 24, 12, 0, 0, 0, time.UTC)
	q := kimiQuota(decode(t, `{
		"usage": {"limit": "2048", "used": "512", "resetTime": "2026-09-28T00:00:00Z"},
		"limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"}, "detail": {"limit": 100, "remaining": 40, "reset_in": 600}}]
	}`), now)
	if len(q.Windows) != 2 {
		t.Fatalf("windows = %+v", q.Windows)
	}
	if w := q.Windows[0]; w.Label != "5h" || w.Used != 60 || w.Amount != "60 / 100" || !w.ResetsAt.Equal(now.Add(10*time.Minute)) {
		t.Errorf("5h = %+v", w)
	}
	if w := q.Windows[1]; w.Label != "Weekly" || w.Used != 25 || w.Amount != "512 / 2048" {
		t.Errorf("weekly = %+v", w)
	}
}

func TestXAIQuota(t *testing.T) {
	weekly := decode(t, `{"config": {"creditUsagePercent": 37.5, "currentPeriod": {"type": "BILLING_PERIOD_TYPE_WEEKLY", "start": "2026-09-21T00:00:00Z", "end": "2026-09-28T00:00:00Z"},
		"productUsage": [{"product": "Grok Build", "usagePercent": 20}]}}`).obj("config")
	monthly := decode(t, `{"config": {"monthlyLimit": {"val": 5000}, "used": {"val": 1250}, "billingPeriodEnd": "2026-10-01T00:00:00Z"}}`).obj("config")
	q := xaiQuota(weekly, monthly)
	if len(q.Windows) != 3 {
		t.Fatalf("windows = %+v", q.Windows)
	}
	if w := q.Windows[0]; w.Label != "Weekly credits" || w.Used != 37.5 || w.ResetsAt.IsZero() {
		t.Errorf("credits = %+v", w)
	}
	if w := q.Windows[1]; w.Label != "Grok Build" || w.Used != 20 {
		t.Errorf("product = %+v", w)
	}
	if w := q.Windows[2]; w.Label != "Monthly" || w.Used != 25 || w.Amount != "$12.50 / $50.00" {
		t.Errorf("monthly = %+v", w)
	}
}

func TestObservedQuota(t *testing.T) {
	at := "2026-09-24T12:00:00Z"
	codex, ok := observedQuota("codex", decode(t, `{"observed_at": "`+at+`", "signals": {
		"X-Codex-Plan-Type": "plus",
		"X-Codex-Primary-Used-Percent": "40", "X-Codex-Primary-Window-Minutes": "300", "X-Codex-Primary-Reset-After-Seconds": "1800",
		"X-Codex-Secondary-Used-Percent": "10", "X-Codex-Secondary-Window-Minutes": "10080", "X-Codex-Secondary-Reset-At": "1790500000"}}`))
	if !ok || codex.Source != SourceObserved || codex.Plan != "Plus" || len(codex.Windows) != 2 {
		t.Fatalf("codex = %+v", codex)
	}
	if w := codex.Windows[0]; w.Label != "5h" || w.Used != 40 || !w.ResetsAt.Equal(time.Date(2026, 9, 24, 12, 30, 0, 0, time.UTC)) {
		t.Errorf("primary = %+v", w)
	}
	if w := codex.Windows[1]; w.Label != "Weekly" || !w.ResetsAt.Equal(time.Unix(1790500000, 0)) {
		t.Errorf("secondary = %+v", w)
	}
	claude, ok := observedQuota("claude", decode(t, `{"observed_at": "`+at+`", "signals": {
		"Anthropic-Ratelimit-Unified-5h-Utilization": "0.93", "Anthropic-Ratelimit-Unified-5h-Reset": "1790200000",
		"Anthropic-Ratelimit-Unified-5h-Status": "rejected", "Anthropic-Ratelimit-Unified-7d-Utilization": "0.1"}}`))
	if !ok || len(claude.Windows) != 2 || claude.Windows[0].Used != 93 || !claude.Windows[0].Reached || claude.Windows[1].Label != "Weekly" {
		t.Fatalf("claude = %+v", claude)
	}
	if _, ok := observedQuota("codex", decode(t, `{"signals": {}}`)); ok {
		t.Error("no signals, no quota")
	}
	if _, ok := observedQuota("kimi", decode(t, `{"signals": {"X-Codex-Primary-Used-Percent": "1"}}`)); ok {
		t.Error("only Codex and Claude report quotas in their headers")
	}
}

func TestLabels(t *testing.T) {
	for seconds, want := range map[float64]string{18000: "5h", 604800: "Weekly", 86400: "Daily", 2592000: "Monthly", 172800: "2d", 1800: "30m", 0: "Limit"} {
		if got := windowLabel(seconds); got != want {
			t.Errorf("windowLabel(%v) = %q, want %q", seconds, got, want)
		}
	}
	for plan, want := range map[string]string{"plus": "Plus", "plan_max": "Max", "prolite": "Pro Lite", "self-serve-business": "Business", "team_plan": "Team Plan", "": ""} {
		if got := planName(plan); got != want {
			t.Errorf("planName(%q) = %q, want %q", plan, got, want)
		}
	}
}
