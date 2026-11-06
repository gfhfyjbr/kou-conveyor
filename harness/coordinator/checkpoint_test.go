package coordinator

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"maps"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

const checkpointSession session.ID = "checkpoint-session"

type checkpointTranslator struct{}

func (checkpointTranslator) Translate(tool.Context, llm.ToolCall) tool.CallStatus {
	return tool.CallStatus{}
}
func (checkpointTranslator) TranslateResult(id string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	output := status.Error
	for _, value := range operations {
		output += string(value.State)
	}
	return llm.ToolResult{CallID: id, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: output}}}, nil
}

type countedHistory struct {
	sessionstore.Store
	reads int
}

func (store *countedHistory) Items(ctx context.Context, id session.ID, after sessionstore.Sequence, limit int) (sessionstore.Page, error) {
	store.reads++
	return store.Store.Items(ctx, id, after, limit)
}

func (store *countedHistory) SaveCheckpoint(ctx context.Context, id session.ID, state jsontext.Value) error {
	return store.Store.(sessionstore.Checkpointer).SaveCheckpoint(ctx, id, state)
}

func checkpointBuilder() contextbuilder.Builder {
	builder := contextbuilder.NewBuilder()
	builder.SetSystemPrompt("checkpoint regression configuration")
	builder.SetMemory(contextbuilder.Memory{Notes: "NOTES.md", Summaries: "summaries.log", ReadNotes: func() (string, error) { return "the notes now", nil }})
	return builder
}

func checkpointCoordinator(store sessionstore.Store, resumed sessionstore.ResumeState) *coordinator {
	return New(Dependencies{
		SessionID: checkpointSession, Sessions: store, Restored: resumed,
		ContextBuilder: checkpointBuilder(), Operations: newFakeOperationManager(),
		Tools: tool.NewRegistry(tool.StaticTranslators{ViewImage: checkpointTranslator{}}, tool.ViewImageName),
	}).(*coordinator)
}

func checkpointRequire(tb testing.TB, err error) {
	tb.Helper()
	if err != nil {
		tb.Fatal(err)
	}
}

func checkpointRecordItem(tb testing.TB, current *coordinator, kind sessionstore.ItemKind, data any) {
	tb.Helper()
	item, err := current.addItemToLocalState(sessionstore.Item{Kind: kind, Data: data})
	checkpointRequire(tb, err)
	checkpointRequire(tb, current.storeItemInSessionStore(tb.Context(), item))
}

func checkpointInput(tb testing.TB, current *coordinator, id, text string) {
	tb.Helper()
	payload, err := json.Marshal(text)
	checkpointRequire(tb, err)
	checkpointRecordItem(tb, current, sessionstore.ItemInput, inbox.Input{ID: inbox.ID(id), Kind: inbox.InputExternal, Payload: payload})
}

func checkpointControl(tb testing.TB, current *coordinator, id string, control inbox.ControlMessage) {
	tb.Helper()
	payload, err := json.Marshal(control)
	checkpointRequire(tb, err)
	checkpointRecordItem(tb, current, sessionstore.ItemInput, inbox.Input{ID: inbox.ID(id), Kind: inbox.InputControl, Payload: payload})
}

func checkpointTurn(tb testing.TB, current *coordinator, id string, kind session.TurnType) {
	tb.Helper()
	checkpointRecordItem(tb, current, sessionstore.ItemTurn, session.Turn{
		ID: session.TurnID(id), PreviousTurnID: current.state.currentTurnID, Type: kind, Model: "test/model",
	})
}

func checkpointFinish(tb testing.TB, current *coordinator, turn session.TurnID, call string, value operation.Operation) {
	tb.Helper()
	current.addOperationToLocalState(value)
	checkpointRequire(tb, current.dependencies.Sessions.SaveOperation(tb.Context(), checkpointSession, value))
	checkpointRecordItem(tb, current, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: turn, CallID: call, Status: tool.CallStatus{WaitingFor: []operation.ID{value.ID}}, Operations: []operation.Operation{value},
	})
}

func checkpointRestore(tb testing.TB, dir string, fast bool) (*coordinator, *countedHistory) {
	tb.Helper()
	store, err := localfile.New(dir)
	checkpointRequire(tb, err)
	store.MapLogs()
	resumed, err := store.Resume(tb.Context(), checkpointSession)
	checkpointRequire(tb, err)
	if fast && resumed.Checkpoint == nil {
		tb.Fatal("ordinary resume did not find its checkpoint")
	}
	if !fast {
		resumed.Checkpoint = nil
	}
	counted := &countedHistory{Store: store}
	current := checkpointCoordinator(counted, resumed)
	checkpointRequire(tb, current.restore(tb.Context()))
	if fast && counted.reads != 0 {
		tb.Fatalf("ordinary checkpoint resume read %d archival pages", counted.reads)
	}
	return current, counted
}

func checkpointComparableState(state loopState) loopState {
	state.operations = maps.Clone(state.operations)
	for id, op := range state.operations {
		needed := !operationIsTerminal(op.Status)
		for _, call := range state.toolCalls {
			if _, exists := call.operations[id]; exists {
				needed = true
			}
		}
		if !needed {
			delete(state.operations, id)
		}
	}
	return state
}

func checkpointAssertReplay(tb testing.TB, dir string) {
	tb.Helper()
	full, _ := checkpointRestore(tb, dir, false)
	fast, _ := checkpointRestore(tb, dir, true)
	if !reflect.DeepEqual(checkpointComparableState(fast.state), checkpointComparableState(full.state)) || !reflect.DeepEqual(fast.history, full.history) {
		tb.Fatalf("checkpoint loop state differs:\nfast=%#v\nfull=%#v", checkpointComparableState(fast.state), checkpointComparableState(full.state))
	}
	want, err := full.dependencies.ContextBuilder.Build()
	checkpointRequire(tb, err)
	got, err := fast.dependencies.ContextBuilder.Build()
	checkpointRequire(tb, err)
	if !reflect.DeepEqual(got, want) {
		tb.Fatalf("checkpoint request differs:\nfast=%#v\nfull=%#v", got, want)
	}
	wantCompaction, err := full.dependencies.ContextBuilder.BuildCompaction("next summary", 0)
	checkpointRequire(tb, err)
	gotCompaction, err := fast.dependencies.ContextBuilder.BuildCompaction("next summary", 0)
	checkpointRequire(tb, err)
	if !reflect.DeepEqual(gotCompaction, wantCompaction) {
		tb.Fatal("future compaction request differs")
	}
}

func TestCoordinatorCheckpointReplayEquivalence(t *testing.T) {
	dir := t.TempDir()
	store, err := localfile.New(dir)
	checkpointRequire(t, err)
	snapshot, err := store.Create(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	resumed := sessionstore.ResumeState{Snapshot: snapshot}
	current := checkpointCoordinator(store, resumed)
	checkpointInput(t, current, "first", "use both tools")
	checkpointTurn(t, current, "ordinary", session.TurnRegular)
	response := sessionstore.ModelResponse{TurnID: "ordinary", Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "old", Name: tool.ViewImageName, Arguments: `{}`}},
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "slow", Name: tool.ViewImageName, Arguments: `{}`}},
	}}}
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, response)
	for _, call := range []string{"old", "slow"} {
		checkpointRecordItem(t, current, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "ordinary", CallID: call, Status: tool.CallStatus{WaitingFor: []operation.ID{operation.ID(call)}},
			Operations: []operation.Operation{{ID: operation.ID(call), Type: "checkpoint-test", Version: 1, Status: operation.StatusReady}},
		})
	}
	output, _ := json.Marshal(strings.Repeat("old output before compaction ", 1000))
	checkpointFinish(t, current, "ordinary", "old", operation.Operation{ID: "old", Type: "checkpoint-test", Version: 1, Status: operation.StatusCompleted, State: output})
	checkpointTurn(t, current, "compact-1", session.TurnCompaction)
	checkpointInput(t, current, "during", "late input while compacting")
	_, err = current.handleModelResponse(t.Context(), sessionstore.ModelResponse{TurnID: "compact-1", Response: textResponse("The old tool is done; slow still runs.")})
	checkpointRequire(t, err)
	checkpointAssertReplay(t, dir)

	checkpointFinish(t, current, "ordinary", "slow", operation.Operation{ID: "slow", Type: "checkpoint-test", Version: 1, Status: operation.StatusCompleted, State: jsontext.Value(`"slow result after checkpoint"`)})
	checkpointTurn(t, current, "continuation", session.TurnRegular)
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "continuation", Response: textResponse("Both results received.")})
	checkpointControl(t, current, "verification", inbox.ControlMessage{Mode: inbox.Heartbeat, Reason: verificationPrefix + " check"})
	checkpointControl(t, current, "stopped", inbox.ControlMessage{Mode: inbox.StopHard, Reason: "interrupted run"})
	current, _ = checkpointRestore(t, dir, true)
	// Live verification/resume bookkeeping is not replay state.
	current.state.verifications, current.state.workSinceVerification, current.state.stopped = 99, false, nil
	checkpointTurn(t, current, "compact-2", session.TurnCompaction)
	_, err = current.handleModelResponse(t.Context(), sessionstore.ModelResponse{TurnID: "compact-2", Response: textResponse("Second summary.")})
	checkpointRequire(t, err)
	checkpointAssertReplay(t, dir)
	checkpointInput(t, current, "after", "work after the second checkpoint")
	checkpointControl(t, current, "settings", inbox.ControlMessage{Mode: inbox.UpdateSettings, Parameters: inbox.Settings{ReasoningEffort: llm.ReasoningEffortHigh}})
	checkpointTurn(t, current, "last", session.TurnRegular)
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "last", Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "unrecorded", Name: tool.ViewImageName, Arguments: `{}`}},
	}}})
	pending := operation.Operation{ID: "unrecorded", Type: "checkpoint-test", Version: 1, Status: operation.StatusReady}
	checkpointRecordItem(t, current, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "last", CallID: "unrecorded", Status: tool.CallStatus{WaitingFor: []operation.ID{pending.ID}}, Operations: []operation.Operation{pending}})
	pending.Status, pending.State = operation.StatusCompleted, jsontext.Value(`"not yet in a status"`)
	checkpointRequire(t, current.dependencies.Sessions.SaveOperation(t.Context(), checkpointSession, pending))
	checkpointAssertReplay(t, dir)
}

func TestCoordinatorUnknownCheckpointFallsBackToFullHistory(t *testing.T) {
	store, err := localfile.New(t.TempDir())
	checkpointRequire(t, err)
	snapshot, err := store.Create(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	resumed := sessionstore.ResumeState{Snapshot: snapshot}
	current := checkpointCoordinator(store, resumed)
	checkpointInput(t, current, "first", "legacy-compatible input")
	checkpointTurn(t, current, "ordinary", session.TurnRegular)
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "ordinary", Response: textResponse("answer")})
	checkpointRequire(t, store.SaveCheckpoint(t.Context(), checkpointSession, []byte(`{"Version":999}`)))
	resumed, err = store.Resume(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	counted := &countedHistory{Store: store}
	current = checkpointCoordinator(counted, resumed)
	checkpointRequire(t, current.restore(t.Context()))
	if counted.reads == 0 {
		t.Fatal("unknown consumer checkpoint did not fall back to archival replay")
	}
	built, err := current.dependencies.ContextBuilder.Build()
	checkpointRequire(t, err)
	if !containsMessage(built.Request, "legacy-compatible input") {
		t.Fatal("fallback lost earlier input")
	}
}

// Generate one turn at a time; output strings are reused, and the caller's
// TempDir owns every fixture. The benchmark sets its own bounded turn count.
func checkpointBenchmarkFixture(tb testing.TB, turns, outputBytes int) (string, int64) {
	tb.Helper()
	dir := tb.TempDir()
	store, err := localfile.New(dir)
	checkpointRequire(tb, err)
	snapshot, err := store.Create(tb.Context(), checkpointSession)
	checkpointRequire(tb, err)
	current := checkpointCoordinator(store, sessionstore.ResumeState{Snapshot: snapshot})
	payload, err := json.Marshal(strings.Repeat("x", outputBytes))
	checkpointRequire(tb, err)
	for index := range turns {
		id := fmt.Sprintf("turn-%d", index)
		checkpointInput(tb, current, "input-"+id, "inspect the synthetic output")
		checkpointTurn(tb, current, id, session.TurnRegular)
		checkpointRecordItem(tb, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: session.TurnID(id), Response: llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: tool.ViewImageName, Arguments: `{}`}}}}})
		value := operation.Operation{ID: operation.ID(id), Type: "checkpoint-test", Version: 1, Status: operation.StatusReady}
		checkpointRecordItem(tb, current, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: session.TurnID(id), CallID: id, Status: tool.CallStatus{WaitingFor: []operation.ID{value.ID}}, Operations: []operation.Operation{value}})
		value.Status, value.State = operation.StatusCompleted, payload
		checkpointFinish(tb, current, session.TurnID(id), id, value)
	}
	checkpointTurn(tb, current, "compact", session.TurnCompaction)
	_, err = current.handleModelResponse(tb.Context(), sessionstore.ModelResponse{TurnID: "compact", Response: textResponse("All synthetic outputs were inspected.")})
	checkpointRequire(tb, err)
	checkpointInput(tb, current, "after", "continue from the summary")
	return dir, int64(len(payload)) * int64(turns)
}
