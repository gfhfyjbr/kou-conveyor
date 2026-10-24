package cockpit

import (
	"bytes"
	"encoding/json/v2"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// rewindLog cuts a session log before the run that sent the prompt with the
// given message ID, as a rewind does. It returns the log that remains and the
// operations only the removed part started.
func rewindLog(data []byte, messageID string, now time.Time) ([]byte, []operation.ID, error) {
	cut, err := cutLog(bytes.NewReader(data), "", messageID)
	if err != nil {
		return nil, nil, err
	}
	remains, err := cut.remains(bytes.NewReader(data), now)
	if err != nil {
		return nil, nil, err
	}
	kept, err := io.ReadAll(remains)
	return kept, cut.dropped, err
}

// A command started before the edited prompt may finish only during the
// prompt's run, when the next runner resumes it. The rewind keeps that
// outcome: otherwise the command would look unfinished again and the next
// runner would start it a second time.
func TestRewindKeepsTheOutcomeOfEarlierCommands(t *testing.T) {
	ctx := t.Context()
	dir := t.TempDir()
	store, err := localfile.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	const id = session.ID("rewound")
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	controls := 0
	control := func(message inbox.ControlMessage) inbox.Input {
		controls++
		payload, _ := json.Marshal(message)
		return inbox.Input{ID: inbox.ID(fmt.Sprintf("control-%d", controls)), Kind: inbox.InputControl, Payload: payload}
	}
	run := func(messageID, prompt string) {
		t.Helper()
		payload, _ := json.Marshal(prompt)
		must(store.AppendInput(ctx, id, control(inbox.ControlMessage{Mode: inbox.UpdateSettings, Parameters: inbox.Settings{ReasoningEffort: llm.ReasoningEffortHigh}})))
		must(store.AppendInput(ctx, id, inbox.Input{ID: inbox.ID(messageID), Kind: inbox.InputExternal, Payload: payload}))
		must(store.AppendInput(ctx, id, control(inbox.ControlMessage{Mode: inbox.StopWhenIdle})))
	}
	shell := func(op operation.ID, status operation.Status) operation.Operation {
		state, _ := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "make " + string(op)}})
		return operation.Operation{ID: op, Type: operation.TypeShell, Version: operation.VersionShell, Status: status, State: state}
	}
	call := func(turn session.TurnID, callID string, op operation.ID, status operation.Status) {
		t.Helper()
		must(store.AppendToolCallStatus(ctx, id, sessionstore.ToolCallStatus{
			TurnID: turn, CallID: callID, Status: tool.CallStatus{WaitingFor: []operation.ID{op}},
			Operations: []operation.Operation{shell(op, status)},
		}))
	}
	respond := func(turn session.TurnID, callID string) {
		t.Helper()
		must(store.AppendModelResponse(ctx, id, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
			Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: callID, Name: "Bash", Arguments: `{}`}}},
		}}))
	}

	_, err = store.Create(ctx, id)
	must(err)
	// The first run starts a build and dies while it runs.
	run("prompt-1", "build it")
	must(store.AppendTurn(ctx, id, session.Turn{ID: "turn-1", Type: session.TurnRegular}))
	respond("turn-1", "call-1")
	call("turn-1", "call-1", "build", operation.StatusReady)
	must(store.SaveOperation(ctx, id, shell("build", operation.StatusAwaiting)))
	// The second run sees the build finish, then runs a command of its own.
	run("prompt-2", "and test it")
	must(store.SaveOperation(ctx, id, shell("build", operation.StatusCompleted)))
	call("turn-1", "call-1", "build", operation.StatusCompleted)
	must(store.AppendTurn(ctx, id, session.Turn{ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular}))
	respond("turn-2", "call-2")
	call("turn-2", "call-2", "test", operation.StatusReady)
	must(store.SaveOperation(ctx, id, shell("test", operation.StatusCompleted)))

	data, err := os.ReadFile(SessionPath(dir, string(id)))
	must(err)
	kept, dropped, err := rewindLog(data, "prompt-2", time.Now().UTC())
	must(err)
	if !slices.Equal(dropped, []operation.ID{"test"}) {
		t.Fatalf("dropped operations = %v", dropped)
	}
	if strings.Contains(string(kept), "prompt-2") || strings.Contains(string(kept), "turn-2") {
		t.Fatalf("the rewound log keeps the edited prompt's run:\n%s", kept)
	}
	// The cut falls before the settings the prompt's run began with: what
	// remains is the log as the first run left it, then the build's outcome.
	keep := bytes.LastIndexByte(data[:bytes.Index(data, []byte(`"control-3"`))], '\n') + 1
	want := slices.Clone(data[:keep])
	for line := range bytes.Lines(data[keep:]) {
		record, err := decodeLogRecord(bytes.TrimSuffix(line, []byte("\n")))
		must(err)
		if record.checkpoint && record.operations[0] == "build" {
			want = append(want, line...)
		}
	}
	if !bytes.Equal(kept, want) {
		t.Fatalf("the rewound log:\n%s\nwant:\n%s", kept, want)
	}
	// What is cut off begins when those settings were recorded: the notes and
	// the summaries go back to then (restoreMemory).
	cut, err := cutLog(bytes.NewReader(data), string(id), "prompt-2")
	must(err)
	settings, err := decodeLogRecord(data[keep : keep+bytes.IndexByte(data[keep:], '\n')])
	must(err)
	if !cut.at.Equal(settings.item.RecordedAt) {
		t.Errorf("cut at %v, want %v", cut.at, settings.item.RecordedAt)
	}

	// The rewound session is what the runner will resume.
	rewound := t.TempDir()
	must(os.WriteFile(SessionPath(rewound, string(id)), kept, 0o600))
	fresh, err := localfile.New(rewound)
	must(err)
	state, err := fresh.Resume(ctx, id)
	must(err)
	if !slices.Equal(state.ExternalInputIDs, []inbox.ID{"prompt-1"}) {
		t.Fatalf("prompts after the rewind = %v", state.ExternalInputIDs)
	}
	// The build is done, which the recorded tool call does not say yet: the
	// runner records its result instead of running it again.
	if len(state.Operations) != 1 || state.Operations[0].ID != "build" || state.Operations[0].Status != operation.StatusCompleted {
		t.Fatalf("operations to resume = %+v", state.Operations)
	}
	// The runner continues the rewound session where the first run left it.
	must(fresh.AppendInput(ctx, id, inbox.Input{ID: "prompt-2b", Kind: inbox.InputExternal, Payload: []byte(`"and test it properly"`)}))
	must(fresh.AppendTurn(ctx, id, session.Turn{ID: "turn-2b", PreviousTurnID: "turn-1", Type: session.TurnRegular}))
}

// Editing a prompt reads the session as the rewind will leave it, then
// rewinds it (LoadSessionBefore, then RewindSession). What the session gained
// in between counts: here, the outcome of a command started before the
// prompt, which the rewind keeps.
func TestRewindSeesWhatTheSessionGainedSinceItWasRead(t *testing.T) {
	ctx := t.Context()
	dir := t.TempDir()
	store, err := localfile.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	const id = session.ID("rewound")
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	prompt := func(messageID string) {
		t.Helper()
		must(store.AppendInput(ctx, id, inbox.Input{ID: inbox.ID(messageID), Kind: inbox.InputExternal, Payload: []byte(`"go on"`)}))
	}
	build := func(status operation.Status) operation.Operation {
		state, _ := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "make"}})
		return operation.Operation{ID: "build", Type: operation.TypeShell, Version: operation.VersionShell, Status: status, State: state}
	}
	_, err = store.Create(ctx, id)
	must(err)
	prompt("prompt-1")
	must(store.AppendTurn(ctx, id, session.Turn{ID: "turn-1", Type: session.TurnRegular}))
	must(store.AppendModelResponse(ctx, id, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
		Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{}`}}},
	}}))
	must(store.AppendToolCallStatus(ctx, id, sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Status: tool.CallStatus{WaitingFor: []operation.ID{"build"}},
		Operations: []operation.Operation{build(operation.StatusReady)},
	}))
	prompt("prompt-2")

	if _, err := LoadSessionBefore(dir, string(id), "prompt-2"); err != nil {
		t.Fatal(err)
	}
	// The build finishes before the rewind.
	must(store.SaveOperation(ctx, id, build(operation.StatusCompleted)))
	must(RewindSession(dir, string(id), "prompt-2"))

	fresh, err := localfile.New(dir)
	must(err)
	state, err := fresh.Resume(ctx, id)
	must(err)
	if len(state.Operations) != 1 || state.Operations[0].Status != operation.StatusCompleted {
		t.Fatalf("operations to resume = %+v", state.Operations)
	}

	// Where the rewind cuts is its prompt's: reading the session before
	// another prompt does not decide it.
	must(fresh.AppendInput(ctx, id, inbox.Input{ID: "prompt-3", Kind: inbox.InputExternal, Payload: []byte(`"and then"`)}))
	if _, err := LoadSessionBefore(dir, string(id), "prompt-1"); err != nil {
		t.Fatal(err)
	}
	must(RewindSession(dir, string(id), "prompt-3"))
	data, err := os.ReadFile(SessionPath(dir, string(id)))
	must(err)
	if !strings.Contains(string(data), "prompt-1") || strings.Contains(string(data), "prompt-3") {
		t.Fatalf("the session rewound before prompt-3:\n%s", data)
	}
}

// A rewind that follows the reading of its session before its prompt cuts the
// session where the reading found the cut, rather than reading it again.
func TestRewindTakesTheCutItsReadingFound(t *testing.T) {
	dir := t.TempDir()
	const id = "rewound"
	l := newMemoryLog(t, dir, id)
	for _, prompt := range []string{"prompt-1", "prompt-2"} {
		l.prompt(prompt)
		l.say()
	}
	path := SessionPath(dir, id)
	data, err := os.ReadFile(path)
	l.must(err)
	info, err := os.Stat(path)
	l.must(err)
	// A cut that falls elsewhere than the prompt's tells which one the rewind
	// took.
	elsewhere, err := cutLog(bytes.NewReader(data), id, "prompt-1")
	l.must(err)
	cuts.put(path, "prompt-2", info, elsewhere)
	l.must(rewindSession(dir, id, "prompt-2", ""))
	rewound, err := os.ReadFile(path)
	l.must(err)
	if want := data[:elsewhere.keep]; !bytes.Equal(rewound, want) {
		t.Fatalf("the rewound session:\n%s\nwant:\n%s", rewound, want)
	}
	if _, ok := cuts.take(path, "prompt-2", info); ok {
		t.Error("the cut was left for another rewind to take")
	}
}

// The cut a reading of a session before a prompt found is for the rewind of
// that prompt while the session's file is the one read, as it was read, and
// it is taken once.
func TestCutIsTakenForTheFileAsItWasRead(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.jsonl")
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	stat := func() fs.FileInfo {
		t.Helper()
		info, err := os.Stat(path)
		must(err)
		return info
	}
	// A time file systems keep whole, which a file can be given back.
	then := time.Now().Add(-time.Hour).Truncate(time.Second)
	write := func(name, data string, at time.Time) {
		t.Helper()
		must(os.WriteFile(name, []byte(data), 0o600))
		must(os.Chtimes(name, at, at))
	}
	var last lastCut
	for _, test := range []struct {
		name   string
		prompt string // the rewind's
		change func() // what becomes of the file after it was read
		taken  bool
	}{
		{"as it was", "prompt", func() {}, true},
		{"for another prompt", "another prompt", func() {}, false},
		{"grown", "prompt", func() { write(path, "the log\nand more\n", then) }, false},
		{"written to", "prompt", func() { write(path, "The log\n", then.Add(time.Second)) }, false},
		{"replaced", "prompt", func() { write(path+".new", "the log\n", then); must(os.Rename(path+".new", path)) }, false},
	} {
		write(path, "the log\n", then)
		last.put(path, "prompt", stat(), logCut{keep: 4})
		test.change()
		if cut, ok := last.take(path, test.prompt, stat()); ok != test.taken || ok && cut.keep != 4 {
			t.Errorf("%s: take = %+v, %v", test.name, cut, ok)
		}
		if _, ok := last.take(path, test.prompt, stat()); ok {
			t.Errorf("%s: the cut was taken again", test.name)
		}
	}
}

// branch branches the session l writes before the prompt with the given
// message ID, whose run followed the turn latest, and returns what writes the
// runs of the branch.
func (l *memoryLog) branch(dir, messageID string, latest session.TurnID) *memoryLog {
	l.t.Helper()
	branch, err := BranchSession(l.t.Context(), dir, string(l.id), messageID)
	l.must(err)
	id := session.ID(branch.SessionID)
	// The branch holds the history of l, whose IDs its own follow.
	return &memoryLog{t: l.t, store: l.store, id: id, notes: notesPath(dir, string(id)), inputs: l.inputs, turns: l.turns, turn: latest}
}

// A branch of a branch holds history of its parent and of the parent's
// parent, each followed by a fork that tells whose it is. A rewind into that
// history keeps the forks before the cut, and adds one that marks what remains
// as inherited when no fork remains after it.
func TestRewindIntoTheHistoryOfABranchOfABranch(t *testing.T) {
	dir := t.TempDir()
	grandparent := newMemoryLog(t, dir, "grandparent")
	for _, prompt := range []string{"prompt-1", "prompt-2", "prompt-3"} {
		grandparent.prompt(prompt)
		grandparent.say()
	}
	parent := grandparent.branch(dir, "prompt-3", "turn-2")
	for _, prompt := range []string{"prompt-4", "prompt-5"} {
		parent.prompt(prompt)
		parent.say()
	}
	l := parent.branch(dir, "prompt-5", "turn-4")
	l.prompt("prompt-6")
	l.say()
	id := string(l.id)
	path := SessionPath(dir, id)
	data, err := os.ReadFile(path)
	l.must(err)
	// start is where the record of the input with the given ID begins.
	start := func(input string) int {
		t.Helper()
		at := bytes.Index(data, []byte(`"`+input+`"`))
		if at < 0 {
			t.Fatalf("the session holds no input %s", input)
		}
		return bytes.LastIndexByte(data[:at], '\n') + 1
	}
	rewind := func(messageID string) []byte {
		t.Helper()
		l.must(os.WriteFile(path, data, 0o600))
		l.must(rewindSession(dir, id, messageID, ""))
		rewound, err := os.ReadFile(path)
		l.must(err)
		store, err := localfile.New(dir)
		l.must(err)
		if _, err := store.Resume(t.Context(), l.id); err != nil {
			t.Fatalf("the runner cannot resume the session rewound before %s: %v", messageID, err)
		}
		return rewound
	}

	// Before the parent's first prompt, the fork after the grandparent's
	// history is the last that remains.
	if rewound, want := rewind("prompt-4"), data[:start("control-4")]; !bytes.Equal(rewound, want) {
		t.Errorf("rewound before the parent's first prompt:\n%s\nwant:\n%s", rewound, want)
	}
	// Before the grandparent's second prompt, a fork marks what remains as
	// the grandparent's, after its first turn.
	rewound, keep := rewind("prompt-2"), start("control-2")
	var fork sessionstore.Fork
	if bytes.HasPrefix(rewound, data[:keep]) {
		if record, err := decodeLogRecord(bytes.TrimSuffix(rewound[keep:], []byte("\n"))); err == nil {
			fork, _ = itemData[sessionstore.Fork](record)
		}
	}
	if want := (sessionstore.Fork{ParentID: "grandparent", PreviousTurnID: "turn-1"}); fork != want {
		t.Errorf("rewound before the grandparent's second prompt:\n%s\nwant its first run, then a fork %+v", rewound, want)
	}
}

func TestRewindLogRejectsWhatIsNotASession(t *testing.T) {
	// The error tells which record is wrong.
	header := `{"type":"session","data":{}}` + "\n"
	for name, test := range map[string]struct{ data, want string }{
		"no header":                {`{"type":"operation","data":{"Operation":{"ID":"x"}}}` + "\n", "read session record 1: "},
		"two headers":              {header + header, "read session record 2: "},
		"garbage":                  {"{\n", "read session record 1: "},
		"garbage after the header": {header + "{\n", "read session record 2: "},
	} {
		if _, _, err := rewindLog([]byte(test.data), "prompt", time.Now()); err == nil || !strings.HasPrefix(err.Error(), test.want) {
			t.Errorf("%s: err = %v, want %q…", name, err, test.want)
		}
	}
	// An empty session holds no prompt, and a record cut short does not count.
	for _, data := range []string{
		`{"type":"session","data":{}}` + "\n",
		`{"type":"session","data":{}}` + "\n" + `{"type":"item","data":{"Item":{"Sequ`,
	} {
		if _, _, err := rewindLog([]byte(data), "prompt", time.Now()); err != ErrPromptGone {
			t.Errorf("%q: err = %v", data, err)
		}
	}
}
