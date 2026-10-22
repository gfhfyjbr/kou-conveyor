package cockpit_test

import (
	"encoding/json/v2"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func readOr(t *testing.T, path, none string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return none
	}
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func appendTo(t *testing.T, path, text string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if _, err := file.WriteString(text); err != nil {
		t.Fatal(err)
	}
}

// changeRecord is a line of a session's change records as the cockpits write
// them (cockpit.Changes).
func changeRecord(t *testing.T, message string, before bool) string {
	t.Helper()
	data, err := json.Marshal(map[string]any{"message": message, "tree": "tree-" + message, "before": before, "at": time.Now().UTC()})
	if err != nil {
		t.Fatal(err)
	}
	return string(data) + "\n"
}

// What the runs of a session wrote beside its log goes back with the log when
// a prompt is edited: the rewound run must not read its discarded future in
// the notes the harness shows it, in the summaries of the compactions, or in
// the snapshot of the workspace it is told its run started from.
func TestRewindTakesTheMemoryBack(t *testing.T) {
	dir := t.TempDir()
	const id = "remembered"
	notes := filepath.Join(dir, id+".notes.md")
	summaries := filepath.Join(dir, id+".summaries.md")
	changes := filepath.Join(dir, ".changes", id+".jsonl")
	const (
		first     = "## Compaction at 2026-10-20T10:00:00Z\n\nwhat the first run did\n\n"
		discarded = "## Compaction at 2026-10-20T11:00:00Z\n\nwhat the edited prompt's run did\n\n"
	)
	// Each run writes what the model did: notes, the summary of a
	// compaction, and the snapshots of the workspace the cockpit took.
	var ids []string
	for i, run := range []struct{ notes, summary string }{
		{"notes of the first run\n", first},
		{"notes of the discarded future\n", discarded},
		{"notes of the run after it\n", ""},
	} {
		ids = append(ids, session(t, dir, id, []string{"first question", "second question", "third question"}[i])...)
		if err := os.WriteFile(notes, []byte(run.notes), 0o600); err != nil {
			t.Fatal(err)
		}
		if run.summary != "" {
			appendTo(t, summaries, run.summary)
		}
		appendTo(t, changes, changeRecord(t, ids[i], true)+changeRecord(t, ids[i], false))
	}

	// The cockpit takes the snapshot the edited prompt's run starts from,
	// then starts it.
	next := uuid.New().String()
	appendTo(t, changes, changeRecord(t, next, true))
	o := cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: dir, Heartbeat: time.Minute}
	job, err := cockpit.Start(t.Context(), o, cockpit.Request{
		SessionID: id, MessageID: next, Prompt: "second question, edited", Rewind: ids[1],
	})
	if err != nil {
		t.Fatal(err)
	}
	drain(t, job, cockpit.NewTranscript())
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	if got := prompts(t, dir, id); strings.Join(got, "|") != "first question|echo: first question|second question, edited|echo: second question, edited" {
		t.Fatalf("history = %q", got)
	}
	if got := readOr(t, notes, "(none)"); got != "notes of the first run\n" {
		t.Errorf("notes after the rewind = %q", got)
	}
	if got := readOr(t, summaries, "(none)"); got != first {
		t.Errorf("summaries after the rewind = %q", got)
	}
	want := []string{"tree-" + ids[0], "tree-" + ids[0], "tree-" + next}
	var trees []string
	for line := range strings.Lines(readOr(t, changes, "")) {
		var record struct {
			Tree string `json:"tree"`
		}
		if err := json.Unmarshal([]byte(line), &record); err != nil {
			t.Fatalf("change record %q: %v", line, err)
		}
		trees = append(trees, record.Tree)
	}
	if strings.Join(trees, " ") != strings.Join(want, " ") {
		t.Errorf("change records after the rewind = %q, want %q", trees, want)
	}

	// The edited prompt's run recorded what it started from in turn: editing
	// it again goes back to the same notes.
	if err := os.WriteFile(notes, []byte("notes of the edited prompt's run\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.RewindSession(dir, id, next); err != nil {
		t.Fatal(err)
	}
	if got := readOr(t, notes, "(none)"); got != "notes of the first run\n" {
		t.Errorf("notes after rewinding the edited prompt = %q", got)
	}
	// Before the first prompt the session had no memory at all.
	if err := cockpit.RewindSession(dir, id, ids[0]); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{notes, summaries, changes, filepath.Join(dir, id+".memory.jsonl")} {
		if got := readOr(t, path, "(none)"); got != "(none)" {
			t.Errorf("%s after rewinding to the start = %q", filepath.Base(path), got)
		}
	}

	// A deleted session leaves no memory behind.
	session(t, dir, id, "a new first question")
	if _, err := os.Stat(filepath.Join(dir, id+".memory.jsonl")); err != nil {
		t.Fatalf("the run recorded no memory: %v", err)
	}
	if err := cockpit.DeleteSession(dir, id); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, id+".memory.jsonl")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("memory of a deleted session: %v", err)
	}
	if list, _ := cockpit.ListSessions(dir); len(list) != 0 {
		t.Fatalf("sessions = %+v", list)
	}
}
