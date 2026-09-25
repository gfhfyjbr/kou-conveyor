package cockpit

import (
	"encoding/json/v2"
	"errors"
	"strings"
	"testing"
	"time"

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
	want := Usage{Input: 250, Cached: 60, Output: 12, Reasoning: 3, Context: 150, Turns: 2}
	if tr.Usage != want {
		t.Fatalf("usage = %#v, want %#v", tr.Usage, want)
	}
	if tr.Title() != "run the tests" {
		t.Fatalf("title = %q", tr.Title())
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
}

func TestClean(t *testing.T) {
	for input, want := range map[string]string{
		"\x1b[32mok\x1b[0m\tdone": "ok\tdone",
		"10%\r50%\r100%\nnext":    "100%\nnext",
		"line\r\nbreak\x07":       "line\nbreak",
		"bad\xffutf8":             "bad\uFFFDutf8",
	} {
		if got := Clean(input); got != want {
			t.Errorf("Clean(%q) = %q, want %q", input, got, want)
		}
	}
	if got := Headline("  a\n b   c ", 3); got != "a …" {
		t.Errorf("Headline = %q", got)
	}
}
