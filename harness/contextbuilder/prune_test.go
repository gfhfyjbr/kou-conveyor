package contextbuilder

import (
	"encoding/json/jsontext"
	"fmt"
	"slices"
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
	current.SetPruning(PruneOptions{Budget: 12_000, Keep: 2, Bytes: 40})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	for index := range 6 {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}})
		text := big
		if index == 3 {
			text = "head...500 bytes truncated; complete output in /ops/3/out...\n" + big
		}
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: text}}, false)
		current.Commit()
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	// ~24,000 tokens of results and the system prompt: the oldest go until
	// the conversation is a quarter of the budget under it (9,000), but the
	// two latest stay.
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

func TestPruneCutsWhatIsFetchedAgainFirst(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 20_000, Keep: 1, First: func(name string) bool { return name == "CanvasTranscript" }})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	tools := []string{"Bash", "Bash", "CanvasTranscript", "CanvasTranscript", "Bash", "Bash"}
	for index, name := range tools {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: "{}"}}}})
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: big}}, false)
		current.Commit()
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	// ~24,000 tokens: three results go to bring it a quarter of the budget
	// under it — the transcripts, which a call fetches again, before the
	// commands older than them, then the oldest command.
	var pruned []int
	for index := range tools {
		if strings.Contains(resultText(t, result, fmt.Sprintf("call-%d", index)), prunedNote) {
			pruned = append(pruned, index)
		}
	}
	if !slices.Equal(pruned, []int{0, 2, 3}) {
		t.Fatalf("pruned %v, want [0 2 3]", pruned)
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

func TestPruneWaitsForABatchWorthCutting(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 4000, Keep: 1})
	addResults := func(from, to int) {
		for index := from; index < to; index++ {
			id := fmt.Sprintf("call-%d", index)
			current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}})
			current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("x", 1200)}}, false)
			current.Commit()
		}
	}
	// The model's own text takes the conversation past the budget; the one
	// result that may go would save too little to change the cached prefix.
	current.AddModelResponse(answer(strings.Repeat("thinking out loud ", 1000)))
	addResults(0, 2)
	if current.estimate() <= 4000 || current.prune() != 0 {
		t.Fatalf("pruned a batch of one at %d tokens", current.estimate())
	}
	// Five results that may go save enough.
	addResults(2, 6)
	if pruned := current.prune(); pruned != 5 {
		t.Fatalf("pruned %d results", pruned)
	}
}

// thinkingOf is a reasoning item of about tokens tokens.
func thinkingOf(tokens int) llm.Item {
	return llm.Item{Type: llm.ItemReasoning, Data: llm.Reasoning{Raw: jsontext.Value(`"` + strings.Repeat("t", tokens*bytesPerToken) + `"`)}}
}

// TestPruneKeepsTheProviderCount checks that a batch takes what it cuts off
// the provider's count of the latest request instead of forgetting the
// count: summed up again from the items' sizes, the estimate would count the
// thinking the provider cleared and never read, and set a compaction off
// long before it is due.
func TestPruneKeepsTheProviderCount(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 12_000, Keep: 2})
	big := strings.Repeat("output line\n", 1330) // ~4,000 tokens
	for index := range 6 {
		id := fmt.Sprintf("call-%d", index)
		response := llm.Response{Output: []llm.Item{thinkingOf(10_000), {Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}}
		if index == 5 {
			// The provider read the results, not the thinking it cleared.
			response.Usage = llm.Usage{InputTokens: 22_000, OutputTokens: 100}
		}
		current.AddModelResponse(response)
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: big}}, false)
		current.Commit()
	}
	original := slices.Clone(current.committedPrefix)
	before := current.estimate()
	if pruned := current.prune(); pruned != 4 {
		t.Fatalf("pruned %d results at %d tokens", pruned, before)
	}
	var saved int64
	for index, item := range current.committedPrefix {
		saved += estimateItem(original[index]) - estimateItem(item)
	}
	if after := current.estimate(); after != before-saved {
		t.Fatalf("estimated %d after the batch, want %d: %d less than %d", after, before-saved, saved, before)
	}
	// Without ThinkingTurns, the thinking stays.
	if kept := len(slices.DeleteFunc(slices.Clone(current.committedPrefix), func(item llm.Item) bool { return item.Type != llm.ItemReasoning })); kept != 6 {
		t.Fatalf("%d thinking blocks kept of 6", kept)
	}
}

// TestResumedEstimatesWhatCameBack checks that a restored conversation is
// estimated at no less than its size until the next response when its
// builder leaves older thinking out, and by the provider's count otherwise:
// the count was of the conversation as the run before pruned it.
func TestResumedEstimatesWhatCameBack(t *testing.T) {
	for _, turns := range []int{0, 2} {
		current := NewBuilder().(*builder)
		current.SetPruning(PruneOptions{ThinkingTurns: turns})
		current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call", Name: "Bash", Arguments: "{}"}}}})
		current.AddToolResult("call", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("output line\n", 1330)}}, false)
		current.Commit()
		current.AddModelResponse(llm.Response{
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}},
			Usage:  llm.Usage{InputTokens: 500},
		})
		current.Resumed()
		want := int64(500)
		if turns > 0 {
			want = current.toolTokens
			for _, item := range current.committedPrefix {
				want += estimateItem(item)
			}
		}
		if got := current.estimate(); got != want || want < 4_000 && turns > 0 {
			t.Fatalf("thinking turns %d: estimated %d after a restore, want %d", turns, got, want)
		}
		current.AddModelResponse(llm.Response{Usage: llm.Usage{InputTokens: 600}})
		if got := current.estimate(); got != 600 {
			t.Fatalf("thinking turns %d: estimated %d after the next response, want its count", turns, got)
		}
	}
}

// TestPruneLeavesTheThinkingOfOlderTurnsOut checks that a batch takes the
// thinking of all but the latest turns with it, before any result, and
// that the provider's count still covers the items it covered.
func TestPruneLeavesTheThinkingOfOlderTurnsOut(t *testing.T) {
	current := NewBuilder().(*builder)
	current.SetPruning(PruneOptions{Budget: 12_000, Keep: 2, ThinkingTurns: 2})
	result := []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("x", 2_000)}}
	for turn := range 5 {
		addInput(t, current, fmt.Sprintf("task %d", turn))
		current.Commit()
		id := fmt.Sprintf("call-%d", turn)
		response := llm.Response{Output: []llm.Item{thinkingOf(2_000), {Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: "Bash", Arguments: "{}"}}}}
		if turn == 4 {
			response.Usage = llm.Usage{InputTokens: 20_000}
		}
		current.AddModelResponse(response)
		current.AddToolResult(id, result, false)
		current.Commit()
		if turn < 4 {
			current.AddModelResponse(llm.Response{Output: []llm.Item{thinkingOf(2_000), {Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Done."}}}})
		}
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	// The thinking of the three older turns goes, and is enough: no result
	// needs to.
	kept := 0
	for _, item := range built.Request.Input {
		if item.Type == llm.ItemReasoning {
			kept++
		}
	}
	if kept != 3 {
		t.Fatalf("%d thinking blocks kept, want the 3 of the latest two turns", kept)
	}
	for turn := range 5 {
		if strings.Contains(resultText(t, built, fmt.Sprintf("call-%d", turn)), prunedNote) {
			t.Fatalf("call-%d was pruned while the old thinking was enough", turn)
		}
	}
	if last := current.committedPrefix[current.usageMark-1]; last.Type != llm.ItemToolCall || last.Data.(llm.ToolCall).CallID != "call-4" {
		t.Fatalf("the provider's count ends at %#v", last)
	}
	if want := 20_000 - 6*estimateItem(thinkingOf(2_000)); current.usage != want {
		t.Fatalf("the provider's count is %d, want %d", current.usage, want)
	}
	if changes := built.Report.Changes; len(changes) != 1 || !strings.Contains(changes[0].Reason, "6 thinking blocks of older turns were left out") {
		t.Fatalf("changes = %#v", changes)
	}
}

func TestTurnStarts(t *testing.T) {
	message := func(role llm.Role, text string) llm.Item {
		return llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: role, Text: text}}
	}
	call := func(id string) llm.Item { return llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id}} }
	result := func(id string) llm.Item { return llm.Item{Type: llm.ItemToolResult, Data: llm.ToolResult{CallID: id}} }
	thinking := llm.Item{Type: llm.ItemReasoning, Data: llm.Reasoning{}}
	items := []llm.Item{
		message(llm.RoleSystem, "preamble"),
		message(llm.RoleUser, "task"), // 1
		thinking, call("a"), call("b"),
		result("a"), result("b"), // the results of the calls just made carry the turn on
		thinking, call("c"),
		result("c"), // a running call's placeholder
		thinking, message(llm.RoleAssistant, "Waiting."),
		result("c"), // 12: its result, after the response that waited for it
		thinking, call("d"),
		result("d"),
		message(llm.RoleUser, "next"), // 16
	}
	var starts []int
	for index, start := range turnStarts(items) {
		if start {
			starts = append(starts, index)
		}
	}
	if !slices.Equal(starts, []int{1, 12, 16}) {
		t.Fatalf("turns start at %v, want [1 12 16]", starts)
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
