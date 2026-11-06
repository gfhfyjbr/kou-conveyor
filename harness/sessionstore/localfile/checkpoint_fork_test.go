package localfile

import (
	"errors"
	"io/fs"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

func TestCheckpointedForkKeepsInheritedOwnershipAndHistoricalGuards(t *testing.T) {
	store := checkpointStore(t)
	ctx := t.Context()
	checkpointMust(t, store.AppendModelResponse(ctx, "session-1", validResponse("turn-1")))
	checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", []byte(`{"generation":1}`)))
	reader, err := New(store.directory)
	checkpointMust(t, err)
	reader.MapLogs()
	_, err = reader.Resume(ctx, "session-1")
	checkpointMust(t, err)
	_, err = reader.Fork(ctx, "fork", "session-1", "turn-1")
	checkpointMust(t, err)
	checkpointMust(t, reader.SaveCheckpoint(ctx, "fork", []byte(`{"generation":1}`)))
	fresh, err := New(store.directory)
	checkpointMust(t, err)
	forked, err := fresh.Resume(ctx, "fork")
	checkpointMust(t, err)
	if len(forked.Operations) != 0 {
		t.Fatalf("fork resumed parent operations: %#v", forked.Operations)
	}
	if err := fresh.AppendToolCallStatus(ctx, "fork", validStatus("turn-1", "parent-late", "operation-1")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("fork accepted late parent status: %v", err)
	}
	if err := fresh.SaveOperation(ctx, "fork", validOperation("operation-1", operation.StatusCompleted)); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("fork accepted parent operation checkpoint: %v", err)
	}
	checkpointMust(t, fresh.AppendTurn(ctx, "fork", session.Turn{ID: "child", PreviousTurnID: "turn-1", Type: session.TurnRegular}))
	status := validStatus("child", "child-call", "operation-1")
	status.Operations = []operation.Operation{validOperation("operation-1", operation.StatusReady)}
	checkpointMust(t, fresh.AppendToolCallStatus(ctx, "fork", status))
	checkpointMust(t, fresh.SaveCheckpoint(ctx, "fork", []byte(`{"generation":2}`)))
	resumed, err := fresh.Resume(ctx, "fork")
	checkpointMust(t, err)
	if len(resumed.Operations) != 1 || resumed.Operations[0].ID != "operation-1" {
		t.Fatalf("child operation ownership = %#v", resumed.Operations)
	}
	if err := reader.AppendModelResponse(ctx, "session-1", validResponse("turn-1")); !errors.Is(err, fs.ErrExist) {
		t.Fatalf("old response guard lost: %v", err)
	}
	duplicate := validStatus("turn-1", "new-call", "operation-1")
	duplicate.Operations = []operation.Operation{validOperation("operation-1", operation.StatusReady)}
	if err := reader.AppendToolCallStatus(ctx, "session-1", duplicate); !errors.Is(err, fs.ErrExist) {
		t.Fatalf("old operation identity guard lost: %v", err)
	}
	changed := validOperation("operation-1", operation.StatusCompleted)
	changed.Version++
	if err := reader.SaveOperation(ctx, "session-1", changed); err == nil {
		t.Fatal("archived operation version change accepted")
	}
}
