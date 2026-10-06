package tool

import (
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A gateway shows Claude the tools as mcp__<server>__<word>_<Tool>; a model
// that shortens the name still reaches the tool.
func TestRegistryResolvesNamesAGatewayDisguised(t *testing.T) {
	read, patch := &fixedTranslator{}, &fixedTranslator{}
	registry := NewRegistry(StaticTranslators{Read: read, ApplyPatch: patch, Bash: &fixedTranslator{}}, ReadName, ApplyPatchName, BashName)
	for name, want := range map[string]Translator{
		"Read": read, "grace_Read": read, "assault_kidney__grace_Read": read, "mcp__assault_kidney__grace_Read": read,
		"apply_patch": patch, "grace_apply_patch": patch, "mcp__x__y_apply_patch": patch,
	} {
		if got, ok := registry.Resolve(name); !ok || got != want {
			t.Errorf("Resolve(%q) = %v, %t", name, got, ok)
		}
	}
	for _, name := range []string{"read", "Readx", "_Read", "grace_", "grace__", "patch", "grace_patch", ""} {
		if got, ok := registry.Resolve(name); ok {
			t.Errorf("Resolve(%q) = %v, want none", name, got)
		}
	}
}

func TestRegistryRefusesANameThatMayStandForTwoTools(t *testing.T) {
	registry := NewRegistry(StaticTranslators{Read: &fixedTranslator{}}, ReadName)
	if err := registry.RegisterTool(Definition{Tool: llm.Tool{Name: "notes_Read"}}, &fixedTranslator{}); err != nil {
		t.Fatal(err)
	}
	if got := Aliases("x_notes_Read", []string{"Read", "notes_Read"}); !reflect.DeepEqual(got, []string{"Read", "notes_Read"}) {
		t.Fatalf("Aliases = %v", got)
	}
	if _, ok := registry.Resolve("x_notes_Read"); ok {
		t.Fatal("an ambiguous name resolved")
	}
	if message := Unavailable(registry, "x_notes_Read"); !strings.Contains(message, "may stand for any of Read, notes_Read") {
		t.Fatalf("message = %q", message)
	}
	if message := Unavailable(registry, "Grep"); message != `tool "Grep" is not available; the tools are Read, notes_Read` {
		t.Fatalf("message = %q", message)
	}
	if message := Unavailable(NewRegistry(StaticTranslators{}), "Grep"); message != `tool "Grep" is not available` {
		t.Fatalf("message without tools = %q", message)
	}
}
