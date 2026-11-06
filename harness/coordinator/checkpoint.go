package coordinator

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// These replay-derived values differ from the live loop's: verification and
// resume notices also change the live values without recording a new item.
// A checkpoint must reproduce replay, not serialize transient run state.
type replayHistory struct {
	WorkSinceVerification bool
	Verifications         int
	Stopped               *checkpointStop
}

type checkpointStop struct {
	At     time.Time
	Reason string
}

type coordinatorCheckpoint struct {
	Version              int
	Builder              jsontext.Value
	CurrentTurnID        session.TurnID
	CurrentTurnType      session.TurnType
	TurnModels           map[session.TurnID]string
	CompactionTurns      map[session.TurnID]struct{}
	ToolCalls            []checkpointToolCall
	Operations           []operation.Operation
	AvailableInputs      int
	DeliveredInputs      int
	CurrentTurnInputs    int
	Compacted            bool
	CompactionAsked      bool
	TurnsSinceCompaction int
	CompactionFailures   int
	RapidRefills         int
	Reminded             bool
	History              replayHistory
}

type checkpointToolCall struct {
	Turn       session.TurnID
	Call       llm.ToolCall
	Operations []operation.ID
	Status     *tool.CallStatus
}

func (current *coordinator) saveCheckpoint(ctx context.Context) error {
	store, supported := current.dependencies.Sessions.(sessionstore.Checkpointer)
	builder, canRestore := current.dependencies.ContextBuilder.(contextbuilder.Checkpointer)
	if !supported || !canRestore {
		return nil
	}
	// A just-recorded hard stop has no replay timestamp in the live loop.
	// It normally ends the run, rather than compacting. Leave this unusual
	// boundary to ordinary replay instead of guessing the store's timestamp.
	if current.history.Stopped != nil && current.history.Stopped.At.IsZero() {
		return nil
	}
	state := &current.state
	value := coordinatorCheckpoint{
		Version: 1, CurrentTurnID: state.currentTurnID, CurrentTurnType: state.currentTurnType,
		TurnModels: state.turnModels, CompactionTurns: state.compactionTurns,
		AvailableInputs: state.availableInputs, DeliveredInputs: state.deliveredInputs,
		CurrentTurnInputs: state.currentTurnInputs, Compacted: state.compacted,
		CompactionAsked: state.compactionAsked, TurnsSinceCompaction: state.turnsSinceCompaction,
		CompactionFailures: state.compactionFailures, RapidRefills: state.rapidRefills,
		Reminded: state.reminded, History: current.history,
	}
	needed := make(map[operation.ID]bool)
	var liveCalls []string
	for key, call := range state.toolCalls {
		liveCalls = append(liveCalls, key.callID)
		kept := checkpointToolCall{Turn: key.turnID, Call: call.toolCall, Status: call.status}
		for id := range call.operations {
			kept.Operations = append(kept.Operations, id)
			needed[id] = true
		}
		value.ToolCalls = append(value.ToolCalls, kept)
	}
	for _, op := range state.operations {
		if needed[op.ID] || !operationIsTerminal(op.Status) {
			value.Operations = append(value.Operations, op)
		}
	}
	var err error
	if value.Builder, err = builder.Checkpoint(liveCalls); err != nil {
		return err
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return store.SaveCheckpoint(ctx, current.dependencies.SessionID, encoded)
}

// Unknown consumer versions fall back to Store.Items, which still has every
// record. Decode and validate before changing either the builder or the loop.
func (current *coordinator) restoreCheckpoint(encoded jsontext.Value) bool {
	builder, supported := current.dependencies.ContextBuilder.(contextbuilder.Checkpointer)
	if !supported {
		return false
	}
	var value coordinatorCheckpoint
	if json.Unmarshal(encoded, &value) != nil || value.Version != 1 || value.CurrentTurnID == "" ||
		value.TurnModels == nil || value.CompactionTurns == nil || value.CurrentTurnType != session.TurnCompaction ||
		value.AvailableInputs < 0 || value.DeliveredInputs < 0 || value.CurrentTurnInputs < 0 ||
		value.DeliveredInputs > value.AvailableInputs || value.CurrentTurnInputs > value.AvailableInputs {
		return false
	}
	calls := make(map[toolCallKey]toolCallState)
	for _, call := range value.ToolCalls {
		if call.Turn == "" || call.Call.CallID == "" {
			return false
		}
		if call.Status != nil && (call.Status.Error == "" || len(call.Status.WaitingFor) != 0 || len(call.Operations) != 0) {
			if _, available := current.dependencies.Tools.Resolve(call.Call.Name); !available {
				return false
			}
		}
		state := toolCallState{toolCall: call.Call, status: call.Status, operations: make(map[operation.ID]struct{})}
		for _, id := range call.Operations {
			state.operations[id] = struct{}{}
		}
		calls[toolCallKey{turnID: call.Turn, callID: call.Call.CallID}] = state
	}
	if builder.RestoreCheckpoint(value.Builder) != nil {
		return false
	}
	state := &current.state
	state.currentTurnID, state.currentTurnType = value.CurrentTurnID, value.CurrentTurnType
	state.turnModels, state.compactionTurns = value.TurnModels, value.CompactionTurns
	state.toolCalls = calls
	for _, op := range value.Operations {
		state.operations[op.ID] = op
	}
	state.availableInputs, state.deliveredInputs, state.currentTurnInputs = value.AvailableInputs, value.DeliveredInputs, value.CurrentTurnInputs
	state.compacted, state.compactionAsked = value.Compacted, value.CompactionAsked
	state.turnsSinceCompaction, state.compactionFailures, state.rapidRefills = value.TurnsSinceCompaction, value.CompactionFailures, value.RapidRefills
	state.reminded = value.Reminded
	current.history = value.History
	state.workSinceVerification, state.verifications = value.History.WorkSinceVerification, value.History.Verifications
	if stopped := value.History.Stopped; stopped != nil {
		state.stopped = &recordedStop{at: stopped.At, reason: stopped.Reason}
	}
	return true
}
