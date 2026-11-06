package coordinator

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestCoordinatorMalformedCheckpointFallbackIsTransactional(t *testing.T) {
	for _, mode := range []string{"call-id", "turn-id", "turn-models", "compaction-turns", "builder", "negative-inputs", "turn-type"} {
		t.Run(mode, func(t *testing.T) {
			dir, _ := checkpointBenchmarkFixture(t, 2, 100)
			store, err := localfile.New(dir)
			checkpointRequire(t, err)
			resumed, err := store.Resume(t.Context(), checkpointSession)
			checkpointRequire(t, err)
			var value coordinatorCheckpoint
			checkpointRequire(t, json.Unmarshal(resumed.Checkpoint.State, &value))
			switch mode {
			case "call-id":
				value.ToolCalls = []checkpointToolCall{{Turn: "turn-0"}}
			case "turn-id":
				value.ToolCalls = []checkpointToolCall{{Call: llm.ToolCall{CallID: "invalid"}}}
			case "turn-models":
				value.TurnModels = nil
			case "compaction-turns":
				value.CompactionTurns = nil
			case "builder":
				value.Builder = jsontext.Value(`{"Version":999}`)
			case "negative-inputs":
				value.DeliveredInputs = -1
			case "turn-type":
				value.CurrentTurnType = "invalid"
			}
			encoded, err := json.Marshal(value)
			checkpointRequire(t, err)
			if mode == "turn-models" || mode == "compaction-turns" {
				var fields map[string]jsontext.Value
				checkpointRequire(t, json.Unmarshal(encoded, &fields))
				delete(fields, map[string]string{"turn-models": "TurnModels", "compaction-turns": "CompactionTurns"}[mode])
				encoded, err = json.Marshal(fields)
				checkpointRequire(t, err)
			}
			// Publishing via the real store makes this a valid-checksum record
			// whose consumer data alone is invalid.
			checkpointRequire(t, store.SaveCheckpoint(t.Context(), checkpointSession, encoded))
			resumed, err = store.Resume(t.Context(), checkpointSession)
			checkpointRequire(t, err)
			counted := &countedHistory{Store: store}
			current := checkpointCoordinator(counted, resumed)
			checkpointRequire(t, current.restore(t.Context()))
			if counted.reads == 0 {
				t.Fatal("malformed consumer checkpoint was accepted")
			}
			want, _ := checkpointRestore(t, dir, false)
			gotRequest, err := current.dependencies.ContextBuilder.Build()
			checkpointRequire(t, err)
			wantRequest, err := want.dependencies.ContextBuilder.Build()
			checkpointRequire(t, err)
			if !reflect.DeepEqual(gotRequest, wantRequest) || !reflect.DeepEqual(current.state, want.state) {
				t.Fatal("fallback replay started on partially restored state")
			}
		})
	}
}

func checkpointPendingFixture(t *testing.T, errorOnly bool) (string, *coordinator) {
	t.Helper()
	dir := t.TempDir()
	store, err := localfile.New(dir)
	checkpointRequire(t, err)
	snapshot, err := store.Create(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	current := checkpointCoordinator(store, sessionstore.ResumeState{Snapshot: snapshot})
	checkpointInput(t, current, "first", "run the plugin")
	checkpointTurn(t, current, "ordinary", session.TurnRegular)
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "ordinary", Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "slow", Name: tool.ViewImageName, Arguments: `{}`}},
	}}})
	status := sessionstore.ToolCallStatus{TurnID: "ordinary", CallID: "slow"}
	if errorOnly {
		status.Status.Error = "invalid arguments"
	} else {
		status.Status.WaitingFor = []operation.ID{"slow"}
		status.Operations = []operation.Operation{{ID: "slow", Type: "checkpoint-test", Version: 1, Status: operation.StatusReady}}
	}
	checkpointRecordItem(t, current, sessionstore.ItemToolCallStatus, status)
	return dir, current
}

func TestCheckpointRequiresActiveTranslatorBeforeDispatch(t *testing.T) {
	for _, errorOnly := range []bool{false, true} {
		t.Run(map[bool]string{false: "active-operation", true: "error-only"}[errorOnly], func(t *testing.T) {
			dir, writer := checkpointPendingFixture(t, errorOnly)
			checkpointTurn(t, writer, "compact", session.TurnCompaction)
			_, err := writer.handleModelResponse(t.Context(), sessionstore.ModelResponse{TurnID: "compact", Response: textResponse("summary")})
			checkpointRequire(t, err)
			for _, fast := range []bool{false, true} {
				store, err := localfile.New(dir)
				checkpointRequire(t, err)
				resumed, err := store.Resume(t.Context(), checkpointSession)
				checkpointRequire(t, err)
				if !fast {
					resumed.Checkpoint = nil
				}
				current := checkpointCoordinator(store, resumed)
				current.dependencies.Tools = tool.NewRegistry(tool.StaticTranslators{})
				if errorOnly {
					checkpointRequire(t, current.restore(t.Context()))
				} else {
					if err := current.Run(t.Context()); err == nil || !strings.Contains(err.Error(), "is not available") {
						t.Fatalf("fast=%v: missing translator = %v", fast, err)
					}
					if len(current.dependencies.Operations.(*fakeOperationManager).adds) != 0 {
						t.Fatal("dispatched work before validating its translator")
					}
				}
			}
		})
	}
}

type checkpointProgressTranslator struct{ checkpointTranslator }

func (checkpointProgressTranslator) DescribeRunning(ops []operation.Operation) string {
	if len(ops) == 0 || len(ops[0].State) == 0 {
		return "initial recorded progress"
	}
	return "progress " + string(ops[0].State)
}

func TestCheckpointIgnoresTransientRunningProgressAndOrdering(t *testing.T) {
	dir, current := checkpointPendingFixture(t, false)
	current.dependencies.Tools = tool.NewRegistry(tool.StaticTranslators{ViewImage: checkpointProgressTranslator{}}, tool.ViewImageName)
	// Re-record the pending status with the progress-capable translator.
	value := current.state.operations["slow"]
	checkpointRecordItem(t, current, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "ordinary", CallID: "slow", Status: tool.CallStatus{WaitingFor: []operation.ID{"slow"}}, Operations: []operation.Operation{value}})
	update := func(text string) {
		value.State = jsontext.Value(`"` + text + `"`)
		checkpointRequire(t, current.handleOperationUpdate(t.Context(), value))
	}
	update("before-compaction")
	checkpointTurn(t, current, "compact-1", session.TurnCompaction)
	checkpointInput(t, current, "during", "input before progress refresh")
	update("during-compaction")
	checkpointControl(t, current, "heartbeat", inbox.ControlMessage{Mode: inbox.Heartbeat, Reason: "input after progress refresh"})
	_, err := current.handleModelResponse(t.Context(), sessionstore.ModelResponse{TurnID: "compact-1", Response: textResponse("Slow still runs.")})
	checkpointRequire(t, err)
	// Both restores need the same progress-capable tool registration. It
	// changes only live refreshes, not the transcript's recorded statuses.
	compare := func() {
		var restored []*coordinator
		for _, fast := range []bool{false, true} {
			store, err := localfile.New(dir)
			checkpointRequire(t, err)
			resumed, err := store.Resume(t.Context(), checkpointSession)
			checkpointRequire(t, err)
			if !fast {
				resumed.Checkpoint = nil
			}
			run := checkpointCoordinator(store, resumed)
			run.dependencies.Tools = current.dependencies.Tools
			checkpointRequire(t, run.restore(t.Context()))
			restored = append(restored, run)
		}
		want, err := restored[0].dependencies.ContextBuilder.Build()
		checkpointRequire(t, err)
		got, err := restored[1].dependencies.ContextBuilder.Build()
		checkpointRequire(t, err)
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("progress checkpoint changed replay/order:\nfast=%#v\nfull=%#v", got, want)
		}
	}
	compare()
	update("after-first-checkpoint")
	checkpointTurn(t, current, "continuation", session.TurnRegular)
	checkpointRecordItem(t, current, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "continuation", Response: textResponse("continuing")})
	checkpointTurn(t, current, "compact-2", session.TurnCompaction)
	_, err = current.handleModelResponse(t.Context(), sessionstore.ModelResponse{TurnID: "compact-2", Response: textResponse("Still waiting.")})
	checkpointRequire(t, err)
	compare()
	value.Status, value.State = operation.StatusCompleted, jsontext.Value(`"completed after two checkpoints"`)
	checkpointFinish(t, current, "ordinary", "slow", value)
	compare()
}
