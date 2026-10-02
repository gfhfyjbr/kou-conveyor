package operation_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

func newFileOperation(t *testing.T, id operation.ID, state operation.FileState) operation.Operation {
	t.Helper()
	spec, err := operation.NewFileSpec(state)
	if err != nil {
		t.Fatal(err)
	}
	return operation.Operation{ID: id, Type: spec.Type, Version: spec.Version, Status: operation.StatusReady, State: spec.State}
}

func TestFileOperationReadsEditsAndWrites(t *testing.T) {
	directory := t.TempDir()
	name := filepath.Join(directory, "a.txt")
	manager := operation.NewLocalOperationManager(t.Context())

	written := newFileOperation(t, "write", operation.FileState{Action: operation.FileWrite, Path: name, Content: "one\ntwo\n"})
	if err := manager.Add(written); err != nil {
		t.Fatal(err)
	}
	done := receiveTerminalOperation(t, manager.Updates(), written.ID)
	state, err := operation.DecodeFileState(done)
	if err != nil || done.Status != operation.StatusCompleted || state.Result == nil || !strings.HasPrefix(state.Result.Text, "Wrote "+name+": 2 lines") || state.Result.Files[0] != name {
		t.Fatalf("operation = %#v, state = %#v, err = %v", done, state, err)
	}

	edited := newFileOperation(t, "edit", operation.FileState{Action: operation.FileEdit, Path: name, OldString: "two", NewString: "2"})
	if err := manager.Add(edited); err != nil {
		t.Fatal(err)
	}
	done = receiveTerminalOperation(t, manager.Updates(), edited.ID)
	state, _ = operation.DecodeFileState(done)
	if done.Status != operation.StatusCompleted || !strings.Contains(state.Result.Text, "replaced 1 occurrence at line 2") || !strings.Contains(state.Result.Text, "     2\t2") {
		t.Fatalf("operation = %#v, state = %#v", done, state)
	}

	read := newFileOperation(t, "read", operation.FileState{Action: operation.FileRead, Path: name})
	if err := manager.Add(read); err != nil {
		t.Fatal(err)
	}
	done = receiveTerminalOperation(t, manager.Updates(), read.ID)
	state, _ = operation.DecodeFileState(done)
	if done.Status != operation.StatusCompleted || state.Result.Text != "     1\tone\n     2\t2" || state.Result.Lines != 2 {
		t.Fatalf("operation = %#v, state = %#v", done, state)
	}
}

func TestFileOperationFailsWithAReadableMessage(t *testing.T) {
	manager := operation.NewLocalOperationManager(t.Context())
	missing := filepath.Join(t.TempDir(), "missing.txt")
	read := newFileOperation(t, "read-missing", operation.FileState{Action: operation.FileRead, Path: missing})
	if err := manager.Add(read); err != nil {
		t.Fatal(err)
	}
	done := receiveTerminalOperation(t, manager.Updates(), read.ID)
	state, err := operation.DecodeFileState(done)
	if err != nil || done.Status != operation.StatusFailed || state.TerminalError != missing+" does not exist" || state.Result != nil {
		t.Fatalf("operation = %#v, state = %#v, err = %v", done, state, err)
	}
}

func TestFileOperationAppliesAPatch(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("one\ntwo\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	patch := "*** Begin Patch\n*** Update File: a.txt\n@@\n-two\n+2\n*** End Patch\n"
	manager := operation.NewLocalOperationManager(t.Context())
	applied := newFileOperation(t, "patch", operation.FileState{Action: operation.FilePatch, Patch: patch, Root: root})
	if err := manager.Add(applied); err != nil {
		t.Fatal(err)
	}
	done := receiveTerminalOperation(t, manager.Updates(), applied.ID)
	state, _ := operation.DecodeFileState(done)
	if done.Status != operation.StatusCompleted || state.Result.Text != "Applied the patch:\nM a.txt" {
		t.Fatalf("operation = %#v, state = %#v", done, state)
	}
	if data, _ := os.ReadFile(filepath.Join(root, "a.txt")); string(data) != "one\n2\n" {
		t.Fatalf("a.txt = %q", data)
	}
}

func TestFileSpecValidation(t *testing.T) {
	for _, state := range []operation.FileState{
		{Action: "bogus", Path: "/x"},
		{Action: operation.FileRead},
		{Action: operation.FilePatch},
		{Action: operation.FileRead, Path: "/x", Offset: -1},
	} {
		if _, err := operation.NewFileSpec(state); err == nil {
			t.Fatalf("state %#v was accepted", state)
		}
	}
}

func TestFileOperationDoesNotChangeAFileAgainAfterARestart(t *testing.T) {
	for _, state := range []operation.FileState{
		{Action: operation.FileEdit, Path: "/work/a.txt", OldString: "a", NewString: "ab"},
		{Action: operation.FilePatch, Patch: "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch", Root: "/work"},
	} {
		current := newFileOperation(t, "resumed", state)
		current.Status = operation.StatusAwaiting
		step, err := operation.AdvanceFile(current, nil)
		if err != nil {
			t.Fatal(err)
		}
		decoded, _ := operation.DecodeFileState(*step.Operation)
		if step.Operation.Status != operation.StatusFailed || !strings.Contains(decoded.TerminalError, "interrupted by a restart") || len(step.Dispatches) != 0 {
			t.Fatalf("%s: step = %#v, state = %#v", state.Action, step, decoded)
		}
	}
	// A read is the same done twice: it is done again.
	current := newFileOperation(t, "resumed-read", operation.FileState{Action: operation.FileRead, Path: "/work/a.txt"})
	current.Status = operation.StatusAwaiting
	step, err := operation.AdvanceFile(current, nil)
	if err != nil || step.Operation.Status != operation.StatusAwaiting || len(step.Dispatches) != 1 {
		t.Fatalf("step = %#v, err = %v", step, err)
	}
}
