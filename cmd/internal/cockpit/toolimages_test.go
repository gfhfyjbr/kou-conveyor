package cockpit_test

import (
	"bytes"
	"encoding/base64"
	"encoding/json/v2"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// viewed returns the ViewImage call of a transcript.
func viewed(t *testing.T, tr *cockpit.Transcript) *cockpit.Entry {
	t.Helper()
	for _, e := range tr.Entries {
		if e.Kind == cockpit.KindTool && e.Tool.Name == "ViewImage" {
			return e
		}
	}
	t.Fatal("no ViewImage call")
	return nil
}

func TestViewImageCallsShowThePictureTheyRead(t *testing.T) {
	sessions, id := t.TempDir(), uuid.New().String()
	live := cockpit.NewTranscript()
	drain(t, start(t, t.Context(), sessions, id, "look at the screen"), live)
	want := cockpit.ImageInfo{Label: "screen.png", MediaType: "image/png", Width: 480, Height: 320, Size: len(cockpittest.Picture())}
	e := viewed(t, live)
	if e.Tool.State != cockpit.ToolDone || e.Tool.Image == nil || *e.Tool.Image != want || e.Tool.Output != "480×320 image/png" {
		t.Fatalf("live call = %+v, image %+v", e.Tool, e.Tool.Image)
	}
	// The transcript has the bytes, as the model got them.
	if img, ok := e.Tool.Picture(); !ok || img.MediaType != "image/png" || img.Label != "screen.png" || !bytes.Equal(img.Data, cockpittest.Picture()) {
		t.Fatalf("picture = %s %q (%d bytes)", img.MediaType, img.Label, len(img.Data))
	}
	// So does a transcript read from the file, and the file itself.
	loaded, err := cockpit.LoadSession(sessions, id)
	if err != nil {
		t.Fatal(err)
	}
	if e := viewed(t, loaded); e.Tool.Image == nil || *e.Tool.Image != want {
		t.Fatalf("loaded call = %+v", e.Tool)
	}
	img, err := cockpit.ToolImage(sessions, id, e.Tool.CallID)
	if err != nil || img.MediaType != "image/png" || !bytes.Equal(img.Data, cockpittest.Picture()) {
		t.Fatalf("tool image = %s (%d bytes), %v", img.MediaType, len(img.Data), err)
	}
	// Other calls read none.
	drain(t, start(t, t.Context(), sessions, id, "use a tool"), live)
	var bash *cockpit.Entry
	for _, e := range live.Entries {
		if e.Kind == cockpit.KindTool && e.Tool.Name == "Bash" {
			bash = e
		}
	}
	if bash == nil || bash.Tool.Image != nil {
		t.Fatalf("Bash call = %+v", bash)
	}
	if _, ok := bash.Tool.Picture(); ok {
		t.Fatal("a Bash call has a picture")
	}
	for _, call := range []string{bash.Tool.CallID, "call-nobody-made"} {
		if _, err := cockpit.ToolImage(sessions, id, call); !errors.Is(err, fs.ErrNotExist) {
			t.Errorf("picture of %s: %v", call, err)
		}
	}
	if _, err := cockpit.ToolImage(sessions, "../escape", e.Tool.CallID); err == nil {
		t.Fatal("an invalid session was read")
	}
	if _, err := cockpit.ToolImage(sessions, uuid.New().String(), e.Tool.CallID); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("picture of a missing session: %v", err)
	}
}

// sessionRecord is one line of a session file.
func sessionRecord(t *testing.T, recordType string, data any) []byte {
	t.Helper()
	line, err := json.Marshal(map[string]any{"type": recordType, "data": data})
	if err != nil {
		t.Fatal(err)
	}
	return append(line, '\n')
}

func viewOperation(t *testing.T, id operation.ID, status operation.Status, result *operation.ViewImageResult) operation.Operation {
	t.Helper()
	state, err := json.Marshal(operation.ViewImageState{
		Path: "/tmp/shots/after.png", Config: operation.ViewImageConfig{MaxSize: 1000, MaxWidth: 10, MaxHeight: 10}, Result: result,
	})
	if err != nil {
		t.Fatal(err)
	}
	return operation.Operation{ID: id, Type: operation.TypeViewImage, Version: operation.VersionViewImage, Status: status, State: state}
}

func statusRecord(t *testing.T, sequence int, call string, operations ...operation.Operation) []byte {
	t.Helper()
	var waiting []operation.ID
	for _, value := range operations {
		waiting = append(waiting, value.ID)
	}
	return sessionRecord(t, "item", map[string]any{
		"Item": sessionstore.Item{
			Sequence: sessionstore.Sequence(sequence), RecordedAt: time.Now().UTC(), Kind: sessionstore.ItemToolCallStatus,
			Data: sessionstore.ToolCallStatus{TurnID: "turn-1", CallID: call, Status: tool.CallStatus{WaitingFor: waiting}},
		},
		"Operations": operations,
	})
}

func TestToolImageFollowsTheFile(t *testing.T) {
	sessions, id := t.TempDir(), uuid.New().String()
	path := filepath.Join(sessions, id+".session.jsonl")
	picture := cockpittest.Picture()
	done := &operation.ViewImageResult{
		Content: base64.StdEncoding.EncodeToString(picture), OriginalWidth: 4800, OriginalHeight: 3200,
		OriginalMIMEType: "image/png", EncodedMIMEType: "image/png", ScaleRatio: 0.1,
	}
	write := func(lines ...[]byte) {
		file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		file.Write(bytes.Join(lines, nil))
	}
	write(
		sessionRecord(t, "session", map[string]any{"Version": 2}),
		statusRecord(t, 1, "call-a", viewOperation(t, "op-a", operation.StatusReady, nil)),
	)
	if _, err := cockpit.ToolImage(sessions, id, "call-a"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("picture of a call still reading: %v", err)
	}
	// The picture of a run that crashed before the call's last status is in
	// the operation's checkpoint; a record still being written is not read.
	checkpoint := sessionRecord(t, "operation", map[string]any{"Operation": viewOperation(t, "op-a", operation.StatusCompleted, done)})
	write(checkpoint[:len(checkpoint)/2])
	if _, err := cockpit.ToolImage(sessions, id, "call-a"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("picture of half a record: %v", err)
	}
	write(checkpoint[len(checkpoint)/2:])
	img, err := cockpit.ToolImage(sessions, id, "call-a")
	if err != nil || !bytes.Equal(img.Data, picture) || img.Label != "after.png" {
		t.Fatalf("picture from the checkpoint = %q (%d bytes), %v", img.Label, len(img.Data), err)
	}
	loaded, err := cockpit.LoadSession(sessions, id)
	if err != nil {
		t.Fatal(err)
	}
	// It describes the picture as the model got it, not the file it came
	// from.
	if e := loaded.Entry("tool:call-a"); e == nil || e.Tool.Image == nil || e.Tool.Image.Width != 480 || e.Tool.Output != "4800×3200 image/png" {
		t.Fatalf("loaded call = %+v", e)
	}
	// A call that failed has no picture.
	write(
		statusRecord(t, 2, "call-b", viewOperation(t, "op-b", operation.StatusFailed, &operation.ViewImageResult{Error: "not an image"})),
		statusRecord(t, 3, "call-c", viewOperation(t, "op-c", operation.StatusCompleted, done)),
	)
	if _, err := cockpit.ToolImage(sessions, id, "call-b"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("picture of a failed call: %v", err)
	}
	if img, err := cockpit.ToolImage(sessions, id, "call-c"); err != nil || !bytes.Equal(img.Data, picture) {
		t.Fatalf("picture of a later call: %v", err)
	}
	// A rewound session is another file, read anew.
	replacement := path + ".new"
	if err := os.WriteFile(replacement, statusRecord(t, 1, "call-d", viewOperation(t, "op-d", operation.StatusCompleted, done)), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(replacement, path); err != nil {
		t.Fatal(err)
	}
	if _, err := cockpit.ToolImage(sessions, id, "call-a"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("picture of a call the rewind took away: %v", err)
	}
	if img, err := cockpit.ToolImage(sessions, id, "call-d"); err != nil || !bytes.Equal(img.Data, picture) {
		t.Fatalf("picture after the rewind: %v", err)
	}
}

func TestValidCallID(t *testing.T) {
	for _, id := range []string{"call-1", "toolu_01JTCyurFezfXA6SqFnQGz6u", "call_abc|fc_1"} {
		if !cockpit.ValidCallID(id) {
			t.Errorf("%q is invalid", id)
		}
	}
	for _, id := range []string{"", "a\x00b", "\x1b[31m", string(make([]byte, 300)), "\xff"} {
		if cockpit.ValidCallID(id) {
			t.Errorf("%q is valid", id)
		}
	}
}
