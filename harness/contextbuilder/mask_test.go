package contextbuilder

import (
	"fmt"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestMaskCutsTheResultsBeyondTheWindow(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetMasking(MaskOptions{Results: 2})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	for index := range 5 {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}})
		text := big
		if index == 1 {
			text = "head...500 bytes truncated; complete output in /ops/1/out...\n" + big
		}
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: text}}, false)
		current.Commit()
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	masked := 0
	for index := range 5 {
		text := resultText(t, result, fmt.Sprintf("call-%d", index))
		if strings.Contains(text, maskedNote) {
			masked++
			if index >= 3 {
				t.Fatalf("call-%d, one of the latest two, was masked", index)
			}
			if index == 1 && !strings.Contains(text, "the whole output is in /ops/1/out") {
				t.Fatalf("call-1 lost its output path: %q", text)
			}
			if strings.Contains(text, "output line") {
				t.Fatalf("call-%d kept its output: %q", index, text[:80])
			}
		}
	}
	if masked != 3 {
		t.Fatalf("masked %d results, want 3", masked)
	}
	if len(result.Report.Changes) != 1 || !strings.Contains(result.Report.Changes[0].Reason, "3 tool results beyond the latest 2 were masked") {
		t.Fatalf("changes = %#v", result.Report.Changes)
	}
	// Masked once, the prefix stays as it is on the next build.
	again, _ := current.Build()
	if again.Request.Input[2].Data.(llm.ToolResult).Output[0].Value != result.Request.Input[2].Data.(llm.ToolResult).Output[0].Value {
		t.Fatal("a second build changed the prefix")
	}
	if current.mask() != 0 {
		t.Fatal("masked again without new results")
	}
	// One more result, and the one that left the window is masked.
	current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-5", Name: "Bash", Arguments: "{}"}}}})
	current.AddToolResult("call-5", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: big}}, false)
	current.Commit()
	if n := current.mask(); n != 1 {
		t.Fatalf("masked %d results as one left the window, want 1", n)
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	if text := resultText(t, built, "call-3"); !strings.Contains(text, maskedNote) {
		t.Fatalf("call-3 stayed whole beyond the window: %q", text[:80])
	}
	if text := resultText(t, built, "call-5"); strings.Contains(text, maskedNote) {
		t.Fatal("the latest result was masked")
	}
}

func TestMaskLeavesImagesAsNotesAndSkipsRunningCalls(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetMasking(MaskOptions{Results: 1})
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
	if text := resultText(t, result, "image"); !strings.HasPrefix(text, "[1 image(s) were masked from the context") || !strings.Contains(text, maskedNote) {
		t.Fatalf("image = %q", text)
	}
	if text := resultText(t, result, "running"); text != ToolCallRunningPayload {
		t.Fatalf("running = %q", text)
	}
	if text := resultText(t, result, "late"); strings.Contains(text, maskedNote) {
		t.Fatal("the latest result was masked while older ones would do")
	}
}

// TestMaskKeepsTheProviderCount checks that a batch takes what it masks off
// the provider's count of the latest request, as a prune batch does, instead
// of forgetting the count.
func TestMaskKeepsTheProviderCount(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetMasking(MaskOptions{Results: 2})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	for index := range 5 {
		id := fmt.Sprintf("call-%d", index)
		response := llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}}
		if index == 4 {
			response.Usage = llm.Usage{InputTokens: 22_000, OutputTokens: 100}
		}
		current.AddModelResponse(response)
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: big}}, false)
		current.Commit()
	}
	original := slices.Clone(current.committedPrefix)
	before := current.estimate()
	if masked := current.mask(); masked != 3 {
		t.Fatalf("masked %d results at %d tokens", masked, before)
	}
	var saved int64
	for index, item := range current.committedPrefix {
		saved += estimateItem(original[index]) - estimateItem(item)
	}
	if after := current.estimate(); after != before-saved {
		t.Fatalf("estimated %d after the batch, want %d: %d less than %d", after, before-saved, saved, before)
	}
}

// TestMaskComposesWithPruning checks that masking runs ahead of pruning: a
// result beyond the window is masked outright, and pruning skips the masked
// and cuts what the budget still asks for.
func TestMaskComposesWithPruning(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetMasking(MaskOptions{Results: 2})
	current.SetPruning(PruneOptions{Budget: 8_000, Keep: 1, Bytes: 40})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	for index := range 3 {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}})
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: big}}, false)
		current.Commit()
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	if text := resultText(t, result, "call-0"); !strings.Contains(text, maskedNote) || strings.Contains(text, prunedNote) {
		t.Fatalf("call-0 = %q, want masked and not pruned", text[:80])
	}
	if text := resultText(t, result, "call-1"); !strings.Contains(text, prunedNote) {
		t.Fatalf("call-1 = %q, want pruned: the budget still asked for more", text[:80])
	}
	if text := resultText(t, result, "call-2"); strings.Contains(text, prunedNote) || strings.Contains(text, maskedNote) {
		t.Fatalf("call-2, the latest, was cut: %q", text[:80])
	}
}

func TestMaskOffByDefault(t *testing.T) {
	current := NewBuilder().(*builder)
	current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "a", Name: "Bash", Arguments: "{}"}}}})
	current.AddToolResult("a", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("x", 100_000)}}, false)
	current.Commit()
	if current.mask() != 0 {
		t.Fatal("masked without a window")
	}
}
