package cockpit_test

import (
	"errors"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func startCompaction(t *testing.T, sessions, id, instructions string) (*cockpit.Job, error) {
	t.Helper()
	return cockpit.Start(t.Context(), cockpit.Options{
		Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions, Heartbeat: time.Minute,
	}, cockpit.Request{SessionID: id, Compact: true, Instructions: instructions, Resume: true})
}

func TestCompactionRunSummarizesSession(t *testing.T) {
	sessions := t.TempDir()
	tr := cockpit.NewTranscript()
	if tr.Compactable() {
		t.Fatal("an empty session is compactable")
	}
	drain(t, start(t, t.Context(), sessions, "session-1", "first question"), tr)
	if !tr.Compactable() || tr.Usage.Context != 120 {
		t.Fatalf("compactable %v, context %d after an answer", tr.Compactable(), tr.Usage.Context)
	}

	job, err := startCompaction(t, sessions, "session-1", "the release")
	if err != nil {
		t.Fatal(err)
	}
	tr.Begin(uuid.New().String(), "Compacting context")
	drain(t, job, tr)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	tr.Finish(nil, false, time.Now())
	last := tr.Entries[len(tr.Entries)-1]
	if last.Kind != cockpit.KindNotice || last.Text != "Context compacted from 152k tokens" ||
		last.Detail != "Summary of the session, focused on the release" {
		t.Fatalf("last entry = %#v", last)
	}
	// The context is the summary's size until the next turn reports it.
	if tr.Compactable() || tr.Usage.Context != 900 || tr.Interrupted() {
		t.Fatalf("compactable %v, context %d, interrupted %v after compacting", tr.Compactable(), tr.Usage.Context, tr.Interrupted())
	}
	persisted, err := cockpit.LoadSession(sessions, "session-1")
	if err != nil || persisted.Compactable() || persisted.Entries[len(persisted.Entries)-1].Text != last.Text {
		t.Fatalf("persisted = %#v, %v", persisted, err)
	}
	doc := cockpit.ExportMarkdown("Session", "session-1", persisted, time.Now())
	if !strings.Contains(doc, "> Context compacted from 152k tokens\n\n<details><summary>Summary</summary>\n\nSummary of the session, focused on the release\n\n</details>") {
		t.Fatalf("export:\n%s", doc)
	}

	drain(t, start(t, t.Context(), sessions, "session-1", "second question"), tr)
	if !tr.Compactable() {
		t.Fatal("an answer after the compaction is not compactable")
	}
}

func TestCompactionWithoutSummaryIsReported(t *testing.T) {
	sessions := t.TempDir()
	tr := cockpit.NewTranscript()
	drain(t, start(t, t.Context(), sessions, "session-1", "first question"), tr)
	job, err := startCompaction(t, sessions, "session-1", "nothing")
	if err != nil {
		t.Fatal(err)
	}
	drain(t, job, tr)
	last := tr.Entries[len(tr.Entries)-1]
	if last.Text != "Context not compacted: the model wrote no summary" || last.Detail != "" || !tr.Compactable() {
		t.Fatalf("last entry = %#v, compactable %v", last, tr.Compactable())
	}
}

func TestCompactionRequestsAreChecked(t *testing.T) {
	sessions := t.TempDir()
	if _, err := startCompaction(t, sessions, "missing", ""); !errors.Is(err, cockpit.ErrSessionGone) {
		t.Fatalf("compacting a missing session: %v", err)
	}
	drain(t, start(t, t.Context(), sessions, "session-1", "first question"), cockpit.NewTranscript())
	_, err := cockpit.Start(t.Context(), cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions},
		cockpit.Request{SessionID: "session-1", Compact: true, Prompt: "and a prompt"})
	if err == nil {
		t.Fatal("a compaction ran a prompt")
	}
	if _, err := cockpit.Start(t.Context(), cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions},
		cockpit.Request{SessionID: "session-1", Compact: true, MessageID: "not-a-uuid"}); err == nil {
		t.Fatal("accepted a malformed message ID")
	}
}
