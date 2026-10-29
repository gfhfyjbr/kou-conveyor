package contextbuilder

import (
	"fmt"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Pruning. The results of the tools are most of a long conversation, and
// most of them are read once: a file listed, a test run, a search. Once the
// conversation outgrows a budget, the oldest results beyond it are cut to a
// note that says what was there and, for a shell command, where the whole
// output still is; the model can read it again if it needs it, and the
// transcript search finds it. Results that show what is kept elsewhere — a
// transcript read, a search — go before the others (PruneOptions.First).
// Pruning changes the committed conversation in place, in batches, so the
// prefix the provider caches changes once a batch rather than a little
// every turn. The latest turns are never pruned: what the model just asked
// for stays in view.
//
// A Claude model keeps the thinking of every turn in its context and reads
// it again on every request. A batch takes the thinking of all but the
// latest turns with it (PruneOptions.ThinkingTurns): left out at once, it
// changes the cached prefix where the batch changes it anyway, rather than
// at every turn, as the API's own clearing of older turns would.

// PruneOptions say when and how the conversation is pruned.
type PruneOptions struct {
	// Budget is the estimated tokens the conversation may take before the
	// oldest tool results are pruned; zero prunes nothing.
	Budget int64
	// Keep is how many of the latest tool results are never pruned.
	Keep int
	// Bytes is what a result is cut to when it is pruned: its head, before
	// the note. Zero keeps nothing of it.
	Bytes int
	// First tells the tools whose results are pruned before the others',
	// oldest first: those that show what is kept elsewhere — another
	// agent's transcript, a search — which a call fetches again. Nil prunes
	// every result by its age alone.
	First func(tool string) bool
	// ThinkingTurns is how many of the latest turns with thinking keep it
	// when a batch is pruned; the thinking of older turns goes first, and
	// counts toward the batch. Zero keeps all thinking.
	ThinkingTurns int
}

// prunedNote marks a pruned result.
const prunedNote = "[The rest of this result was pruned from the context to save space"

// prunedResult is the text of a result pruned to bytes, followed by the
// note.
func prunedResult(text string, bytes int, paths string) string {
	head := ""
	if bytes > 0 && len(text) > 0 {
		head = clip(text, bytes)
		if head == text {
			return text
		}
		head, _, _ = strings.Cut(head, "\n[… ")
		head = strings.TrimRight(head, "\n") + "\n"
	}
	note := prunedNote
	if paths != "" {
		note += "; the whole output is in " + paths
	}
	return head + note + "; TranscriptSearch finds it too.]"
}

// isPruned reports a result that was pruned.
func isPruned(output []llm.ToolResultOutput) bool {
	return len(output) == 1 && output[0].Kind == llm.ToolResultText && strings.Contains(output[0].Value, prunedNote)
}

// SetPruning sets how the conversation is pruned; Build prunes before it
// builds a request.
func (current *builder) SetPruning(options PruneOptions) {
	current.pruning = options
}

// prune cuts the oldest tool results of the committed conversation down
// once the whole conversation is estimated at more than the budget: the
// model's own text and reasoning grow it too, and only the results, and the
// thinking of older turns, can be cut. The old thinking goes first, then
// results from the oldest on, until the estimate is a quarter of the budget
// lower, leaving the latest results alone. A batch that would save less
// than an eighth of the budget waits until there is more to cut: otherwise
// every turn would cut the one result that left the latest ones, and the
// prefix the provider caches would change every turn. It reports how many
// results it pruned.
func (current *builder) prune() int {
	options := current.pruning
	if options.Budget <= 0 {
		return 0
	}
	estimated := current.estimate()
	if estimated <= options.Budget {
		return 0
	}
	// saved is what the batch takes off the estimate; counted is the part
	// of it that the provider's count of the latest request holds.
	var saved, counted int64
	save := func(index int, saving int64) {
		saved += saving
		if index < current.usageMark {
			counted += saving
		}
	}
	thinking := current.oldThinking(options.ThinkingTurns)
	for _, index := range thinking {
		save(index, estimateItem(current.committedPrefix[index]))
	}
	var results []int
	for index, item := range current.committedPrefix {
		if item.Type != llm.ItemToolResult {
			continue
		}
		result := item.Data.(llm.ToolResult)
		if isPruned(result.Output) || isRunning(result.Output) {
			continue
		}
		results = append(results, index)
	}
	results = results[:max(len(results)-options.Keep, 0)]
	if options.First != nil {
		// What a call fetches again goes first; each part stays oldest first.
		first := func(index int) bool {
			return options.First(current.calls[current.committedPrefix[index].Data.(llm.ToolResult).CallID].Name)
		}
		results = append(slices.DeleteFunc(slices.Clone(results), func(index int) bool { return !first(index) }),
			slices.DeleteFunc(results, first)...)
	}
	target := options.Budget - options.Budget/4
	cut := make(map[int]llm.Item)
	for _, index := range results {
		if estimated-saved <= target {
			break
		}
		item := current.committedPrefix[index]
		pruned := pruneItem(item, options.Bytes)
		if saving := estimateItem(item) - estimateItem(pruned); saving > 0 {
			cut[index] = pruned
			save(index, saving)
		}
	}
	if saved < options.Budget/8 {
		return 0
	}
	for index, item := range cut {
		current.committedPrefix[index] = item
	}
	current.leaveOut(thinking)
	current.uncount(counted)
	current.pruned += len(cut)
	current.thinkingLeftOut += len(thinking)
	return len(cut)
}

// uncount takes what was cut from the items the provider counted for the
// latest request off that count. Forgetting the count instead would make
// the next estimate add up every item again, the thinking the provider no
// longer read included, and set a compaction off long before it is due.
func (current *builder) uncount(saved int64) {
	if current.usage > 0 {
		current.usage = max(current.usage-saved, 0)
	}
}

// leaveOut removes the items at indices, which are in order, from the
// committed conversation; the provider's count still covers the items it
// covered.
func (current *builder) leaveOut(indices []int) {
	if len(indices) == 0 {
		return
	}
	kept := make([]llm.Item, 0, len(current.committedPrefix)-len(indices))
	mark := current.usageMark
	for index, item := range current.committedPrefix {
		if len(indices) != 0 && indices[0] == index {
			indices = indices[1:]
			if index < current.usageMark {
				mark--
			}
			continue
		}
		kept = append(kept, item)
	}
	current.committedPrefix, current.usageMark = kept, mark
}

// oldThinking lists, in order, the reasoning items of the committed
// conversation from before its latest turns with reasoning, turns of them;
// none for zero.
func (current *builder) oldThinking(turns int) []int {
	if turns <= 0 {
		return nil
	}
	items := current.committedPrefix
	starts := turnStarts(items)
	seen, thinking := 0, false
	for index := len(items) - 1; index > 0; index-- {
		switch {
		case items[index].Type == llm.ItemReasoning:
			thinking = true
		case starts[index] && thinking:
			seen, thinking = seen+1, false
			if seen < turns {
				continue
			}
			var old []int
			for before, item := range items[:index] {
				if item.Type == llm.ItemReasoning {
					old = append(old, before)
				}
			}
			return old
		}
	}
	return nil
}

// turnStarts marks the items where a turn begins, as sessionmetrics counts
// the API's turns: where something other than the results of the calls the
// model just made comes in — a message, or a result that arrives after the
// response that waited for it, which the Messages API shows as the user's
// text. The results of the calls just made carry the turn on.
func turnStarts(items []llm.Item) []bool {
	starts := make([]bool, len(items))
	calls := make(map[string]bool)
	responding := false
	respond := func() {
		if !responding {
			clear(calls)
		}
		responding = true
	}
	for index, item := range items {
		switch data := item.Data.(type) {
		case llm.ToolCall:
			respond()
			calls[data.CallID] = true
		case llm.Reasoning:
			respond()
		case llm.Message:
			if data.Role == llm.RoleAssistant {
				respond()
				continue
			}
			starts[index] = index > 0 && (strings.TrimSpace(data.Text) != "" || len(data.Images) != 0)
			responding = false
		case llm.ToolResult:
			starts[index] = !calls[data.CallID]
			responding = false
		}
	}
	return starts
}

// pruneItem cuts a result to its note: its images go, its texts are cut to
// bytes, and a shell command's output paths are named.
func pruneItem(item llm.Item, bytes int) llm.Item {
	result := item.Data.(llm.ToolResult)
	var text strings.Builder
	images := 0
	for _, part := range result.Output {
		switch part.Kind {
		case llm.ToolResultImage:
			images++
		default:
			text.WriteString(part.Value)
		}
	}
	content := text.String()
	paths := outputPaths(content)
	pruned := prunedResult(content, bytes, paths)
	if images > 0 {
		pruned = fmt.Sprintf("[%d image(s) were pruned from the context; view them again if needed.]\n", images) + pruned
	}
	item.Data = llm.ToolResult{CallID: result.CallID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: pruned}}}
	return item
}

// outputPaths finds where a shell result says its whole output is: the
// capture paths its truncation marker names, or none.
func outputPaths(text string) string {
	const marker = "complete output in "
	at := strings.Index(text, marker)
	if at < 0 {
		return ""
	}
	rest := text[at+len(marker):]
	// The marker ends with "..." before the tail of the output.
	if end := strings.Index(rest, "..."); end >= 0 {
		rest = rest[:end]
	}
	if end := strings.IndexAny(rest, "\n"); end >= 0 {
		rest = rest[:end]
	}
	return strings.TrimSpace(rest)
}
