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

// countingRegistry counts how many times the tools' definitions are made.
type countingRegistry struct {
	tool.Registry
	made *int
}

func (counting countingRegistry) StaticDefinitions() []tool.Definition {
	*counting.made++
	return counting.Registry.StaticDefinitions()
}

// Each call a session holds of a built-in tool the profile leaves out is
// resolved as the session is restored, to read its result; the refusal of
// a new call, which lists every tool the run has, is made only for one.
func TestRunToolsMakeTheRefusalOnlyForANewCall(t *testing.T) {
	directory := t.TempDir()
	translators := tool.StaticTranslators{
		Read: file.New(file.KindRead, tool.ReadName, file.Config{Directory: directory}),
		Edit: file.New(file.KindEdit, tool.EditName, file.Config{Directory: directory}),
	}
	made := 0
	registry := countingRegistry{Registry: tool.NewRegistry(translators, tool.ReadName), made: &made}
	tools := runTools{Registry: registry, live: &livePlugins{}, builtins: builtinTranslators(translators)}
	for range 3 {
		translator, exists := tools.Resolve("Edit")
		if !exists {
			t.Fatal("Edit does not resolve")
		}
		if _, err := translator.TranslateResult("call", tool.CallStatus{Error: "turned away"}, nil); err != nil {
			t.Fatal(err)
		}
	}
	if made != 0 {
		t.Fatalf("restoring 3 calls made the tools' definitions %d times", made)
	}
	translator, _ := tools.Resolve("Edit")
	want := tool.Unavailable(registry.Registry, "Edit")
	if status := translator.Translate(nil, llm.ToolCall{CallID: "call", Name: "Edit"}); status.Error != want || made == 0 {
		t.Fatalf("status of a new call = %+v, want the error %q", status, want)
	}
}
