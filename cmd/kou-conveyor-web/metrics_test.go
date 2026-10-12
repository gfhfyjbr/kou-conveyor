package main

import (
	"bytes"
	"encoding/json/v2"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestParseSince(t *testing.T) {
	now := time.Date(2026, 10, 9, 12, 0, 0, 0, time.UTC)
	want := time.Date(2026, 10, 8, 22, 16, 0, 0, time.UTC)
	for _, value := range []string{"2026-10-08T22:16", "2026-10-08T22:16:00Z", "2026-10-09T01:16+03:00", "2026-10-08 22:16", " 13h44m "} {
		if got, err := parseSince(value, now); err != nil || !got.Equal(want) {
			t.Errorf("parseSince(%q) = %v, %v; want %v", value, got, err, want)
		}
	}
	if got, err := parseSince("", now); err != nil || !got.IsZero() {
		t.Errorf(`parseSince("") = %v, %v`, got, err)
	}
	if _, err := parseSince("yesterday", now); err == nil {
		t.Error(`parseSince("yesterday") took it`)
	}
}

func TestMetricsCommand(t *testing.T) {
	path := filepath.Join("..", "..", "harness", "sessionstore", "localfile", "testdata", "golden-session.session.jsonl")
	var stdout, stderr bytes.Buffer
	if code := metricsMain([]string{"-json", path}, &stdout, &stderr); code != 0 {
		t.Fatalf("-json: exit %d: %s", code, stderr.String())
	}
	var measured []struct {
		File   string `json:"file"`
		Report struct {
			Session  string `json:"session"`
			Requests int    `json:"requests"`
		} `json:"report"`
	}
	if err := json.Unmarshal(stdout.Bytes(), &measured); err != nil {
		t.Fatalf("%v: %s", err, stdout.String())
	}
	if len(measured) != 1 || measured[0].File != path || measured[0].Report.Session != "golden-session" || measured[0].Report.Requests != 1 {
		t.Errorf("measured %+v", measured)
	}

	stdout.Reset()
	if code := metricsMain([]string{"-since", "2026-08-27T10:00", path}, &stdout, &stderr); code != 0 {
		t.Fatalf("markdown: exit %d: %s", code, stderr.String())
	}
	if out := stdout.String(); !strings.Contains(out, "## golden-session\n") || !strings.Contains(out, "| Requests | 1 |") {
		t.Errorf("markdown:\n%s", out)
	}

	stderr.Reset()
	if code := metricsMain([]string{filepath.Join(t.TempDir(), "none")}, &stdout, &stderr); code != 1 || !strings.Contains(stderr.String(), "no such session log") {
		t.Errorf("a missing log: exit %d: %s", code, stderr.String())
	}
	if code := metricsMain([]string{"-since", "yesterday", path}, &stdout, &stderr); code != 2 {
		t.Errorf("a bad -since: exit %d", code)
	}
	if code := metricsMain(nil, &stdout, &stderr); code != 2 {
		t.Errorf("no session: exit %d", code)
	}
}
