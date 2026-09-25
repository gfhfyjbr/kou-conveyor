package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestHistoryRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nested", "history.json")
	want := appendHistory(nil, "  inspect   this project  ", "session-1", "model")
	if _, err := saveHistory(path, want); err != nil {
		t.Fatal(err)
	}
	got, err := loadHistory(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].Title != "inspect this project" || got[0].SessionID != "session-1" {
		t.Fatalf("history=%#v", got)
	}
}

func TestHistoryMergesConcurrentTerminals(t *testing.T) {
	path := filepath.Join(t.TempDir(), "history.json")
	first := appendHistory(nil, "from the first terminal", "s1", "")
	second := appendHistory(nil, "from the second terminal", "s2", "")
	if _, err := saveHistory(path, first); err != nil {
		t.Fatal(err)
	}
	// The second terminal loaded its history before the first one saved.
	merged, err := saveHistory(path, second)
	if err != nil {
		t.Fatal(err)
	}
	onDisk, _ := loadHistory(path)
	if len(merged) != 2 || len(onDisk) != 2 || onDisk[0].Prompt != "from the first terminal" {
		t.Fatalf("merged = %#v, on disk = %#v", merged, onDisk)
	}
	if again := appendHistory(merged, "from the second terminal", "s2", ""); len(again) != 2 {
		t.Fatalf("a repeated prompt was recorded twice: %#v", again)
	}
}

func TestHistoryFromDiskCannotEscapeTheTerminal(t *testing.T) {
	path := filepath.Join(t.TempDir(), "history.json")
	os.WriteFile(path, []byte(`[{"Prompt":"hi \u001b]52;c;cHduZWQ=\u0007 there","Title":"\u001b[2Jwiped"}]`), 0o600)
	history, err := loadHistory(path)
	if err != nil || len(history) != 1 {
		t.Fatalf("history = %+v, %v", history, err)
	}
	if strings.ContainsAny(history[0].Prompt+history[0].Title, "\x1b\x07") {
		t.Fatalf("escapes survived: %q %q", history[0].Prompt, history[0].Title)
	}
}
