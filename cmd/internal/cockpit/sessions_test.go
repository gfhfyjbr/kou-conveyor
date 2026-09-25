package cockpit

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

func writeSession(t *testing.T, dir, id, prompt string, modified time.Time, tail string) {
	t.Helper()
	payload := []byte(`"` + prompt + `"`)
	var data bytes.Buffer
	for _, line := range [][]byte{
		fileLine(t, "session", map[string]any{"Version": 2, "Session": session.Session{ID: session.ID(id), CreatedAt: recorded}}),
		fileLine(t, "item", map[string]any{"Item": sessionstore.Item{
			Sequence: 1, RecordedAt: modified, Kind: sessionstore.ItemInput,
			Data: inbox.Input{ID: "in-" + inbox.ID(id), Kind: inbox.InputExternal, Payload: payload},
		}}),
	} {
		data.Write(line)
		data.WriteByte('\n')
	}
	data.WriteString(tail)
	path := SessionPath(dir, id)
	if err := os.WriteFile(path, data.Bytes(), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(path, modified, modified); err != nil {
		t.Fatal(err)
	}
}

func TestListSessionsNewestFirstWithTitles(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "older", "first   question", recorded, "")
	writeSession(t, dir, "newer", "second question", recorded.Add(time.Hour), "")
	os.WriteFile(filepath.Join(dir, "bad.name.session.jsonl"), nil, 0o600)
	os.Mkdir(filepath.Join(dir, "operations"), 0o700)

	sessions, err := ListSessions(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(sessions) != 2 || sessions[0].ID != "newer" || sessions[1].Title != "first question" {
		t.Fatalf("sessions = %#v", sessions)
	}
	if missing, err := ListSessions(filepath.Join(dir, "missing")); err != nil || len(missing) != 0 {
		t.Fatalf("missing directory = %#v, %v", missing, err)
	}
}

func TestLoadSessionIgnoresUncommittedTail(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "s1", "hello", recorded, `{"type":"item","data":{"Item":{"Seq`)
	tr, err := LoadSession(dir, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if len(tr.Entries) != 1 || tr.Entries[0].Text != "hello" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	if _, err := LoadSession(dir, "../escape"); err == nil {
		t.Fatal("loaded a session with an invalid ID")
	}
}

func TestLoadSessionReportsUnreadableRecords(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "s1", "hello", recorded, "{not json}\n")
	tr, err := LoadSession(dir, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if last := tr.Entries[len(tr.Entries)-1]; last.Kind != KindNotice {
		t.Fatalf("entries = %#v", tr.Entries)
	}
}

// appendRecord adds a record to a session file, as a run does, and moves
// its time on.
func appendRecord(t *testing.T, dir, id string, item sessionstore.Item, modified time.Time) {
	t.Helper()
	path := SessionPath(dir, id)
	file, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	file.Write(append(fileLine(t, "item", map[string]any{"Item": item}), '\n'))
	file.Close()
	if err := os.Chtimes(path, modified, modified); err != nil {
		t.Fatal(err)
	}
}

// The list goes by when the user last wrote to a session: the agent at work
// in one does not move it, a new prompt does.
func TestListSessionsGoByTheUsersLastPrompt(t *testing.T) {
	dir := t.TempDir()
	writeSession(t, dir, "first", "one", recorded, "")
	writeSession(t, dir, "second", "two", recorded.Add(time.Minute), "")
	order := func() string {
		t.Helper()
		sessions, err := ListSessions(dir)
		if err != nil {
			t.Fatal(err)
		}
		var ids []string
		for _, s := range sessions {
			ids = append(ids, s.ID)
		}
		return strings.Join(ids, ",")
	}
	if got := order(); got != "second,first" {
		t.Fatalf("order = %s", got)
	}
	// The agent answers in the older session, much later: it stays.
	later := recorded.Add(time.Hour)
	appendRecord(t, dir, "first", sessionstore.Item{
		Sequence: 2, RecordedAt: later, Kind: sessionstore.ItemTurn,
		Data: session.Turn{ID: "turn-1", Type: session.TurnRegular},
	}, later)
	if got := order(); got != "second,first" {
		t.Fatalf("after the agent's turn, order = %s", got)
	}
	// A control input is not the user's either.
	appendRecord(t, dir, "first", sessionstore.Item{
		Sequence: 3, RecordedAt: later, Kind: sessionstore.ItemInput,
		Data: inbox.Input{ID: "control-1", Kind: inbox.InputControl, Payload: []byte(`{"Mode":"when_idle"}`)},
	}, later)
	if got := order(); got != "second,first" {
		t.Fatalf("after a control input, order = %s", got)
	}
	// The user writes to it: it comes first.
	appendRecord(t, dir, "first", sessionstore.Item{
		Sequence: 4, RecordedAt: later.Add(time.Minute), Kind: sessionstore.ItemInput,
		Data: inbox.Input{ID: "in-again", Kind: inbox.InputExternal, Payload: []byte(`"again"`)},
	}, later.Add(time.Minute))
	if got := order(); got != "first,second" {
		t.Fatalf("after a prompt, order = %s", got)
	}
	// A rewound session, its file replaced, is read again.
	writeSession(t, dir, "first", "one, edited", recorded.Add(30*time.Second), "")
	if got := order(); got != "second,first" {
		t.Fatalf("after a rewind, order = %s", got)
	}
}

// Pinned sessions keep the order the user gives them.
func TestPinnedSessionsKeepTheirOrder(t *testing.T) {
	dir := t.TempDir()
	for n, id := range []string{"a", "b", "c", "d"} {
		writeSession(t, dir, id, id, recorded.Add(time.Duration(n)*time.Minute), "")
	}
	pinned := func() string {
		t.Helper()
		sessions, err := ListSessions(dir)
		if err != nil {
			t.Fatal(err)
		}
		var ids []string
		for _, s := range sessions {
			if s.Pinned {
				ids = append(ids, s.ID)
			}
		}
		return strings.Join(ids, ",")
	}
	for _, id := range []string{"b", "d", "a"} {
		if err := PinSession(dir, id, true); err != nil {
			t.Fatal(err)
		}
	}
	// Pinned later, placed after.
	if got := pinned(); got != "b,d,a" {
		t.Fatalf("pinned = %s", got)
	}
	// A prompt in a pinned session does not move it among the pinned.
	appendRecord(t, dir, "a", sessionstore.Item{
		Sequence: 2, RecordedAt: recorded.Add(time.Hour), Kind: sessionstore.ItemInput,
		Data: inbox.Input{ID: "in-a2", Kind: inbox.InputExternal, Payload: []byte(`"more"`)},
	}, recorded.Add(time.Hour))
	if got := pinned(); got != "b,d,a" {
		t.Fatalf("after a prompt, pinned = %s", got)
	}
	// Dragged into another order; IDs left out, unknown or not pinned are
	// no trouble.
	if err := OrderPins(dir, []string{"a", "b", "c", "missing", "../x"}); err != nil {
		t.Fatal(err)
	}
	if got := pinned(); got != "a,b,d" {
		t.Fatalf("reordered = %s", got)
	}
	// Unpinned and pinned again, it goes last.
	PinSession(dir, "a", false)
	PinSession(dir, "a", true)
	if got := pinned(); got != "b,d,a" {
		t.Fatalf("pinned again = %s", got)
	}
	if meta := LoadMeta(dir, "c"); meta.Pinned || meta.PinOrder != 0 {
		t.Fatalf("an unpinned session got an order: %+v", meta)
	}
}
