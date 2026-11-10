package main

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

func current(t *testing.T) operation.Operation {
	t.Helper()
	value, err := operation.NewShellSpec(operation.ShellInput{Shell: "/bin/sh", Command: "printf must-not-run"}, t.TempDir(), 4000)
	if err != nil {
		t.Fatal(err)
	}
	return operation.Operation{ID: "owned", Type: value.Type, Version: value.Version, MaxOutputLength: value.MaxOutputLength, State: value.State, Status: operation.StatusReady}
}
func TestShellOracleDoesNotExecuteArtifactsOrCommands(t *testing.T) {
	op := current(t)
	state, err := operation.DecodeShellState(op)
	if err != nil {
		t.Fatal(err)
	}
	result := run(spec{Action: "shell_advance", Operation: op})
	if result.Error != "" || len(result.Steps) != 1 || result.Steps[0].Operation == nil || len(result.Steps[0].Dispatches) != 1 {
		t.Fatalf("result = %#v", result)
	}
	if _, err := os.Stat(filepath.Join(state.BaseDirectory, "owned")); !os.IsNotExist(err) {
		t.Fatalf("artifact created: %v", err)
	}
}
func TestShellOracleRejectsAllUnownedTimerSignals(t *testing.T) {
	op := current(t)
	state, err := operation.DecodeShellState(op)
	if err != nil {
		t.Fatal(err)
	}
	state.Phase, state.ProcessGroupID = operation.ShellPhaseProcess, os.Getpid()
	op.Status = operation.StatusAwaiting
	op.State, err = json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	for _, correlation := range []primitives.CorrelationID{"timeout", "kill"} {
		result := run(spec{Action: "shell_advance", Operation: op, Event: &eventSpec{Type: primitives.PrimitiveEventTimerFired, Source: "owned", CorrelationID: correlation}})
		if result.Error != "fixture rejected: fired timer with an unowned process group" || len(result.Steps) != 0 {
			t.Fatalf("unsafe fixture accepted: %#v", result)
		}
	}
}
func TestShellOraclePreservesNilCheckpointChunks(t *testing.T) {
	op := current(t)
	state, _ := operation.DecodeShellState(op)
	state.Phase = operation.ShellPhaseReadOut
	op.Status = operation.StatusAwaiting
	op.State, _ = json.Marshal(state)
	result := run(spec{Action: "shell_advance", Operation: op, Event: &eventSpec{Type: primitives.PrimitiveEventIOReadOutput, Source: "owned", CorrelationID: "read_out", ResultKind: "read_output", Result: jsontext.Value(`{"Offset":0,"Data":"b3duZWQ="}`)}})
	if result.Error != "" || len(result.Steps) != 1 || result.Steps[0].Operation != nil || len(result.Steps[0].Dispatches) != 0 {
		t.Fatalf("chunk was checkpointed: %#v", result)
	}
	op.Status = operation.StatusAwaiting
	state.Phase, state.ProcessGroupID = operation.ShellPhaseProcess, 0
	op.State, _ = json.Marshal(state)
	result = run(spec{Action: "shell_advance", Operation: op})
	if result.Error != "" || len(result.Steps) != 1 || !strings.Contains(string(result.Steps[0].Operation.State), "outcome is unknown") {
		t.Fatalf("recovery = %#v", result)
	}
}
