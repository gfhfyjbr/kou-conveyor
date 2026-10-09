package code_test

import (
	"encoding/json/v2"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/code"
)

type recordingContext struct {
	specs []operation.Spec
}

func (ctx *recordingContext) Submit(spec operation.Spec) operation.ID {
	ctx.specs = append(ctx.specs, spec)
	return "operation-1"
}

func TestTranslateBuildsACodeOperationWithTheCurrentTools(t *testing.T) {
	translator := code.New(code.Config{
		Shell: "/bin/bash", Directory: "/work", Transcript: "/t.jsonl",
		Skills: func() map[string]string { return map[string]string{"review": "/s/SKILL.md"} },
		Tools: func() []codevm.CommandTool {
			return []codevm.CommandTool{{Name: "glance", Command: []string{"./glance"}}}
		},
	})
	ctx := &recordingContext{}
	status := translator.Translate(ctx, llm.ToolCall{CallID: "c", Name: "Code", Arguments: `{"code":"return await bash('ls')"}`})
	if status.Error != "" || len(ctx.specs) != 1 || ctx.specs[0].Type != operation.TypeCode {
		t.Fatalf("status = %#v, specs = %#v", status, ctx.specs)
	}
	var state operation.CodeState
	if err := json.Unmarshal(ctx.specs[0].State, &state); err != nil {
		t.Fatal(err)
	}
	if state.Code != "return await bash('ls')" || state.Config.Shell != "/bin/bash" || state.Config.Skills["review"] != "/s/SKILL.md" || state.Config.Tools[0].Name != "glance" || state.Config.Transcript != "/t.jsonl" {
		t.Fatalf("state = %#v", state)
	}
	declarations := code.Declarations(translator)
	if !strings.Contains(declarations, `skill(name: "review")`) || !strings.Contains(declarations, "declare function glance(") {
		t.Fatalf("declarations = %s", declarations)
	}
	for _, bad := range []string{`{}`, `{"code":" "}`, `[]`, `nope`} {
		if status := translator.Translate(&recordingContext{}, llm.ToolCall{CallID: "c", Name: "Code", Arguments: bad}); status.Error == "" {
			t.Fatalf("%s was accepted", bad)
		}
	}
}

func TestTranslateResultShowsTheRun(t *testing.T) {
	translator := code.New(code.Config{Shell: "/bin/sh"})
	now := time.Now()
	result := codevm.Result{Calls: []codevm.Call{{Name: "bash", Arguments: "ls", Output: "a.go", Started: now, Finished: now, Image: "data:image/png;base64,AAAA"}}, Value: "done", Started: now, Finished: now, Done: true}
	state := operation.CodeState{Code: "x", Config: codevm.Config{Shell: "/bin/sh"}, Result: &result}
	encoded, _ := json.Marshal(state)
	completed := operation.Operation{ID: "op", Type: operation.TypeCode, Version: operation.VersionCode, Status: operation.StatusCompleted, State: encoded, MaxOutputLength: 10_000}
	translated, err := translator.TranslateResult("c", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{completed})
	if err != nil {
		t.Fatal(err)
	}
	if len(translated.Output) != 2 || !strings.HasPrefix(translated.Output[0].Value, "[CODE] 1 tool call") || !strings.Contains(translated.Output[0].Value, "- bash: ls") || translated.Output[1].Kind != llm.ToolResultImage {
		t.Fatalf("output = %#v", translated.Output)
	}
	running := completed
	running.Status = operation.StatusAwaiting
	translated, err = translator.TranslateResult("c", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{running})
	if err != nil || !strings.Contains(translated.Output[0].Value, "still running") || !strings.Contains(translated.Output[0].Value, "1 tool calls so far") {
		t.Fatalf("output = %#v, err = %v", translated.Output, err)
	}
	state.TerminalError = "the code run was interrupted"
	encoded, _ = json.Marshal(state)
	failed := completed
	failed.Status, failed.State = operation.StatusFailed, encoded
	translated, err = translator.TranslateResult("c", tool.CallStatus{WaitingFor: []operation.ID{"op"}}, []operation.Operation{failed})
	if err != nil || !strings.Contains(translated.Output[0].Value, "Error: the code run was interrupted") || !strings.Contains(translated.Output[0].Value, "- bash: ls") {
		t.Fatalf("output = %#v, err = %v", translated.Output, err)
	}
}
