package cockpit

import (
	"encoding/json/v2"
	"fmt"
	"os"
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

func TestRewindLogRejectsWhatIsNotASession(t *testing.T) {
	for name, data := range map[string]string{
		"no header":   `{"type":"operation","data":{"Operation":{"ID":"x"}}}` + "\n",
		"two headers": `{"type":"session","data":{}}` + "\n" + `{"type":"session","data":{}}` + "\n",
		"garbage":     "{\n",
	} {
		if _, _, err := rewindLog([]byte(data), "prompt", time.Now()); err == nil || err == ErrPromptGone {
			t.Errorf("%s: err = %v", name, err)
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
