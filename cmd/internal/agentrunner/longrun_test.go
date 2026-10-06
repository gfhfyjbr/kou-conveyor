package agentrunner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/coordinator"
)

func TestReadRunBounds(t *testing.T) {
	read := func(env map[string]string) (runBounds, error) {
		return readRunBounds(func(name string) string { return env[name] })
	}
	if bounds, err := read(nil); err != nil || bounds != (runBounds{reportEvery: defaultReportEvery}) {
		t.Fatalf("defaults = %+v, %v", bounds, err)
	}
	bounds, err := read(map[string]string{
		maxTurnsEnvironment: " 200 ", maxCompactionsEnvironment: "6", maxDurationEnvironment: "8h", reportEveryEnvironment: "off",
	})
	if err != nil || bounds != (runBounds{maxTurns: 200, maxCompactions: 6, maxDuration: 8 * time.Hour}) {
		t.Fatalf("bounds = %+v, %v", bounds, err)
	}
	for name, value := range map[string]string{
		maxTurnsEnvironment: "-1", maxCompactionsEnvironment: "many", maxDurationEnvironment: "8", reportEveryEnvironment: "-5m",
	} {
		if _, err := read(map[string]string{name: value}); err == nil || !strings.Contains(err.Error(), name) {
			t.Errorf("%s=%s: %v", name, value, err)
		}
	}
}

func TestSessionMemoryKeepsSummariesAndCheckpoints(t *testing.T) {
	directory := t.TempDir()
	memory := newSessionMemory(directory, "abc", true)
	if notes := memory.builder(); notes.Notes != filepath.Join(directory, "abc.notes.md") || notes.Summaries != filepath.Join(directory, "abc.summaries.md") {
		t.Fatalf("memory = %+v", notes)
	}
	if text, err := memory.builder().ReadNotes(); text != "" || err != nil {
		t.Fatalf("missing notes read as %q, %v", text, err)
	}
	now := time.Date(2026, 10, 4, 12, 0, 0, 0, time.UTC)

	// Without the cockpit's records, a compaction is only kept.
	note, err := memory.compacted(coordinator.Compaction{Summary: " First. ", Number: 1, FileChanges: 2}, now)
	if err != nil || note != "" {
		t.Fatalf("note %q, %v", note, err)
	}

	// With them, it says what changed since.
	if err := os.MkdirAll(filepath.Join(directory, ".changes"), 0o700); err != nil {
		t.Fatal(err)
	}
	records := `{"message":"m1","tree":"aaa","before":true,"at":"2026-10-04T11:00:00Z"}
{"message":"m1","tree":"bbb","changed":["x.go"],"at":"2026-10-04T11:30:00Z"}
{"broken`
	if err := os.WriteFile(filepath.Join(directory, ".changes", "abc.jsonl"), []byte(records), 0o600); err != nil {
		t.Fatal(err)
	}
	repository := filepath.Join(directory, ".changes", "repo.git")
	note, err = memory.compacted(coordinator.Compaction{Summary: "Second.", Number: 2, FileChanges: 1}, now.Add(time.Hour))
	if err != nil || !strings.Contains(note, "snapshot bbb in the git repository "+repository+": `git --git-dir "+repository+" diff --stat aaa bbb` shows what changed since the prompt's run began") {
		t.Fatalf("note %q, %v", note, err)
	}
	kept, err := os.ReadFile(filepath.Join(directory, "abc.summaries.md"))
	if err != nil || string(kept) != "## Compaction at 2026-10-04T12:00:00Z\n\nFirst.\n\n## Compaction at 2026-10-04T13:00:00Z, workspace snapshot bbb\n\nSecond.\n\n" {
		t.Fatalf("summaries %q, %v", kept, err)
	}

	// The workspace stays as it was: the third compaction in a row without a
	// change asks whether the work goes round in circles, and the count
	// starts over.
	for number := 3; number <= 5; number++ {
		note, err = memory.compacted(coordinator.Compaction{Summary: "Same.", Number: number}, now)
		if stuck := strings.Contains(note, "3 compactions in a row came with no file changed."); err != nil || stuck != (number == 5) || strings.Contains(note, "diff --stat") {
			t.Fatalf("compaction %d: note %q, %v", number, note, err)
		}
	}
}

func TestSessionMemoryWithoutFileToolsOrRecordsCannotTellProgress(t *testing.T) {
	memory := newSessionMemory(t.TempDir(), "abc", false)
	for number := 1; number <= 4; number++ {
		if note, err := memory.compacted(coordinator.Compaction{Summary: "Same.", Number: number}, time.Now()); note != "" || err != nil {
			t.Fatalf("compaction %d: note %q, %v", number, note, err)
		}
	}
}
