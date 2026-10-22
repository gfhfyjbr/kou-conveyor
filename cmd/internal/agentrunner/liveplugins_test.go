package agentrunner

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/file"
)

// The calls of the tools a run has not resolve as the registry resolves
// those of its own — by the tool's name, or by a name that ends with it —
// and are turned away: those of a plugin's tool saying why, those of a
// built-in tool as the coordinator turns away a call of a tool there is not.
func TestRunToolsResolveTheToolsTheRunHasNot(t *testing.T) {
	directory := t.TempDir()
	translators := tool.StaticTranslators{
		Read: file.New(file.KindRead, tool.ReadName, file.Config{Directory: directory}),
		Edit: file.New(file.KindEdit, tool.EditName, file.Config{Directory: directory}),
	}
	registry := tool.NewRegistry(translators, tool.ReadName)
	tools := runTools{
		Registry: registry,
		live:     &livePlugins{retired: map[string]retiredTool{"CanvasSend": {translator: translators.Edit, refusal: "no canvas"}}},
		builtins: builtinTranslators(translators),
	}
	for _, name := range []string{"Read", "functions_Read"} {
		if translator, exists := tools.Resolve(name); !exists || translator != translators.Read {
			t.Fatalf("%s resolves to %v, %v", name, translator, exists)
		}
	}
	for name, want := range map[string]string{
		"CanvasSend":             "no canvas",
		"default_api_CanvasSend": "no canvas",
		"Edit":                   tool.Unavailable(registry, "Edit"),
		"functions_Edit":         tool.Unavailable(registry, "functions_Edit"),
	} {
		translator, exists := tools.Resolve(name)
		if !exists {
			t.Fatalf("%s does not resolve", name)
		}
		status := translator.Translate(nil, llm.ToolCall{CallID: "call", Name: name, Arguments: `{"path": "a"}`})
		if status.Error != want || len(status.WaitingFor) != 0 {
			t.Fatalf("%s: status = %+v, want the error %q", name, status, want)
		}
		// The call reads as the coordinator gives one of a tool there is
		// not: the error alone.
		result, err := translator.TranslateResult("call", status, nil)
		if err != nil || len(result.Output) != 1 || result.Output[0].Value != want {
			t.Fatalf("%s: result = %+v, %v", name, result, err)
		}
	}
	for _, name := range []string{"Nothing", "functions_Nothing"} {
		if _, exists := tools.Resolve(name); exists {
			t.Fatalf("%s resolves", name)
		}
	}
}
