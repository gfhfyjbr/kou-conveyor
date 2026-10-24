//go:build unix

package localfile

import (
	"bytes"
	"encoding/json/jsontext"
	"os"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"unsafe"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// within says whether value lies in buffer.
func within(buffer, value []byte) bool {
	start := uintptr(unsafe.Pointer(unsafe.SliceData(buffer)))
	at := uintptr(unsafe.Pointer(unsafe.SliceData(value)))
	return at >= start && at+uintptr(len(value)) <= start+uintptr(len(buffer))
}

// writeFinishedCall writes a session whose tool call has two operations that
// finish, one saved as it finished and one not, and returns the state the
// one saved finished with.
func writeFinishedCall(t *testing.T, store *Store, output string) jsontext.Value {
	t.Helper()
	ctx := t.Context()
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	_, err := store.Create(ctx, "session-1")
	must(err)
	must(store.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-1", Type: session.TurnRegular}))
	first := validOperation("operation-1", operation.StatusReady)
	second := validOperation("operation-2", operation.StatusReady)
	status := sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1",
		Status:     tool.CallStatus{WaitingFor: []operation.ID{first.ID, second.ID}},
		Operations: []operation.Operation{first, second},
	}
	must(store.AppendToolCallStatus(ctx, "session-1", status))
	completed := first
	completed.Status = operation.StatusCompleted
	completed.State = jsontext.Value(`{"output":"` + output + `"}`)
	must(store.SaveOperation(ctx, "session-1", completed))
	failed := second
	failed.Status = operation.StatusFailed
	status.Operations = []operation.Operation{completed, failed}
	must(store.AppendToolCallStatus(ctx, "session-1", status))
	return completed.State
}

// A mapped session file decodes as a read one does, with the operations'
// states the parts of the mapping they are and no room past their ends, and
// reads so still once the mapping's pages are given back.
func TestMappedLogDecodesAsRead(t *testing.T) {
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, store, strings.Repeat("output ", 1000))
	path := store.sessionPath("session-1")
	encoded, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	want, wantSize, err := decodeLog(encoded)
	if err != nil {
		t.Fatal(err)
	}
	mapped, held, err := mapFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if held == nil {
		t.Fatal("the session file was read, not mapped")
	}
	defer held.close()
	got, gotSize, err := decodeLog(mapped, aliasing(mapped))
	if err != nil {
		t.Fatal(err)
	}
	held.drop()
	if gotSize != wantSize || !reflect.DeepEqual(got, want) {
		t.Fatalf("mapped log = %#v (%d bytes), read one = %#v (%d bytes)", got, gotSize, want, wantSize)
	}
	if !bytes.Equal(mapped, encoded) {
		t.Fatal("the mapping no longer reads as the file once its pages are given back")
	}
	states := func(state storedState) []jsontext.Value {
		var values []jsontext.Value
		for _, value := range state.Operations {
			values = append(values, value.State)
		}
		for _, item := range state.Items {
			if status, ok := item.Data.(sessionstore.ToolCallStatus); ok {
				for _, value := range status.Operations {
					values = append(values, value.State)
				}
			}
		}
		return values
	}
	if values := states(got); len(values) != 6 {
		t.Fatalf("states = %q", values)
	}
	for _, value := range states(got) {
		if !within(mapped, value) || cap(value) != len(value) {
			t.Fatalf("state %s is not the mapping's own", value)
		}
	}
	// Bytes of no mapping are copied.
	copied, _, err := decodeLog(encoded, aliasing(mapped))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(copied, want) {
		t.Fatalf("log decoded with the option = %#v, without = %#v", copied, want)
	}
	for _, value := range states(copied) {
		if within(encoded, value) || within(mapped, value) {
			t.Fatalf("state %s is not a copy", value)
		}
	}
}

// A store that maps session files keeps what tools output in the files'
// pages: reading a session allocates a small part of its size.
func TestStoreMappingLogsKeepsOutputsInTheFile(t *testing.T) {
	directory := t.TempDir()
	writer, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, writer, strings.Repeat("output ", 1<<20))
	info, err := os.Stat(writer.sessionPath("session-1"))
	if err != nil {
		t.Fatal(err)
	}
	allocated := func(mapLogs bool) uint64 {
		t.Helper()
		store, err := New(directory)
		if err != nil {
			t.Fatal(err)
		}
		if mapLogs {
			store.MapLogs()
		}
		var before, after runtime.MemStats
		runtime.ReadMemStats(&before)
		page, err := store.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
		runtime.ReadMemStats(&after)
		if err != nil || len(page.Items) != 3 {
			t.Fatalf("items = %d, %v", len(page.Items), err)
		}
		return after.TotalAlloc - before.TotalAlloc
	}
	size := uint64(info.Size())
	if read := allocated(false); read < size {
		t.Fatalf("reading a session of %d bytes allocated %d", size, read)
	}
	if mapped := allocated(true); mapped > size/8 {
		t.Fatalf("reading a mapped session of %d bytes allocated %d", size, mapped)
	}
}

// What was decoded from a mapping may be decoded into again, in place, as
// jsontext.Value.UnmarshalJSON and json.RawMessage do: the write goes to a
// copy of the page, and the file keeps what it had. Giving the mapping's
// pages back keeps the copy, and what was written to it.
func TestMappedLogTakesWritesInPlace(t *testing.T) {
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	state := writeFinishedCall(t, store, strings.Repeat("output ", 1000))
	path := store.sessionPath("session-1")
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	mapped, held, err := mapFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if held == nil {
		t.Fatal("the session file was read, not mapped")
	}
	defer held.close()
	decoded, _, err := decodeLog(mapped, aliasing(mapped))
	if err != nil {
		t.Fatal(err)
	}
	position, ok := decoded.operationPositions["operation-1"]
	value := decoded.Operations[position].State
	if !ok || !bytes.Equal(value, state) || !within(mapped, value) {
		t.Fatalf("decoded state %.80s…", value)
	}
	const written = `{"output":"over"}`
	if err := value.UnmarshalJSON([]byte(written)); err != nil {
		t.Fatal(err)
	}
	if !within(mapped, value) || string(value) != written {
		t.Fatalf("UnmarshalJSON no longer writes in place, which this test is about: %s", value)
	}
	held.drop()
	want := bytes.Clone(before)
	copy(want[uintptr(unsafe.Pointer(unsafe.SliceData(value)))-uintptr(unsafe.Pointer(unsafe.SliceData(mapped))):], written)
	if !bytes.Equal(mapped, want) {
		t.Fatalf("once its pages are given back the mapping reads %q where %q was written", value, written)
	}
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(after, before) {
		t.Fatal("a write to a value decoded from the mapping reached the file")
	}
}

// A run that died as it wrote a record left part of one past the last. The
// store cuts it as it appends, which leaves what it decoded from a mapping
// of the file as it was.
func TestMappedLogOutlivesItsCutTail(t *testing.T) {
	directory := t.TempDir()
	writer, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	state := writeFinishedCall(t, writer, strings.Repeat("output ", 10000))
	path := writer.sessionPath("session-1")
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	committed := info.Size()
	file, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		t.Fatal(err)
	}
	_, err = file.WriteString(`{"type":"operation","data":{"Operation":{"State":"` + strings.Repeat("cut ", 1<<15))
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		t.Fatal(err)
	}

	store, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	store.MapLogs()
	page, err := store.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	if err := store.AppendTurn(t.Context(), "session-1", session.Turn{
		ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular,
	}); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(path); err != nil || info.Size() <= committed || info.Size() >= committed+1<<15 {
		t.Fatalf("file after the append: %v, %v; %d bytes were committed", info, err, committed)
	}
	for _, index := range []int{1, 2} {
		status := page.Items[index].Data.(sessionstore.ToolCallStatus)
		if index == 2 && !bytes.Equal(status.Operations[0].State, state) {
			t.Fatalf("the finished state reads as %.80s…", status.Operations[0].State)
		}
		for _, value := range status.Operations {
			if !value.State.IsValid() {
				t.Fatalf("state %.80s… is not valid", value.State)
			}
		}
	}
	fresh, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	fresh.MapLogs()
	reread, err := fresh.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(reread.Items) != 4 || !reflect.DeepEqual(reread.Items[:3], page.Items) {
		t.Fatalf("items read again = %#v, before = %#v", reread.Items, page.Items)
	}
}

// The raw JSON values items hold, as a model's reasoning in its provider's
// own form and its usage, decode from a mapped session file as the parts of
// the mapping they are, as operations' states do.
func TestMappedLogDecodesItemsRawValuesInPlace(t *testing.T) {
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	ctx := t.Context()
	if _, err := store.Create(ctx, "session-1"); err != nil {
		t.Fatal(err)
	}
	if err := store.AppendTurn(ctx, "session-1", session.Turn{ID: "turn-1", Type: session.TurnRegular}); err != nil {
		t.Fatal(err)
	}
	if err := store.AppendModelResponse(ctx, "session-1", sessionstore.ModelResponse{
		TurnID: "turn-1",
		Response: llm.Response{
			Output: []llm.Item{
				{Type: llm.ItemReasoning, Data: llm.Reasoning{
					Summary: []string{"inspect"},
					Raw:     jsontext.Value(`{"encrypted_content":"` + strings.Repeat("opaque", 1000) + `"}`),
				}},
				{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}},
			},
			Usage: llm.Usage{InputTokens: 12, OutputTokens: 3, Raw: jsontext.Value(`{"input_tokens":12,"output_tokens":3}`)},
		},
	}); err != nil {
		t.Fatal(err)
	}
	path := store.sessionPath("session-1")
	encoded, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	want, _, err := decodeLog(encoded)
	if err != nil {
		t.Fatal(err)
	}
	mapped, held, err := mapFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if held == nil {
		t.Fatal("the session file was read, not mapped")
	}
	defer held.close()
	got, _, err := decodeLog(mapped, aliasing(mapped))
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("mapped log = %#v, read one = %#v", got, want)
	}
	response := got.Items[len(got.Items)-1].Data.(sessionstore.ModelResponse).Response
	for _, value := range []jsontext.Value{response.Output[0].Data.(llm.Reasoning).Raw, response.Usage.Raw} {
		if len(value) == 0 || !within(mapped, value) || cap(value) != len(value) {
			t.Fatalf("raw value %.80s is not the mapping's own", value)
		}
	}
}

// openFiles is how many files the process has open.
func openFiles(t *testing.T) int {
	t.Helper()
	entries, err := os.ReadDir("/dev/fd")
	if err != nil {
		t.Fatal(err)
	}
	return len(entries)
}

// A store that maps session files holds a session's file open as long as it
// holds what it decoded from it, to give the file's pages back as it lets
// go of it (mapping), and no longer: not once it has decoded the file again
// as it changed, nor once it has appended, nor when it only inspected the
// session or could not read it.
func TestMappedStoreClosesTheFilesItLetsGo(t *testing.T) {
	directory := t.TempDir()
	writer, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, writer, "output")
	store, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	store.MapLogs()
	ctx := t.Context()
	start := openFiles(t)
	expect := func(what string, want int) {
		t.Helper()
		if open := openFiles(t); open != start+want {
			t.Fatalf("%s, %d files are open, %d before; want %d more", what, open, start, want)
		}
	}
	if _, err := store.Inspect(ctx, "session-1"); err != nil {
		t.Fatal(err)
	}
	expect("once the session was inspected", 0)
	if _, err := store.Items(ctx, "session-1", sessionstore.BeforeFirst, 10); err != nil {
		t.Fatal(err)
	}
	expect("as the session's items are held", 1)
	if err := writer.AppendTurn(ctx, "session-1", session.Turn{
		ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular,
	}); err != nil {
		t.Fatal(err)
	}
	if page, err := store.Items(ctx, "session-1", sessionstore.BeforeFirst, 10); err != nil || len(page.Items) != 4 {
		t.Fatalf("items = %d, %v", len(page.Items), err)
	}
	expect("as the items of the session decoded again are held", 1)
	if err := store.AppendTurn(ctx, "session-1", session.Turn{
		ID: "turn-3", PreviousTurnID: "turn-2", Type: session.TurnRegular,
	}); err != nil {
		t.Fatal(err)
	}
	expect("once the store appended to the session", 0)
	encoded, err := os.ReadFile(store.sessionPath("session-1"))
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(store.sessionPath("session-2"), encoded, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Items(ctx, "session-2", sessionstore.BeforeFirst, 10); err == nil ||
		!strings.Contains(err.Error(), `file contains session "session-1"`) {
		t.Fatalf("items of a file of another session: %v", err)
	}
	expect("once the store failed to read a session", 0)
}
