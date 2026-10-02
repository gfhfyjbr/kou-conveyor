package contextbuilder

import (
	"fmt"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Pruning. The results of the tools are most of a long conversation, and
// most of them are read once: a file listed, a test run, a search. Once the
// conversation outgrows a budget, the oldest results beyond it are cut to a
// note that says what was there and, for a shell command, where the whole
// output still is; the model can read it again if it needs it, and the
// transcript search finds it. Pruning changes the committed conversation
// in place, in batches, so the prefix the provider caches changes once a
// batch rather than a little every turn. The latest turns are never pruned:
// what the model just asked for stays in view.

// PruneOptions say when and how the conversation is pruned.
type PruneOptions struct {
	// Budget is the estimated tokens the tool results of the conversation
	// may take together before the oldest are pruned; zero prunes nothing.
	Budget int64
	// Keep is how many of the latest tool results are never pruned.
	Keep int
	// Bytes is what a result is cut to when it is pruned: its head, before
	// the note. Zero keeps nothing of it.
	Bytes int
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
// once, together, they take more than the budget: from the oldest on,
// until what is left fits in half the budget, leaving the latest results
// alone. It reports how many results it pruned.
func (current *builder) prune() int {
	options := current.pruning
	if options.Budget <= 0 {
		return 0
	}
	var results []int
	var total int64
	for index, item := range current.committedPrefix {
		if item.Type != llm.ItemToolResult {
			continue
		}
		result := item.Data.(llm.ToolResult)
		if isPruned(result.Output) || isRunning(result.Output) {
			continue
		}
		results = append(results, index)
		total += estimateItem(item)
	}
	if total <= options.Budget {
		return 0
	}
	target := options.Budget / 2
	pruned := 0
	for _, index := range results {
		if total <= target || len(results)-pruned <= options.Keep {
			break
		}
		item := current.committedPrefix[index]
		before := estimateItem(item)
		current.committedPrefix[index] = pruneItem(item, options.Bytes)
		total -= before - estimateItem(current.committedPrefix[index])
		pruned++
	}
	if pruned > 0 {
		// The provider's count of the request no longer holds.
		current.usage, current.usageMark = 0, 0
		current.pruned += pruned
	}
	return pruned
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
