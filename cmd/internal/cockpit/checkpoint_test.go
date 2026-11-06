package cockpit_test

import (
	"bytes"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/sessionmetrics"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	harness "github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	transcriptsearch "github.com/gfhfyjbr/kou-conveyor/harness/transcript"
)

func checkpointTestMust(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func TestCheckpointRawReadersAndRewindKeepOnlyRetainedHistory(t *testing.T) {
	dir, id := t.TempDir(), harness.ID(uuid.New().String())
	store, err := localfile.New(dir)
	checkpointTestMust(t, err)
	_, err = store.Create(t.Context(), id)
	checkpointTestMust(t, err)
	input := func(idText, text string) {
		checkpointTestMust(t, store.AppendInput(t.Context(), id, inbox.Input{ID: inbox.ID(idText), Kind: inbox.InputExternal, Payload: []byte(`"` + text + `"`)}))
	}
	input("p1", "first prompt")
	checkpointTestMust(t, store.AppendTurn(t.Context(), id, harness.Turn{ID: "t1", Type: harness.TurnRegular}))
	checkpointTestMust(t, store.AppendModelResponse(t.Context(), id, sessionstore.ModelResponse{TurnID: "t1", Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "before checkpoints"}},
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call", Name: "Read", Arguments: `{}`}},
	}}}))
	settled := operation.Operation{ID: "settled", Type: "checkpoint-test", Version: 1, Status: operation.StatusCompleted, State: jsontext.Value(`{"Result":"archive needle before compaction"}`)}
	pending := operation.Operation{ID: "pending", Type: "checkpoint-test", Version: 1, Status: operation.StatusReady}
	checkpointTestMust(t, store.AppendToolCallStatus(t.Context(), id, sessionstore.ToolCallStatus{TurnID: "t1", CallID: "call", Status: tool.CallStatus{WaitingFor: []operation.ID{settled.ID, pending.ID}}, Operations: []operation.Operation{settled, pending}}))
	checkpointTestMust(t, store.SaveOperation(t.Context(), id, settled))
	checkpointTestMust(t, store.SaveCheckpoint(t.Context(), id, []byte(`{"checkpoint_only":"hidden needle","generation":1}`)))
	input("p2", "prompt to remove")
	checkpointTestMust(t, store.AppendTurn(t.Context(), id, harness.Turn{ID: "t2", PreviousTurnID: "t1", Type: harness.TurnRegular}))
	pending.Status, pending.State = operation.StatusCompleted, jsontext.Value(`"late completion after removed prompt"`)
	checkpointTestMust(t, store.SaveOperation(t.Context(), id, pending))
	checkpointTestMust(t, store.AppendToolCallStatus(t.Context(), id, sessionstore.ToolCallStatus{TurnID: "t1", CallID: "call", Status: tool.CallStatus{WaitingFor: []operation.ID{settled.ID, pending.ID}}, Operations: []operation.Operation{settled, pending}}))
	checkpointTestMust(t, store.AppendModelResponse(t.Context(), id, sessionstore.ModelResponse{TurnID: "t2", Response: llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "after checkpoints"}}}}}))
	checkpointTestMust(t, store.SaveCheckpoint(t.Context(), id, []byte(`{"generation":2}`)))
	input("p3", "also removed")
	encoded, err := os.ReadFile(store.Path(id))
	checkpointTestMust(t, err)
	var legacy bytes.Buffer
	for line := range bytes.Lines(encoded) {
		if !bytes.HasPrefix(line, []byte(`{"type":"checkpoint",`)) {
			legacy.Write(line)
		}
	}
	with, err := cockpit.LoadSession(dir, string(id))
	checkpointTestMust(t, err)
	legacyDir := t.TempDir()
	checkpointTestMust(t, os.WriteFile(filepath.Join(legacyDir, filepath.Base(store.Path(id))), legacy.Bytes(), 0o600))
	without, err := cockpit.LoadSession(legacyDir, string(id))
	checkpointTestMust(t, err)
	withContent := *with
	withContent.Size = without.Size // File size includes the acceleration records.
	if !reflect.DeepEqual(&withContent, without) {
		t.Fatal("checkpoint changed raw transcript")
	}
	withMetrics, err := sessionmetrics.Read(bytes.NewReader(encoded), sessionmetrics.Options{})
	checkpointTestMust(t, err)
	withoutMetrics, err := sessionmetrics.Read(bytes.NewReader(legacy.Bytes()), sessionmetrics.Options{})
	checkpointTestMust(t, err)
	if !reflect.DeepEqual(withMetrics, withoutMetrics) || withMetrics.Unreadable != 0 {
		t.Fatal("checkpoint changed session metrics")
	}
	search := func(pattern string) string {
		result, err := transcriptsearch.Search(store.Path(id), pattern, 10)
		checkpointTestMust(t, err)
		return result
	}
	if bytes.Contains([]byte(search("archive needle")), []byte("No match")) {
		t.Fatal("archival search lost old output")
	}
	if !bytes.Contains([]byte(search("hidden needle")), []byte("No match")) {
		t.Fatal("search indexed acceleration state")
	}
	preview, err := cockpit.LoadSessionBefore(dir, string(id), "p2")
	checkpointTestMust(t, err)
	if len(preview.Entries) == 0 {
		t.Fatal("rewind preview lost retained transcript")
	}
	err = cockpit.RewindSession(dir, string(id), "p2")
	checkpointTestMust(t, err)
	reader, err := localfile.New(dir)
	checkpointTestMust(t, err)
	reader.MapLogs()
	resumed, err := reader.Resume(t.Context(), id)
	checkpointTestMust(t, err)
	if resumed.Checkpoint == nil || string(resumed.Checkpoint.State) != `{"checkpoint_only":"hidden needle","generation":1}` {
		t.Fatalf("rewind kept wrong checkpoint: %#v", resumed.Checkpoint)
	}
	if !reflect.DeepEqual(resumed.ExternalInputIDs, []inbox.ID{"p1"}) {
		t.Fatalf("rewind resurrected input IDs: %v", resumed.ExternalInputIDs)
	}
	if len(resumed.Operations) != 1 || !reflect.DeepEqual(resumed.Operations[0], pending) {
		t.Fatalf("rewind lost moved late operation record: %#v", resumed.Operations)
	}
	if bytes.Contains([]byte(search("archive needle")), []byte("No match")) {
		t.Fatal("rewind lost retained output")
	}
	err = cockpit.RewindSession(dir, string(id), "p1")
	checkpointTestMust(t, err)
	reader, err = localfile.New(dir)
	checkpointTestMust(t, err)
	resumed, err = reader.Resume(t.Context(), id)
	checkpointTestMust(t, err)
	if resumed.Checkpoint != nil || len(resumed.ExternalInputIDs) != 0 || len(resumed.Operations) != 0 {
		t.Fatalf("rewind before all checkpoints resurrected state: %#v", resumed)
	}
}
