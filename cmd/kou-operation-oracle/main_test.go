package main

import (
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

func TestFileTransitionNeverExecutesItsCompute(t *testing.T) {
	path := filepath.Join(t.TempDir(), "owned.txt")
	if err := os.WriteFile(path, []byte("unchanged"), 0o600); err != nil {
		t.Fatal(err)
	}
	value, err := operation.NewFileSpec(operation.FileState{Action: "write", Path: path, Content: "must not run"})
	if err != nil {
		t.Fatal(err)
	}
	current := operation.Operation{ID: "file", Type: value.Type, Version: value.Version, Status: "ready", State: value.State}
	result := run(spec{Action: "file_advance", Operation: current})
	if result.Error != "" || result.Operation == nil || result.Operation.Status != "awaiting" || len(result.Dispatches) != 1 || result.Dispatches[0].Type != primitives.PrimitiveDispatchCompute {
		t.Fatalf("result = %#v", result)
	}
	if content, err := os.ReadFile(path); err != nil || string(content) != "unchanged" {
		t.Fatalf("file changed: %q, %v", content, err)
	}
}

func TestPureRecoveryAndEventProjection(t *testing.T) {
	for _, action := range []operation.FileAction{"read", "write", "edit", "patch"} {
		t.Run(string(action), func(t *testing.T) {
			value, err := operation.NewFileSpec(operation.FileState{Action: action, Path: "/owned-inert-fixture", Patch: "*** Begin Patch\n*** End Patch"})
			if err != nil {
				t.Fatal(err)
			}
			current := operation.Operation{ID: "file", Type: value.Type, Version: value.Version, Status: "awaiting", State: value.State}
			result := run(spec{Action: "file_advance", Operation: current})
			if result.Error != "" || result.Operation == nil {
				t.Fatalf("result = %#v", result)
			}
			if action == "edit" || action == "patch" {
				state, err := operation.DecodeFileState(*result.Operation)
				if err != nil || result.Operation.Status != "failed" || len(result.Dispatches) != 0 || !strings.Contains(state.TerminalError, "interrupted by a restart") {
					t.Fatalf("result = %#v; state = %#v; %v", result, state, err)
				}
			} else if result.Operation.Status != "awaiting" || len(result.Dispatches) != 1 {
				t.Fatalf("result = %#v", result)
			}
			completed := run(spec{Action: "file_advance", Operation: current, Event: &eventSpec{Type: primitives.PrimitiveEventComputeCompleted, Source: "file", ResultKind: "file", Result: jsontext.Value(`{"Text":"owned result","Files":[]}`)}})
			if completed.Error != "" || completed.Operation == nil || completed.Operation.Status != "completed" {
				t.Fatalf("completion = %#v", completed)
			}
		})
	}
}

func TestValueProjectionAndMalformedState(t *testing.T) {
	value, err := operation.NewValueSpec(jsontext.Value(`null`))
	if err != nil {
		t.Fatal(err)
	}
	current := operation.Operation{ID: "value", Type: value.Type, Version: value.Version, Status: "ready", State: value.State}
	completed := run(spec{Action: "value_advance", Operation: current})
	if completed.Error != "" || completed.Operation == nil || completed.Operation.Status != "completed" || len(completed.Dispatches) != 0 {
		t.Fatalf("result = %#v", completed)
	}
	current.State = jsontext.Value(`[]`)
	result := run(spec{Action: "value_decode", Operation: current})
	// encoding/json/v2 deliberately randomizes this modal once per process.
	if result.Error != `decode value operation "value" state: json: cannot unmarshal JSON array into Go operation.ValueState` &&
		result.Error != `decode value operation "value" state: json: unable to unmarshal JSON array into Go operation.ValueState` {
		t.Fatalf("result = %#v", result)
	}
}
