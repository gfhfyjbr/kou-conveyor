package cockpit

import (
	"encoding/json/v2"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

const messageID = "6f1c2b1e-3a0e-4c55-9a43-0d8d3f1b6a10"

var recorded = time.Date(2026, 9, 23, 12, 0, 0, 0, time.UTC)

func liveLine(t *testing.T, sequence int, kind sessionstore.ItemKind, data any) []byte {
	t.Helper()
	line, err := json.Marshal(sessionstore.Item{
		Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded.Add(time.Duration(sequence) * time.Second),
		Kind: kind, Data: data,
	})
	if err != nil {
		t.Fatal(err)
	}
	return line
}

func fileLine(t *testing.T, recordType string, data any) []byte {
	t.Helper()
	line, err := json.Marshal(map[string]any{"type": recordType, "data": data})
	if err != nil {
		t.Fatal(err)
	}
	return line
}

func shellOperation(t *testing.T, status operation.Status, result *operation.ShellResult) operation.Operation {
	t.Helper()
	state, err := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "go test ./..."}, Result: result})
	if err != nil {
		t.Fatal(err)
	}
	return operation.Operation{ID: "op-1", Type: operation.TypeShell, Version: operation.VersionShell, Status: status, State: state}
}

func apply(t *testing.T, tr *Transcript, lines ...[]byte) {
	t.Helper()
	for _, line := range lines {
		if _, err := tr.Apply(line); err != nil {
			t.Fatalf("apply %s: %v", line, err)
		}
	}
}

func TestTranscriptFoldsLiveRun(t *testing.T) {
	tr := NewTranscript()
	pending := tr.Submit(messageID, "run the tests", recorded)
	payload, _ := json.Marshal("run the tests")
	waiting := tool.CallStatus{WaitingFor: []operation.ID{"op-1"}}
	apply(t, tr,
		liveLine(t, 1, sessionstore.ItemInput, inbox.Input{ID: inbox.ID(messageID), Kind: inbox.InputExternal, Payload: payload}),
		liveLine(t, 2, sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular}),
		liveLine(t, 3, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{
				{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"**Planning**"}}},
				{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"go test ./...","max_output_length":100}`}},
			},
			Usage: llm.Usage{InputTokens: 100, CachedInputTokens: 60, OutputTokens: 7, ReasoningTokens: 3},
		}}),
	)
	if tr.Activity != "Running Bash" {
		t.Fatalf("activity = %q", tr.Activity)
	}
	apply(t, tr, liveLine(t, 4, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Status: waiting,
		Operations: []operation.Operation{shellOperation(t, operation.StatusReady, nil)},
	}))
	call := tr.Entry("tool:call-1")
	if call == nil || call.Tool.State != ToolRunning || call.Tool.Input != "go test ./..." {
		t.Fatalf("running tool = %#v", call)
	}
	changed, err := tr.Apply(liveLine(t, 5, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Status: waiting,
		Operations: []operation.Operation{shellOperation(t, operation.StatusCompleted, &operation.ShellResult{Out: "ok\n", Err: "warn", ExitCode: 1})},
	}))
	if err != nil || len(changed) != 1 || changed[0] != call {
		t.Fatalf("changed = %#v, %v", changed, err)
	}
	if call.Tool.State != ToolDone || *call.Tool.ExitCode != 1 || call.Tool.Output != "ok\n" || call.Tool.Stderr != "warn" {
		t.Fatalf("finished tool = %#v", call.Tool)
	}
	if call.Tool.Finished.Sub(call.Tool.Started) != 2*time.Second {
		t.Fatalf("tool timing = %v → %v", call.Tool.Started, call.Tool.Finished)
	}
	apply(t, tr, liveLine(t, 6, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
		Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "One test fails.", Phase: "final_answer"}}},
		Usage:  llm.Usage{InputTokens: 150, OutputTokens: 5},
	}}))

	if pending.State != "" || pending.At != recorded.Add(time.Second) {
		t.Fatalf("prompt was not settled by the persisted input: %#v", pending)
	}
	var kinds []string
	for _, e := range tr.Entries {
		kinds = append(kinds, e.Kind)
	}
	if got := strings.Join(kinds, ","); got != "user,reasoning,tool,assistant" {
		t.Fatalf("kinds = %s", got)
	}
	// The turn began at 2s and its first response came at 3s; the second
	// is no answer of a turn begun since.
	want := Usage{Input: 250, Cached: 60, Output: 12, Reasoning: 3, Context: 150, Turns: 2,
		TimedOutput: 7, TimedMillis: 1000, LastOutput: 7, LastMillis: 1000}
	if tr.Usage != want {
		t.Fatalf("usage = %#v, want %#v", tr.Usage, want)
	}
	if tr.Title() != "run the tests" {
		t.Fatalf("title = %q", tr.Title())
	}
}

// The time from a turn's start to its response is how fast its model wrote
// the output tokens the response reports: the tokens per second of the
// latest turn, and of the session's turns together.
func TestTranscriptTimesTurns(t *testing.T) {
	tr := NewTranscript()
	sequence := 0
	item := func(after time.Duration, kind sessionstore.ItemKind, data any) []byte {
		sequence++
		line, err := json.Marshal(sessionstore.Item{
			Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded.Add(after), Kind: kind, Data: data,
		})
		if err != nil {
			t.Fatal(err)
		}
		return line
	}
	turn := func(after time.Duration, id session.TurnID) []byte {
		return item(after, sessionstore.ItemTurn, session.Turn{ID: id, Type: session.TurnRegular})
	}
	response := func(after time.Duration, id session.TurnID, output int64) []byte {
		return item(after, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: id, Response: llm.Response{
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "ok"}}},
			Usage:  llm.Usage{InputTokens: 10, OutputTokens: output},
		}})
	}
	speed := func() [4]int64 {
		u := tr.Usage
		return [4]int64{u.TimedOutput, u.TimedMillis, u.LastOutput, u.LastMillis}
	}

	apply(t, tr, turn(0, "turn-1"), response(2*time.Second, "turn-1", 100))
	if got := speed(); got != [4]int64{100, 2000, 100, 2000} {
		t.Fatalf("after a turn: %v", got)
	}
	apply(t, tr, turn(3*time.Second, "turn-2"), response(3500*time.Millisecond, "turn-2", 50))
	if got := speed(); got != [4]int64{150, 2500, 50, 500} {
		t.Fatalf("after two turns: %v", got)
	}
	// A response that reports no output tokens, one to a turn other than
	// the latest, and one that comes again for its turn say nothing of it.
	apply(t, tr,
		turn(4*time.Second, "turn-3"), response(5*time.Second, "turn-3", 0),
		turn(6*time.Second, "turn-4"), turn(7*time.Second, "turn-5"), response(8*time.Second, "turn-4", 30),
		response(9*time.Second, "turn-5", 40), response(10*time.Second, "turn-5", 40))
	if got := speed(); got != [4]int64{190, 4500, 40, 2000} {
		t.Fatalf("after the turns not timed: %v", got)
	}
}

func TestTranscriptReadsSessionFileRecords(t *testing.T) {
	tr := NewTranscript()
	payload, _ := json.Marshal("hello")
	item := func(sequence int, kind sessionstore.ItemKind, data any, operations ...operation.Operation) []byte {
		return fileLine(t, "item", map[string]any{
			"Item": sessionstore.Item{
				Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded, Kind: kind, Data: data,
			},
			"Operations": operations,
		})
	}
	waiting := tool.CallStatus{WaitingFor: []operation.ID{"op-1"}}
	apply(t, tr,
		fileLine(t, "session", map[string]any{"Version": 2, "Session": session.Session{ID: "s", CreatedAt: recorded}}),
		item(1, sessionstore.ItemInput, inbox.Input{ID: "in-1", Kind: inbox.InputExternal, Payload: payload}),
		item(2, sessionstore.ItemTurn, session.Turn{ID: "turn-1"}),
		item(3, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"ls"}`}}},
		}}),
		item(4, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "turn-1", CallID: "call-1", Status: waiting},
			shellOperation(t, operation.StatusReady, nil)),
	)
	call := tr.Entry("tool:call-1")
	if call.Tool.State != ToolRunning {
		t.Fatalf("state = %q", call.Tool.State)
	}
	// Operation checkpoints complete the tool before its second status.
	apply(t, tr, fileLine(t, "operation", map[string]any{
		"Operation": shellOperation(t, operation.StatusCompleted, &operation.ShellResult{Out: "a b c"}),
	}))
	if call.Tool.State != ToolDone || call.Tool.Output != "a b c" || call.Tool.Input != "ls" {
		t.Fatalf("tool = %#v", call.Tool)
	}
	// The status recorded as the operation finished names it by its ID, as
	// the file has it already.
	apply(t, tr, fileLine(t, "item", map[string]any{
		"Item": sessionstore.Item{
			Sequence: 5, RecordedAt: recorded, Kind: sessionstore.ItemToolCallStatus,
			Data: sessionstore.ToolCallStatus{TurnID: "turn-1", CallID: "call-1", Status: waiting},
		},
		"Operations": []operation.ID{"op-1"},
	}))
	if call.Tool.State != ToolDone || call.Tool.Output != "a b c" {
		t.Fatalf("tool after its last status = %#v", call.Tool)
	}
	if tr.Entry("input:in-1").Text != "hello" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
}

func TestTranscriptCallErrorFailsTool(t *testing.T) {
	tr := NewTranscript()
	apply(t, tr, liveLine(t, 1, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-9", Status: tool.CallStatus{Error: `bash argument "command" must be set`},
	}))
	call := tr.Entry("tool:call-9")
	if call.Tool.State != ToolFailed || !strings.Contains(call.Tool.Error, "must be set") || len(tr.Running()) != 0 {
		t.Fatalf("tool = %#v", call.Tool)
	}
}

func TestTranscriptFoldsCodeCalls(t *testing.T) {
	exit := func(code int) *int { return &code }
	start := recorded.Add(time.Second)
	calls := []codevm.Call{
		{Index: 0, Name: "bash", Arguments: "set -euo pipefail\ngo test ./...", Output: "FAIL", ExitCode: exit(1), Started: start, Finished: start.Add(2 * time.Second)},
		{Index: 1, Name: "edit", Arguments: "/work/cmd/main.go", Error: "edit: oldString was not found in cmd/main.go", Started: start, Finished: start.Add(time.Second)},
		{Index: 2, Name: "readText", Arguments: "/work/go.mod", Started: start.Add(time.Second), Running: true},
	}
	status := func(sequence int, status operation.Status, result *codevm.Result) []byte {
		state, err := json.Marshal(operation.CodeState{Code: "await bash('go test ./...')", Config: codevm.Config{Shell: "/bin/bash", Directory: "/work"}, Result: result})
		if err != nil {
			t.Fatal(err)
		}
		return liveLine(t, sequence, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "turn-1", CallID: "call-1", Status: tool.CallStatus{WaitingFor: []operation.ID{"op-1"}},
			Operations: []operation.Operation{{ID: "op-1", Type: operation.TypeCode, Version: operation.VersionCode, Status: status, State: state}},
		})
	}
	summary := func(calls []CodeCall) string {
		var lines []string
		for _, call := range calls {
			lines = append(lines, call.State+" "+call.Gist)
		}
		return strings.Join(lines, "; ")
	}
	tr := NewTranscript()
	apply(t, tr,
		liveLine(t, 1, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Code", Arguments: `{"code":"await bash('go test ./...')"}`}}},
		}}),
		status(2, operation.StatusAwaiting, &codevm.Result{Calls: calls, Started: start}),
	)
	code := tr.Entry("tool:call-1").Tool
	if got, want := summary(code.Calls), "done go test ./...; failed cmd/main.go; running go.mod"; code.State != ToolRunning || got != want {
		t.Fatalf("running code: %s, calls %q, want %q", code.State, got, want)
	}
	// The code is highlighted as JavaScript: it starts with a keyword.
	if len(code.Syntax) < 2 || code.Syntax[0] != int32(len("await")) || highlight.Classes[code.Syntax[1]] != "k" {
		t.Fatalf("syntax = %v", code.Syntax)
	}
	// Code that threw ends its run, and the call it left running with it:
	// the Code call failed, though its operation completed.
	apply(t, tr, status(3, operation.StatusCompleted, &codevm.Result{
		Calls: calls, Error: "ReferenceError: x is not defined", Started: start, Finished: start.Add(3 * time.Second), Done: true,
	}))
	code = tr.Entry("tool:call-1").Tool
	if got, want := summary(code.Calls), "done go test ./...; failed cmd/main.go; canceled go.mod"; code.State != ToolFailed || got != want || !strings.Contains(code.Error, "ReferenceError") {
		t.Fatalf("code that threw: %s %q, calls %q, want %q", code.State, code.Error, got, want)
	}
}

func TestCodeGist(t *testing.T) {
	for _, test := range []struct{ name, input, want string }{
		{"readText", "/work/cmd/main.go", "cmd/main.go"},
		{"viewImage", "/work/shots/tree.png", "shots/tree.png"},
		{"edit", "/elsewhere/main.go", "/elsewhere/main.go"},
		{"read", "cmd/main.go", "cmd/main.go"},
		{"applyPatch", "*** Begin Patch\n*** Update File: a.go\n@@\n-x\n+y\n*** Add File: b/c.go\n+package c\n*** Delete File: d.go\n*** End Patch", "Update a.go, Add b/c.go, Delete d.go"},
		{"applyPatch", "*** Begin Patch\n*** End Patch", "*** Begin Patch"},
		{"bash", "set -euo pipefail\n# build it first\n\ngo build ./...\ngo test ./...", "go build ./..."},
		{"bash", "set -e; go test ./...", "set -e; go test ./..."},
		{"bash", "#!/bin/sh\nset -e\n", "#!/bin/sh"},
		{"bash", "\n  ls -la  \n", "ls -la"},
		{"transcriptSearch", "\n\nfoo|bar\nmore", "foo|bar"},
		{"sleep", "", ""},
	} {
		if got := codeGist(test.name, test.input, "/work"); got != test.want {
			t.Errorf("codeGist(%q, %q) = %q, want %q", test.name, test.input, got, test.want)
		}
	}
}

func TestTranscriptFinish(t *testing.T) {
	t.Run("undelivered prompt and unreported exit", func(t *testing.T) {
		tr := NewTranscript()
		prompt := tr.Submit(messageID, "hi", recorded)
		changed := tr.Finish(errors.New("exit status 2"), false, recorded)
		if prompt.State != Undelivered || len(changed) != 2 || changed[1].Kind != KindError {
			t.Fatalf("changed = %#v", changed)
		}
	})
	t.Run("reported failure is not repeated", func(t *testing.T) {
		tr := NewTranscript()
		tr.Submit(messageID, "hi", recorded)
		apply(t, tr, []byte(`{"type":"error","message":"no \u001b[31mkey\u001b[0m"}`))
		changed := tr.Finish(errors.New("exit status 1"), false, recorded)
		if len(changed) != 1 || tr.Entries[1].Text != "no key" {
			t.Fatalf("changed = %#v entries = %#v", changed, tr.Entries)
		}
	})
	t.Run("live-only entries of different runs never share an ID", func(t *testing.T) {
		// The web server folds every run into a fresh transcript.
		ids := make(map[string]bool)
		for _, message := range []string{messageID, "9a0d6a52-2cde-4b8f-8f0d-7d4c1e2b3a45"} {
			tr := NewTranscript()
			tr.Submit(message, "hi", recorded)
			apply(t, tr, []byte(`{"type":"error","message":"boom"}`))
			tr.Finish(errors.New("canceled"), true, recorded)
			for _, e := range tr.Entries[1:] {
				if ids[e.ID] {
					t.Fatalf("duplicate ID %q across runs", e.ID)
				}
				ids[e.ID] = true
			}
		}
	})
	t.Run("stopped run cancels running tools", func(t *testing.T) {
		tr := NewTranscript()
		tr.Submit(messageID, "hi", recorded)
		apply(t, tr, liveLine(t, 1, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{}`}}},
		}}))
		tr.Finish(errors.New("canceled"), true, recorded)
		if tr.Entry("tool:call-1").Tool.State != ToolCanceled || tr.Entries[len(tr.Entries)-1].Text != "Run stopped" {
			t.Fatalf("entries = %#v", tr.Entries)
		}
	})
	t.Run("stop the session records says why, once", func(t *testing.T) {
		tr := NewTranscript()
		tr.Submit(messageID, "hi", recorded)
		control, err := json.Marshal(inbox.ControlMessage{Mode: inbox.StopHard, Reason: "the user stopped the run from the cockpit"})
		if err != nil {
			t.Fatal(err)
		}
		apply(t, tr, liveLine(t, 2, sessionstore.ItemInput, inbox.Input{
			ID: "0f5c3f6e-8f43-4e8a-9f0e-2b1d6c7a8e90", Kind: inbox.InputControl, Payload: control,
		}))
		tr.Finish(errors.New("canceled"), true, recorded)
		var notices []string
		for _, e := range tr.Entries {
			if e.Kind == KindNotice {
				notices = append(notices, e.Text)
			}
		}
		if len(notices) != 1 || notices[0] != "Run stopped: the user stopped the run from the cockpit" || tr.Entry("stop:2") == nil {
			t.Fatalf("notices = %q", notices)
		}
		// The next run's stop, which the session does not record, is marked
		// as before.
		tr.Submit("9a0d6a52-2cde-4b8f-8f0d-7d4c1e2b3a45", "again", recorded)
		tr.Finish(errors.New("canceled"), true, recorded)
		if last := tr.Entries[len(tr.Entries)-1]; last.Text != "Run stopped" {
			t.Fatalf("last entry = %#v", last)
		}
	})
}

func TestClean(t *testing.T) {
	for input, want := range map[string]string{
		"\x1b[32mok\x1b[0m\tdone":         "ok\tdone",
		"10%\r50%\r100%\nnext":            "100%\nnext",
		"line\r\nbreak\x07":               "line\nbreak",
		"bad\xffutf8":                     "bad\uFFFDutf8",
		"del\x7f and next\u0085":          "del and next",
		"plain\ttext\nкириллица 🙂 \uFFFD": "plain\ttext\nкириллица 🙂 \uFFFD",
	} {
		if got := Clean(input); got != want {
			t.Errorf("Clean(%q) = %q, want %q", input, got, want)
		}
	}
	// What most tools print has nothing to clean: it is kept, not copied.
	plain := strings.Repeat("ok  \tgithub.com/x/y\t0.4s\n", 100)
	if allocs := testing.AllocsPerRun(10, func() { plain = Clean(plain) }); allocs != 0 {
		t.Errorf("Clean of plain text allocates %v times", allocs)
	}
	if got := Headline("  a\n b   c ", 3); got != "a …" {
		t.Errorf("Headline = %q", got)
	}
}

// Clean keeps text it has nothing to do to as it is, and cleans the rest:
// the two come to the same.
func FuzzClean(f *testing.F) {
	for _, seed := range []string{
		"", "go test ./...\n\tok", "\x1b[32mok\x1b[0m", "10%\r50%", "line\r\nbreak", "bell\x07", "del\x7f",
		"next\u0085line", "\u009b31m", "already \uFFFD", "bad\xffutf8", "кириллица ћ ě … € 🙂", "\u202eflipped\u200b",
		"\x1b]8;;http://x\x07link\x1b]8;;\x07", "tab\tand\vvertical\fform",
	} {
		f.Add(seed)
	}
	f.Fuzz(func(t *testing.T, s string) {
		if got, want := Clean(s), clean(s); got != want {
			t.Fatalf("Clean(%q) = %q, but cleaning it makes %q", s, got, want)
		}
	})
}
