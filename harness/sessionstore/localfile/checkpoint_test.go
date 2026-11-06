package localfile

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"io/fs"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

func checkpointStore(t *testing.T) *Store {
	t.Helper()
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, store, strings.Repeat("old output ", 1000))
	return store
}

func checkpointMust(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func checkResumeEquivalent(t *testing.T, store *Store) sessionstore.ResumeState {
	t.Helper()
	encoded, err := os.ReadFile(store.Path("session-1"))
	checkpointMust(t, err)
	full, size, err := decodeLog(encoded)
	checkpointMust(t, err)
	fast, gotSize, err := decodeResumeLog(encoded)
	checkpointMust(t, err)
	want, got := full.resume(), fast.resume()
	checkpoint := got.Checkpoint
	got.Checkpoint = nil
	if gotSize != size || !reflect.DeepEqual(got, want) {
		t.Fatalf("checkpoint replay = %#v (%d bytes), full = %#v (%d bytes)", got, gotSize, want, size)
	}
	got.Checkpoint = checkpoint
	return got
}

func TestResumeLegacyLogsWithoutCheckpoints(t *testing.T) {
	for _, path := range []string{"testdata/golden-session.session.jsonl", "testdata/golden-fork.session.jsonl"} {
		encoded, err := os.ReadFile(path)
		checkpointMust(t, err)
		full, size, err := decodeLog(encoded)
		checkpointMust(t, err)
		fast, gotSize, err := decodeResumeLog(encoded)
		checkpointMust(t, err)
		if size != gotSize || !reflect.DeepEqual(full, fast) || fast.checkpoint != nil {
			t.Fatalf("legacy replay differs for %s", path)
		}
	}
}

func TestCheckpointReplayMultipleCompactionsAndLaterRecords(t *testing.T) {
	store := checkpointStore(t)
	ctx := t.Context()
	checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", jsontext.Value(`{"generation":1}`)))
	checkpointMust(t, store.AppendInput(ctx, "session-1", inbox.Input{ID: "before-second", Kind: inbox.InputExternal, Payload: []byte(`"second question"`)}))
	checkpointMust(t, store.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnCompaction}))
	checkpointMust(t, store.AppendModelResponse(ctx, "session-1", validResponse("turn-2")))
	checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", jsontext.Value(`{"generation":2}`)))
	checkpointMust(t, store.AppendInput(ctx, "session-1", inbox.Input{ID: "late", Kind: inbox.InputExternal, Payload: []byte(`"late question"`)}))
	got := checkResumeEquivalent(t, store)
	if got.Checkpoint == nil || string(got.Checkpoint.State) != `{"generation":2}` || got.Checkpoint.After != 6 || len(got.Checkpoint.Items) != 1 {
		t.Fatalf("resume checkpoint = %#v", got.Checkpoint)
	}
	encoded, err := os.ReadFile(store.Path("session-1"))
	checkpointMust(t, err)
	for line := range bytes.Lines(encoded) {
		if bytes.HasPrefix(line, checkpointPrefix[1:]) && bytes.Contains(line, []byte("old output")) {
			t.Fatal("a checkpoint copied archived output")
		}
	}
	for _, mapped := range []bool{false, true} {
		reader, err := New(store.directory)
		checkpointMust(t, err)
		if mapped {
			reader.MapLogs()
		}
		resumed, err := reader.Resume(ctx, "session-1")
		checkpointMust(t, err)
		if !reflect.DeepEqual(resumed, got) {
			t.Fatalf("store resume (mapped=%v) differs", mapped)
		}
		// An archival read after a fast resume must upgrade its cache, not
		// mistake the short replay tail for the complete history.
		page, err := reader.Items(ctx, "session-1", sessionstore.BeforeFirst, 100)
		checkpointMust(t, err)
		if len(page.Items) != 7 {
			t.Fatalf("archival items = %d", len(page.Items))
		}
		status := page.Items[2].Data.(sessionstore.ToolCallStatus)
		if !bytes.Contains(status.Operations[0].State, []byte("old output")) {
			t.Fatal("archived output disappeared")
		}
		_, err = reader.Fork(ctx, session.ID(map[bool]string{false: "fork-read", true: "fork-mapped"}[mapped]), "session-1", "turn-1")
		checkpointMust(t, err)
		again, err := reader.Resume(ctx, "session-1")
		checkpointMust(t, err)
		if !reflect.DeepEqual(again, got) {
			t.Fatalf("archive cache upgrade disabled checkpoint resume (mapped=%v)", mapped)
		}
		if err := reader.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-1", PreviousTurnID: "turn-2", Type: session.TurnRegular}); !errors.Is(err, fs.ErrExist) {
			t.Fatalf("old duplicate turn accepted: %v", err)
		}
	}
}

func TestCheckpointKeepsPendingOperationsAndLoadsArchivedReferences(t *testing.T) {
	store := checkpointStore(t)
	ctx := t.Context()
	// A terminal checkpoint newer than its last running status must still
	// be reconciled. The completed operations already in history need not be.
	checkpointMust(t, store.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular}))
	pending := validOperation("pending", operation.StatusReady)
	status := validStatus("turn-2", "pending-call", pending.ID)
	status.Operations = []operation.Operation{pending}
	checkpointMust(t, store.AppendToolCallStatus(ctx, "session-1", status))
	pending.Status = operation.StatusCompleted
	pending.State = jsontext.Value(`{"result":"unrecorded completion"}`)
	checkpointMust(t, store.SaveOperation(ctx, "session-1", pending))
	checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", []byte(`{"generation":1}`)))
	got := checkResumeEquivalent(t, store)
	if len(got.Operations) != 2 || !reflect.DeepEqual(got.Operations[1], pending) {
		t.Fatalf("pending = %#v", got.Operations)
	}
	// An ID-only suffix can name a settled operation from before compaction.
	// It is loaded lazily from its original record, including its full output.
	item := sessionstore.Item{Sequence: 6, RecordedAt: time.Now().UTC(), Kind: sessionstore.ItemToolCallStatus, Data: validStatus("turn-1", "call-1", "operation-1", "operation-2")}
	line, err := encodeRecord(recordItem, itemRecord{Item: item, Operations: []ItemOperation{
		{Operation: operation.Operation{ID: "operation-1"}, Saved: true},
		{Operation: operation.Operation{ID: "operation-2"}, Saved: true},
	}})
	checkpointMust(t, err)
	file, err := os.OpenFile(store.Path("session-1"), os.O_WRONLY|os.O_APPEND, 0)
	checkpointMust(t, err)
	_, err = file.Write(line)
	checkpointMust(t, err)
	checkpointMust(t, file.Close())
	got = checkResumeEquivalent(t, store)
	if !bytes.Contains(got.Checkpoint.Items[0].Data.(sessionstore.ToolCallStatus).Operations[0].State, []byte("old output")) {
		t.Fatal("archived reference lost output")
	}
}

func TestCheckpointRecovery(t *testing.T) {
	for _, mode := range []string{"checksum", "version", "offset", "partial-checkpoint", "torn-suffix", "malformed-committed"} {
		t.Run(mode, func(t *testing.T) {
			store := checkpointStore(t)
			ctx := t.Context()
			checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", []byte(`{"generation":1}`)))
			checkpointMust(t, store.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnCompaction}))
			checkpointMust(t, store.AppendModelResponse(ctx, "session-1", validResponse("turn-2")))
			encoded, err := os.ReadFile(store.Path("session-1"))
			checkpointMust(t, err)
			boundary := len(encoded)
			checkpointMust(t, store.SaveCheckpoint(ctx, "session-1", []byte(`{"generation":2}`)))
			encoded, err = os.ReadFile(store.Path("session-1"))
			checkpointMust(t, err)
			wantGeneration := `{"generation":1}`
			switch mode {
			case "checksum", "version", "offset":
				var envelope logRecord
				checkpointMust(t, json.Unmarshal(encoded[boundary:len(encoded)-1], &envelope))
				var checkpoint checkpointRecord
				checkpointMust(t, json.Unmarshal(envelope.Data, &checkpoint))
				if mode == "checksum" {
					checkpoint.SHA256 = strings.Repeat("0", 64)
				}
				if mode == "version" {
					checkpoint.Version++
				}
				if mode == "offset" {
					checkpoint.Offset--
				}
				line, err := encodeRecord(recordCheckpoint, checkpoint)
				checkpointMust(t, err)
				encoded = append(encoded[:boundary], line...)
			case "partial-checkpoint":
				encoded = encoded[:len(encoded)-30]
			case "torn-suffix":
				encoded = append(encoded, []byte(`{"type":"item","data":{"Item":`)...)
				wantGeneration = `{"generation":2}`
			case "malformed-committed":
				encoded = append(encoded, []byte("{broken}\n")...)
			}
			checkpointMust(t, os.WriteFile(store.Path("session-1"), encoded, 0o600))
			if mode == "malformed-committed" {
				if _, _, err := decodeResumeLog(encoded); err == nil {
					t.Fatal("accepted malformed committed suffix")
				}
				return
			}
			got := checkResumeEquivalent(t, store)
			if got.Checkpoint == nil || string(got.Checkpoint.State) != wantGeneration {
				t.Fatalf("recovered checkpoint = %#v", got.Checkpoint)
			}
			reader, err := New(store.directory)
			checkpointMust(t, err)
			reader.MapLogs()
			resumed, err := reader.Resume(ctx, "session-1")
			checkpointMust(t, err)
			checkpointMust(t, reader.AppendInput(ctx, "session-1", inbox.Input{ID: "after-recovery", Kind: inbox.InputExternal, Payload: []byte(`"continue"`)}))
			if !resumed.Checkpoint.State.IsValid() {
				t.Fatal("append invalidated mapped checkpoint state")
			}
			got = checkResumeEquivalent(t, reader)
			if got.ExternalInputIDs[len(got.ExternalInputIDs)-1] != "after-recovery" {
				t.Fatal("append after recovery lost input")
			}
		})
	}
}
