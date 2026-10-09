package file_test

import (
	"encoding/json/v2"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/file"
)

type recordingContext struct {
	specs []operation.Spec
}

func (ctx *recordingContext) Submit(spec operation.Spec) operation.ID {
	ctx.specs = append(ctx.specs, spec)
	return "operation-1"
}

func translate(t *testing.T, kind file.Kind, arguments string) (tool.CallStatus, operation.FileState) {
	t.Helper()
	translator := file.New(kind, string(kind), file.Config{Directory: "/work", Transcript: "/work/.harness/s.jsonl"})
	ctx := &recordingContext{}
	status := translator.Translate(ctx, llm.ToolCall{CallID: "call", Name: string(kind), Arguments: arguments})
	if status.Error != "" {
		return status, operation.FileState{}
	}
	if len(ctx.specs) != 1 {
		t.Fatalf("specs = %#v", ctx.specs)
	}
	var state operation.FileState
	if err := json.Unmarshal(ctx.specs[0].State, &state); err != nil {
		t.Fatal(err)
	}
	return status, state
}

func TestTranslateBuildsFileOperations(t *testing.T) {
	_, state := translate(t, file.KindRead, `{"path":"a/b.go","offset":10,"limit":5}`)
	if state.Action != operation.FileRead || state.Path != "/work/a/b.go" || state.Offset != 10 || state.Limit != 5 {
		t.Fatalf("state = %#v", state)
	}
	_, state = translate(t, file.KindEdit, `{"file_path":"/abs.go","old_string":"a","new_string":"","replace_all":true}`)
	if state.Action != operation.FileEdit || state.Path != "/abs.go" || state.OldString != "a" || state.NewString != "" || !state.ReplaceAll {
		t.Fatalf("state = %#v", state)
	}
	_, state = translate(t, file.KindWrite, `{"path":"n.txt","content":"x"}`)
	if state.Action != operation.FileWrite || state.Path != "/work/n.txt" || state.Content != "x" {
		t.Fatalf("state = %#v", state)
	}
	_, state = translate(t, file.KindApplyPatch, `{"input":"*** Begin Patch\n*** End Patch"}`)
	if state.Action != operation.FilePatch || state.Root != "/work" || !strings.HasPrefix(state.Patch, "*** Begin") {
		t.Fatalf("state = %#v", state)
	}
	_, state = translate(t, file.KindTranscriptSearch, `{"query":"TestRetry","limit":5}`)
	if state.Action != operation.FileTranscriptSearch || state.Path != "/work/.harness/s.jsonl" || state.Query != "TestRetry" || state.Limit != 5 {
		t.Fatalf("state = %#v", state)
	}
}

func TestTranslateRejectsBadArguments(t *testing.T) {
	for kind, arguments := range map[file.Kind]string{
		file.KindRead: `{}`, file.KindEdit: `{"path":"a","old_string":"x"}`, file.KindWrite: `[]`, file.KindApplyPatch: `{"input":" "}`,
		file.KindTranscriptSearch: `{"query":""}`, "bogus": `{"path":"a"}`,
	} {
		status, _ := translate(t, kind, arguments)
		if status.Error == "" {
			t.Errorf("%s accepted %s", kind, arguments)
		}
	}
	if status, _ := translate(t, file.KindRead, `{"path":"a\u0000b"}`); !strings.Contains(status.Error, "NUL") {
		t.Fatalf("status = %#v", status)
	}
}

func TestTranslateResultReadsTheOperation(t *testing.T) {
	translator := file.New(file.KindRead, "Read", file.Config{})
	result, err := translator.TranslateResult("call", tool.CallStatus{Error: "bad"}, nil)
	if err != nil || result.Output[0].Value != "Error: bad" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
	state := operation.FileState{Action: operation.FileRead, Path: "/a", Result: &operation.FileResult{Text: "     1\tx"}}
	encoded, _ := json.Marshal(state)
	completed := operation.Operation{ID: "op", Type: operation.TypeFile, Version: operation.VersionFile, Status: operation.StatusCompleted, State: encoded}
	result, err = translator.TranslateResult("call", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{completed})
	if err != nil || result.Output[0].Value != "     1\tx" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
	state.Result, state.TerminalError = nil, "/a does not exist"
	encoded, _ = json.Marshal(state)
	failed := completed
	failed.Status, failed.State = operation.StatusFailed, encoded
	result, err = translator.TranslateResult("call", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{failed})
	if err != nil || result.Output[0].Value != "Error: /a does not exist" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
	running := completed
	running.Status = operation.StatusAwaiting
	if result, err = translator.TranslateResult("call", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{running}); err != nil || !strings.Contains(result.Output[0].Value, "still running") {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
}

func TestReadSaysWhenTheSameLinesAreReadAgain(t *testing.T) {
	translator := file.New(file.KindRead, "Read", file.Config{})
	read := func(callID, path, text string) string {
		t.Helper()
		encoded, _ := json.Marshal(operation.FileState{Action: operation.FileRead, Path: path, Result: &operation.FileResult{Text: text}})
		completed := operation.Operation{ID: operation.ID(callID), Type: operation.TypeFile, Version: operation.VersionFile, Status: operation.StatusCompleted, State: encoded}
		result, err := translator.TranslateResult(callID, tool.CallStatus{WaitingFor: []operation.ID{completed.ID}}, []operation.Operation{completed})
		if err != nil {
			t.Fatal(err)
		}
		return result.Output[0].Value
	}
	note := "[harness] These lines of /a.go have now been read 3 times, unchanged since the first."
	for index, call := range []struct{ id, path, text string }{
		{"c1", "/a.go", "     1\tx"}, {"c2", "/b.go", "     1\tx"}, {"c3", "/a.go", "     1\tx"},
	} {
		if text := read(call.id, call.path, call.text); text != call.text {
			t.Fatalf("read %d = %q", index+1, text)
		}
	}
	if text := read("c4", "/a.go", "     1\tx"); !strings.HasPrefix(text, "     1\tx\n\n"+note) {
		t.Fatalf("third read = %q", text)
	}
	// The result of a call reads the same however often it is translated.
	if text := read("c4", "/a.go", "     1\tx"); !strings.Contains(text, note) {
		t.Fatalf("third read again = %q", text)
	}
	// A change to the lines starts the count over.
	if text := read("c5", "/a.go", "     1\ty"); text != "     1\ty" {
		t.Fatalf("changed read = %q", text)
	}
}
