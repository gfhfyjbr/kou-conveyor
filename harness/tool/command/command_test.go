package command

import (
	"os/exec"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

type captured struct{ specs []operation.Spec }

func (c *captured) Submit(spec operation.Spec) operation.ID {
	c.specs = append(c.specs, spec)
	return "operation-1"
}

func TestCommandToolRunsWithArgumentsOnStandardInput(t *testing.T) {
	directory := t.TempDir()
	translator, err := New(Config{
		Tool:          llm.Tool{Name: "Echo"},
		Command:       []string{"/bin/sh", "-c", `printf '%s|%s|%s|%s|' "$(cat)" "$KOU_CONVEYOR_TOOL_NAME" "$KOU_CONVEYOR_TOOL_CALL_ID" "$PLUGIN_NOTE"; pwd`, "it's"},
		Directory:     directory,
		BaseDirectory: t.TempDir(),
		Environment:   map[string]string{"PLUGIN_NOTE": "it's $HOME `quoted`"},
	})
	if err != nil {
		t.Fatal(err)
	}
	context := &captured{}
	status := translator.Translate(context, llm.ToolCall{CallID: "call-7", Name: "Echo", Arguments: "{\n  \"text\": \"line\\nKOU_CONVEYOR_TOOL_ARGUMENTS\\n'quote'\"\n}"})
	if status.Error != "" || len(context.specs) != 1 {
		t.Fatalf("status = %+v", status)
	}
	var shell operation.Operation
	shell.Type, shell.Version, shell.State, shell.MaxOutputLength = context.specs[0].Type, context.specs[0].Version, context.specs[0].State, context.specs[0].MaxOutputLength
	state, err := operation.DecodeShellState(shell)
	if err != nil {
		t.Fatal(err)
	}
	if state.Input.Shell != Shell || state.Input.Directory != directory || shell.MaxOutputLength != operation.DefaultMaxOutputLength {
		t.Fatalf("input = %+v", state.Input)
	}
	run := exec.Command(state.Input.Shell, "-c", state.Input.Command)
	run.Dir = directory
	output, err := run.CombinedOutput()
	if err != nil {
		t.Fatalf("%v: %s", err, output)
	}
	resolved, _ := exec.Command("/bin/sh", "-c", "cd "+directory+" && pwd -P").Output()
	want := `{"text":"line\nKOU_CONVEYOR_TOOL_ARGUMENTS\n'quote'"}|Echo|call-7|it's $HOME ` + "`quoted`|"
	if got := string(output); !strings.HasPrefix(got, want) || !strings.Contains(got, strings.TrimSpace(string(resolved))) && !strings.Contains(got, directory) {
		t.Fatalf("output = %q, want %q", got, want)
	}
}

func TestCommandToolRejectsArgumentsThatAreNotAnObject(t *testing.T) {
	translator, err := New(Config{Tool: llm.Tool{Name: "Echo"}, Command: []string{"cat"}, BaseDirectory: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	for _, arguments := range []string{"[1]", "{", `"text"`} {
		status := translator.Translate(&captured{}, llm.ToolCall{CallID: "c", Arguments: arguments})
		if status.Error == "" {
			t.Fatalf("accepted %q", arguments)
		}
		result, err := translator.TranslateResult("c", status, nil)
		if err != nil || !strings.HasPrefix(result.Output[0].Value, "Error: Echo arguments must be a JSON object") {
			t.Fatalf("result = %+v, %v", result, err)
		}
	}
	if status := translator.Translate(&captured{}, llm.ToolCall{CallID: "c"}); status.Error != "" {
		t.Fatalf("empty arguments: %+v", status)
	}
	if _, err := New(Config{Tool: llm.Tool{Name: "Bad"}, Command: []string{"cat"}, Environment: map[string]string{"NOT OK": "x"}}); err == nil {
		t.Fatal("accepted a bad variable name")
	}
}
