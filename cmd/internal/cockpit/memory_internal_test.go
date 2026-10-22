package cockpit

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// memoryLog writes a session log the way the runs of the runner do.
type memoryLog struct {
	t      *testing.T
	store  *localfile.Store
	id     session.ID
	notes  string // the path of the session's notes
	inputs int
	turns  int
	turn   session.TurnID
}

func newMemoryLog(t *testing.T, dir string, id session.ID) *memoryLog {
	t.Helper()
	store, err := localfile.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Create(t.Context(), id); err != nil {
		t.Fatal(err)
	}
	return &memoryLog{t: t, store: store, id: id, notes: notesPath(dir, string(id))}
}

func (l *memoryLog) must(err error) {
	l.t.Helper()
	if err != nil {
		l.t.Fatal(err)
	}
}

func (l *memoryLog) control(message inbox.ControlMessage) {
	l.t.Helper()
	l.inputs++
	payload, err := json.Marshal(message)
	l.must(err)
	l.must(l.store.AppendInput(l.t.Context(), l.id, inbox.Input{
		ID: inbox.ID(fmt.Sprintf("control-%d", l.inputs)), Kind: inbox.InputControl, Payload: payload,
	}))
}

// prompt starts the run of a prompt.
func (l *memoryLog) prompt(messageID string) {
	l.t.Helper()
	l.control(inbox.ControlMessage{Mode: inbox.UpdateSettings, Parameters: inbox.Settings{ReasoningEffort: llm.ReasoningEffortHigh}})
	payload, err := json.Marshal("the prompt " + messageID)
	l.must(err)
	l.must(l.store.AppendInput(l.t.Context(), l.id, inbox.Input{ID: inbox.ID(messageID), Kind: inbox.InputExternal, Payload: payload}))
}

func (l *memoryLog) respond(kind session.TurnType, compacted *sessionstore.Compacted, output ...llm.Item) {
	l.t.Helper()
	l.turns++
	turn := session.TurnID(fmt.Sprintf("turn-%d", l.turns))
	l.must(l.store.AppendTurn(l.t.Context(), l.id, session.Turn{ID: turn, PreviousTurnID: l.turn, Type: kind}))
	l.turn = turn
	l.must(l.store.AppendModelResponse(l.t.Context(), l.id, sessionstore.ModelResponse{
		TurnID: turn, Response: llm.Response{Stop: llm.StopComplete, Output: output}, Compacted: compacted,
	}))
}

// say is a turn that calls no tool.
func (l *memoryLog) say() {
	l.t.Helper()
	l.respond(session.TurnRegular, nil)
}

// write is a turn in which the model writes its notes.
func (l *memoryLog) write(notes string) {
	l.t.Helper()
	arguments, err := json.Marshal(map[string]string{"file_path": l.notes, "content": notes})
	l.must(err)
	l.respond(session.TurnRegular, nil, llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{
		CallID: fmt.Sprintf("call-%d", l.turns+1), Name: "Write", Arguments: string(arguments),
	}})
}

// compact is a compaction that shows the notes as given.
func (l *memoryLog) compact(shown string) {
	l.t.Helper()
	l.respond(session.TurnCompaction, &sessionstore.Compacted{Message: "the summary", Notes: &shown})
}

// changed is the harness telling the model its notes changed, as a run's
// first turn does (contextbuilder.NotesUpdate).
func (l *memoryLog) changed(shown string) {
	l.t.Helper()
	reason := contextbuilder.NotesUpdatePrefix + " The file " + l.notes + " is empty now."
	if shown != "" {
		reason = contextbuilder.NotesUpdatePrefix + " They are in " + l.notes + ", and read now:\n<notes>\n" + shown + "\n</notes>"
	}
	l.control(inbox.ControlMessage{Mode: inbox.Heartbeat, Reason: reason})
}

// A session made before memory snapshots were kept, or a prompt sent into
// the run of another, has no snapshot to go back to: the rewind restores the
// notes and the summaries as far as the log that remains tells.
func TestRewindRestoresTheMemoryTheLogTellsOf(t *testing.T) {
	long := strings.Repeat("a line of the notes\n", contextbuilder.NotesLimit/10)
	const (
		kept      = "## Compaction at 2020-01-02T03:04:05Z, workspace snapshot 0123abcd\n\nwhat the session did\n\n"
		discarded = "## Compaction at 2100-01-02T03:04:05Z\n\nwhat its discarded future did\n\n"
	)
	for name, test := range map[string]struct {
		// started are the notes the first run started from, in a snapshot;
		// "" for none.
		started string
		first   func(l *memoryLog) // the run that remains
		edited  func(l *memoryLog) // the run of the edited prompt
		notes   string             // the notes before the rewind
		want    string             // and after it; "" for no file
	}{
		"as a compaction showed them": {
			first:  func(l *memoryLog) { l.write("first notes"); l.compact("first notes") },
			edited: func(l *memoryLog) { l.write("discarded notes") },
			notes:  "discarded notes\n", want: "first notes\n",
		},
		"as the harness said they changed": {
			first:  func(l *memoryLog) { l.write("first notes"); l.changed("first notes"); l.say() },
			edited: func(l *memoryLog) { l.compact("first notes"); l.write("discarded notes") },
			notes:  "discarded notes\n", want: "first notes\n",
		},
		"as the harness said they were emptied": {
			first:  func(l *memoryLog) { l.write("first notes"); l.changed("") },
			edited: func(l *memoryLog) { l.write("discarded notes") },
			notes:  "discarded notes\n", want: "",
		},
		"none when nothing that remains wrote them": {
			first:  func(l *memoryLog) { l.say() },
			edited: func(l *memoryLog) { l.write("discarded notes") },
			notes:  "discarded notes\n", want: "",
		},
		// Rather lost than read by the rewound run.
		"none when what remains does not tell them": {
			first:  func(l *memoryLog) { l.write("first notes") },
			edited: func(l *memoryLog) { l.write("discarded notes") },
			notes:  "discarded notes\n", want: "",
		},
		"as they are when what is cut off did not write them": {
			first:  func(l *memoryLog) { l.write("first notes") },
			edited: func(l *memoryLog) { l.say(); l.compact("first notes, and more") },
			notes:  "first notes, and more\n", want: "first notes, and more\n",
		},
		// What shows only the beginning and end of notes too long to show
		// whole tells nothing.
		"as a run that remains started from them": {
			started: "the notes the first run started from\n",
			first:   func(l *memoryLog) { l.write(long); l.compact(contextbuilder.ShownNotes(long, l.notes)) },
			edited:  func(l *memoryLog) { l.write("discarded notes") },
			notes:   "discarded notes\n", want: "the notes the first run started from\n",
		},
	} {
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			const id = "remembered"
			l := newMemoryLog(t, dir, id)
			if test.started != "" {
				// As Start takes it before the run.
				l.must(os.WriteFile(notesPath(dir, id), []byte(test.started), 0o600))
				l.must(snapshotMemory(dir, id, "prompt-1"))
			}
			l.prompt("prompt-1")
			test.first(l)
			l.prompt("prompt-2")
			test.edited(l)
			l.prompt("prompt-3")
			l.say()
			l.must(os.WriteFile(notesPath(dir, id), []byte(test.notes), 0o600))
			l.must(os.WriteFile(summariesPath(dir, id), []byte(kept+discarded), 0o600))

			l.must(rewindSession(dir, id, "prompt-2", ""))
			notes, err := os.ReadFile(notesPath(dir, id))
			if errors.Is(err, fs.ErrNotExist) {
				err = nil
			}
			l.must(err)
			if string(notes) != test.want {
				t.Errorf("notes after the rewind = %q, want %q", notes, test.want)
			}
			if summaries, _ := os.ReadFile(summariesPath(dir, id)); string(summaries) != kept {
				t.Errorf("summaries after the rewind = %q", summaries)
			}
			snaps, err := readMemorySnapshots(dir, id)
			l.must(err)
			if test.started != "" && (len(snaps) != 1 || snaps[0].Message != "prompt-1") {
				t.Errorf("snapshots after the rewind = %+v", snaps)
			}
		})
	}
}

func TestCutSummaries(t *testing.T) {
	sections := []string{
		"## Compaction at 2026-10-20T11:59:59Z\n\nbefore the cut\n\n",
		// The headers tell the time to the second: of those in the second of
		// the cut, as many came after it as compactions were cut off. A
		// header in a summary is no section's.
		"## Compaction at 2026-10-20T12:00:00Z, workspace snapshot 0123abcd\n\nin its second\n## Compaction at 2026-10-20T12:00:05Z\n\n",
		"## Compaction at 2026-10-20T12:00:01Z\n\nafter it\n\n",
		"## Compaction at 2026-10-20T13:00:00Z\n\nlater\n\n",
	}
	path := filepath.Join(t.TempDir(), "summaries.md")
	cut := time.Date(2026, 10, 20, 12, 0, 0, 500_000_000, time.UTC)
	for _, test := range []struct {
		at          time.Time
		compactions int // cut off
		want        int // how many sections remain
	}{
		{time.Time{}, 0, 4},
		{cut, 0, 2},
		{cut, 2, 2},
		{cut, 3, 1},
		// What the clock tells stays.
		{cut, 4, 1},
		{time.Date(2026, 10, 20, 11, 59, 58, 0, time.UTC), 0, 0},
		{time.Date(2027, 1, 1, 0, 0, 0, 0, time.UTC), 1, 4},
	} {
		if err := os.WriteFile(path, []byte(strings.Join(sections, "")), 0o600); err != nil {
			t.Fatal(err)
		}
		if err := cutSummaries(path, test.at, test.compactions); err != nil {
			t.Fatal(err)
		}
		got, err := os.ReadFile(path)
		if errors.Is(err, fs.ErrNotExist) {
			err = nil
		}
		if err != nil {
			t.Fatal(err)
		}
		if want := strings.Join(sections[:test.want], ""); string(got) != want {
			t.Errorf("cut at %v after %d compactions: %q, want %q", test.at, test.compactions, got, want)
		}
	}
	if err := cutSummaries(filepath.Join(t.TempDir(), "none.md"), time.Now(), 1); err != nil {
		t.Fatalf("no summaries: %v", err)
	}
}

// The cockpits take the snapshot a run starts from before they start it, so
// before the rewind the run makes way for: that snapshot is the coming run's,
// and stays.
func TestPruneChangeRecords(t *testing.T) {
	record := func(r string) string {
		message, before := strings.CutSuffix(r, "+")
		if r == "?" {
			return "not a record"
		}
		data, err := json.Marshal(changeRecord{Message: message, Tree: "tree-" + r, Before: before, At: time.Now().UTC()})
		if err != nil {
			t.Fatal(err)
		}
		return string(data)
	}
	for name, test := range map[string]struct {
		records, keep, next, want string // records as "m+" for one taken as the run of m started
	}{
		"of the discarded runs":       {"m1+ m1 m2+ m2 m3+ m3", "m1", "", "m1+ m1"},
		"but the coming run's":        {"m1+ m1 m2+ m2 m3+ m3 n+", "m1", "n", "m1+ m1 n+"},
		"when it took none":           {"m1+ m1 m2+ m2", "m1", "n", "m1+ m1"},
		"after a rewind to the start": {"m1+ m1 n+", "", "n", "n+"},
		"what is not a record":        {"m1+ ? m2+", "m1", "", "m1+ ?"},
		"none when all are dropped":   {"m1+ m1", "", "", ""},
		"all when none are dropped":   {"m1+ m1 n+", "m1", "n", "m1+ m1 n+"},
	} {
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			const id = "changed"
			path := changeRecordsPath(dir, id)
			if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
				t.Fatal(err)
			}
			lines := func(records string) string {
				var out strings.Builder
				for r := range strings.FieldsSeq(records) {
					out.WriteString(record(r) + "\n")
				}
				return out.String()
			}
			data := lines(test.records)
			if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
				t.Fatal(err)
			}
			keep := map[string]bool{}
			for m := range strings.FieldsSeq(test.keep) {
				keep[m] = true
			}
			if err := pruneChangeRecords(dir, id, test.next, keep); err != nil {
				t.Fatal(err)
			}
			got, err := os.ReadFile(path)
			if errors.Is(err, fs.ErrNotExist) {
				got, err = []byte("(none)"), nil
			}
			if err != nil {
				t.Fatal(err)
			}
			want := lines(test.want)
			if test.want == "" {
				want = "(none)"
			}
			// The trees tell the records apart; their times may differ.
			if trees(string(got)) != trees(want) {
				t.Errorf("records after the rewind:\n%s\nwant:\n%s", got, want)
			}
		})
	}
}

// trees lists the trees of change records, a line each, and the lines that
// are not records.
func trees(records string) string {
	var out []string
	for line := range strings.Lines(records) {
		var r changeRecord
		if json.Unmarshal([]byte(line), &r) != nil {
			out = append(out, strings.TrimSpace(line))
			continue
		}
		out = append(out, r.Tree)
	}
	return strings.Join(out, " ")
}
