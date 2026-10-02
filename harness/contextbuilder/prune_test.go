package contextbuilder

import (
	"fmt"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func resultText(t *testing.T, result Result, callID string) string {
	t.Helper()
	for _, item := range result.Request.Input {
		if item.Type == llm.ItemToolResult && item.Data.(llm.ToolResult).CallID == callID {
			return item.Data.(llm.ToolResult).Output[0].Value
		}
	}
	t.Fatalf("no result for %s", callID)
	return ""
}

func TestPruneCutsTheOldestResultsPastTheBudget(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 400, Keep: 2, Bytes: 40})
	big := strings.Repeat("output line\n", 40) // ~120 tokens
	for index := range 6 {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}})
		text := big
		if index == 3 {
			text = "head...500 bytes truncated; complete output in /ops/3/out...tail\n"
		}
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: text}}, false)
		current.Commit()
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	// ~720 tokens of results: the oldest go until half the budget (200)
	// is left, but the two latest stay.
	pruned := 0
	for index := range 6 {
		text := resultText(t, result, fmt.Sprintf("call-%d", index))
		if strings.Contains(text, prunedNote) {
			pruned++
			if index >= 4 {
				t.Fatalf("call-%d, one of the latest, was pruned", index)
			}
			if index == 3 && !strings.Contains(text, "the whole output is in /ops/3/out") {
				t.Fatalf("call-3 lost its output path: %q", text)
			}
			if index == 0 && !strings.HasPrefix(text, "output line\noutput line\n") {
				t.Fatalf("call-0 lost its head: %q", text)
			}
		}
	}
	if pruned != 4 {
		t.Fatalf("pruned %d results", pruned)
	}
	if len(result.Report.Changes) != 1 || !strings.Contains(result.Report.Changes[0].Reason, "4 older tool results were pruned") {
		t.Fatalf("changes = %#v", result.Report.Changes)
	}
	// Pruned once, the prefix stays as it is on the next build.
	again, _ := current.Build()
	if again.Request.Input[2].Data.(llm.ToolResult).Output[0].Value != result.Request.Input[2].Data.(llm.ToolResult).Output[0].Value {
		t.Fatal("a second build changed the prefix")
	}
	if current.prune() != 0 {
		t.Fatal("pruned again without growth")
	}
}

func TestPruneLeavesImagesAsNotesAndSkipsRunningCalls(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 100, Keep: 1})
	current.AddModelResponse(llm.Response{Output: []llm.Item{
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "image", Name: "ViewImage", Arguments: "{}"}},
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "running", Name: "Bash", Arguments: "{}"}},
	}})
	current.AddToolResult("image", []llm.ToolResultOutput{{Kind: llm.ToolResultImage, Value: "data:image/png;base64,AAAA"}, {Kind: llm.ToolResultText, Value: "1x1"}}, false)
	current.AddToolResult("running", nil, true)
	current.Commit()
	current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "late", Name: "Bash", Arguments: "{}"}}}})
	current.AddToolResult("late", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("x", 800)}}, false)
	current.Commit()
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	if text := resultText(t, result, "image"); !strings.HasPrefix(text, "[1 image(s) were pruned") {
		t.Fatalf("image = %q", text)
	}
	if text := resultText(t, result, "running"); text != ToolCallRunningPayload {
		t.Fatalf("running = %q", text)
	}
	if text := resultText(t, result, "late"); strings.Contains(text, prunedNote) {
		t.Fatal("the latest result was pruned while older ones would do")
	}
}

func TestPruneOffByDefault(t *testing.T) {
	current := NewBuilder().(*builder)
	current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "a", Name: "Bash", Arguments: "{}"}}}})
	current.AddToolResult("a", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("x", 100_000)}}, false)
	current.Commit()
	if current.prune() != 0 {
		t.Fatal("pruned without a budget")
	}
}
