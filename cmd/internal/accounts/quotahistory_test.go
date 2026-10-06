package accounts

import (
	"bufio"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"
)

func TestQuotaHistory(t *testing.T) {
	path := filepath.Join(t.TempDir(), "quota-history.jsonl")
	start := time.Date(2026, 10, 3, 12, 0, 0, 0, time.UTC)
	history := newQuotaHistory(path, start)
	records := []record{
		decode(t, `{"name":"claude-a.json","id":"claude-a.json","auth_index":"1","provider":"claude","status":"active"}`),
		decode(t, `{"name":"codex-b.json","id":"codex-b.json","auth_index":"2","provider":"codex","status":"active"}`),
		decode(t, `{"name":"claude-off.json","id":"claude-off.json","auth_index":"3","provider":"claude","disabled":true}`),
		decode(t, `{"name":"claude:apikey:1","id":"claude:apikey:1","auth_index":"4","provider":"claude","account_type":"api_key"}`),
		decode(t, `{"name":"claude-c.json","id":"claude-c.json","auth_index":"5","provider":"claude","status":"active",
			"quota":{"observed_at":"2026-10-03T12:10:00Z","signals":{"Anthropic-Ratelimit-Unified-5h-Utilization":"0.5"}}}`),
	}
	used := map[string]time.Time{
		"claude-a.json":   start.Add(5 * time.Minute),
		"codex-b.json":    start.Add(-time.Hour), // before the history started
		"claude-off.json": start.Add(5 * time.Minute),
		"claude:apikey:1": start.Add(5 * time.Minute),
		"claude-c.json":   start.Add(10 * time.Minute),
	}
	var asked []string
	live := func(a Account) Quota {
		asked = append(asked, a.Name)
		if a.Name == "claude-c.json" {
			return Quota{Source: SourceLive, Error: "429: rate limited"}
		}
		return Quota{Source: SourceLive, Windows: []Window{{Label: "5h", Used: 42}}}
	}
	record := func(at time.Time) {
		t.Helper()
		asked = nil
		if err := history.record(t.Context(), at, records, func(id, _ string) time.Time { return used[id] }, live); err != nil {
			t.Fatal(err)
		}
	}
	read := func(path string) []QuotaSnapshot {
		t.Helper()
		file, err := os.Open(path)
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		var snapshots []QuotaSnapshot
		for scanner := bufio.NewScanner(file); scanner.Scan(); {
			var snapshot QuotaSnapshot
			if err := json.Unmarshal(scanner.Bytes(), &snapshot); err != nil {
				t.Fatal(err)
			}
			snapshots = append(snapshots, snapshot)
		}
		return snapshots
	}

	// The accounts used since the history started are written down; a
	// disabled account and a key, which reports no quota, are not.
	record(start.Add(15 * time.Minute))
	if !slices.Equal(asked, []string{"claude-a.json", "claude-c.json"}) {
		t.Fatalf("asked %v", asked)
	}
	snapshots := read(path)
	if len(snapshots) != 2 || snapshots[0].Account != "claude-a.json" || snapshots[0].Provider != "claude" ||
		!snapshots[0].At.Equal(start.Add(15*time.Minute)) || len(snapshots[0].Quota.Windows) != 1 || snapshots[0].Quota.Windows[0].Used != 42 {
		t.Fatalf("snapshots = %+v", snapshots)
	}
	// The provider could not be asked: the headers of the latest response tell.
	if c := snapshots[1].Quota; c.Source != SourceObserved || c.Error != "" || len(c.Windows) != 1 || c.Windows[0].Used != 50 {
		t.Errorf("observed = %+v", c)
	}

	// Accounts that served no request since are not asked again.
	record(start.Add(30 * time.Minute))
	if len(asked) != 0 || len(read(path)) != 2 {
		t.Fatalf("idle accounts were asked: %v", asked)
	}
	used["codex-b.json"] = start.Add(40 * time.Minute)
	record(start.Add(45 * time.Minute))
	if !slices.Equal(asked, []string{"codex-b.json"}) || len(read(path)) != 3 {
		t.Fatalf("asked %v", asked)
	}

	// A file past the bound becomes the previous one.
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	history.limit = info.Size() + 10
	used["claude-a.json"] = start.Add(50 * time.Minute)
	record(start.Add(time.Hour))
	if previous := read(filepath.Join(filepath.Dir(path), "quota-history.1.jsonl")); len(previous) != 3 {
		t.Errorf("the previous file has %d snapshots", len(previous))
	}
	if latest := read(path); len(latest) != 1 || latest[0].Account != "claude-a.json" {
		t.Errorf("the new file has %+v", latest)
	}
}

func TestHistoryLastUsed(t *testing.T) {
	h := NewHistory("")
	if !h.LastUsed("a", "1").IsZero() {
		t.Fatal("an account without requests was used")
	}
	ok, failed := time.Date(2026, 10, 3, 12, 0, 0, 0, time.UTC), time.Date(2026, 10, 3, 12, 5, 0, 0, time.UTC)
	h.accounts["a"] = &accountHistory{Index: "1", LastOK: ok, LastFail: failed}
	if got := h.LastUsed("a", ""); !got.Equal(failed) {
		t.Errorf("by ID: %v", got)
	}
	if got := h.LastUsed("other", "1"); !got.Equal(failed) {
		t.Errorf("by index: %v", got)
	}
}
