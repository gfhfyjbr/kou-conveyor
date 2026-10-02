package operation_test

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

func TestCodeOperationRunsAndReportsProgress(t *testing.T) {
	directory := t.TempDir()
	picture := filepath.Join(directory, "dot.png")
	file, err := os.Create(picture)
	if err != nil {
		t.Fatal(err)
	}
	canvas := image.NewNRGBA(image.Rect(0, 0, 2, 2))
	canvas.Set(0, 0, color.NRGBA{R: 255, A: 255})
	png.Encode(file, canvas)
	file.Close()

	spec, err := operation.NewCodeSpec(operation.CodeState{
		Code:   "const a = await bash('echo hi'); const img = await viewImage('dot.png'); return a.stdout.trim() + ' ' + img;",
		Config: codevm.Config{Shell: "/bin/sh", Directory: directory, RunTimeout: 20},
	}, 0)
	if err != nil {
		t.Fatal(err)
	}
	current := operation.Operation{ID: "code-1", Type: spec.Type, Version: spec.Version, Status: operation.StatusReady, State: spec.State, MaxOutputLength: spec.MaxOutputLength}
	manager := operation.NewLocalOperationManager(t.Context())
	if err := manager.Add(current); err != nil {
		t.Fatal(err)
	}
	progress := 0
	done := receiveOperation(t, manager.Updates(), current.ID, func(update operation.Operation) bool {
		if update.Status == operation.StatusAwaiting {
			if state, err := operation.DecodeCodeState(update); err == nil && state.Result != nil && len(state.Result.Calls) != 0 {
				progress++
			}
			return false
		}
		return true
	})
	state, err := operation.DecodeCodeState(done)
	if err != nil || done.Status != operation.StatusCompleted || state.Result == nil {
		t.Fatalf("operation = %#v, state = %#v, err = %v", done, state, err)
	}
	if state.Result.Error != "" || !strings.HasPrefix(state.Result.Value, "hi Image attached to the result: 2x2 image/png") {
		t.Fatalf("result = %+v", state.Result)
	}
	if progress == 0 {
		t.Fatal("no progress was reported")
	}
	if images := state.Result.Images(); len(images) != 1 || !strings.HasPrefix(images[0], "data:image/png;base64,") {
		t.Fatalf("images = %v", images)
	}
}

func TestCodeOperationDoesNotRunAgainAfterARestart(t *testing.T) {
	spec, err := operation.NewCodeSpec(operation.CodeState{Code: "return 1", Config: codevm.Config{Shell: "/bin/sh"}}, 0)
	if err != nil {
		t.Fatal(err)
	}
	current := operation.Operation{ID: "code-2", Type: spec.Type, Version: spec.Version, Status: operation.StatusAwaiting, State: spec.State, MaxOutputLength: spec.MaxOutputLength}
	step, err := operation.AdvanceCode(current, nil)
	if err != nil {
		t.Fatal(err)
	}
	state, _ := operation.DecodeCodeState(*step.Operation)
	if step.Operation.Status != operation.StatusFailed || !strings.Contains(state.TerminalError, "restart") || len(step.Dispatches) != 0 {
		t.Fatalf("step = %#v, state = %#v", step, state)
	}
}
