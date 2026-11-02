package contextbuilder

import (
	"fmt"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Masking. A rolling window keeps the latest tool results whole and
// replaces every older one with a note, whatever the conversation's size:
// the model reads the results it just asked for closely and the older ones
// rarely. The paper "The Complexity Trap: Simple Observation Masking Is as
// Efficient as LLM Summarization for Agent Context Management" (JetBrains
// Research, arXiv:2508.21433) finds a masked agent matches the solve rate
// of one whose context an LLM summarizes, at about half the cost — and
// that summarization serves best deferred to the longest trajectories,
// which masking does on its own: a masked conversation grows slowly enough
// that a token-budgeted compaction comes late or never. The paper's window
// counts turns of one observation each; here a tool result is the
// observation, so the window counts results (MaskOptions.Results), and a
// step's calls share it.
//
// Like pruning, masking changes the committed conversation in place — the
// transcript keeps what was masked, and the note says so. Unlike pruning,
// it asks nothing of a budget: a step's results cross the window's edge
// one at a time, at the end of the masked prefix, so the part of the
// conversation the provider caches — everything older than the window —
// stays as it was.

// MaskOptions say when tool results are masked.
type MaskOptions struct {
	// Results is how many of the latest tool results stay whole; the
	// older ones are masked. Zero masks nothing.
	Results int
}

// maskedNote marks a masked result.
const maskedNote = "[This result was masked to save space"

// SetMasking sets how tool results are masked; Build masks before it
// builds a request.
func (current *builder) SetMasking(options MaskOptions) {
	current.masking = options
}

// mask replaces the results beyond the latest ones the window keeps with
// notes, in place, and reports how many it masked. A call still running
// keeps its placeholder, and a result already cut as small — pruned to a
// short note — is left as it is.
func (current *builder) mask() int {
	keep := current.masking.Results
	if keep <= 0 {
		return 0
	}
	boundary := len(current.committedPrefix)
	for index, seen := len(current.committedPrefix)-1, 0; index >= 0 && seen < keep; index-- {
		if current.committedPrefix[index].Type == llm.ItemToolResult {
			seen++
			boundary = index
		}
	}
	masked := 0
	var counted int64
	for index := 0; index < boundary; index++ {
		item := current.committedPrefix[index]
		if item.Type != llm.ItemToolResult {
			continue
		}
		result := item.Data.(llm.ToolResult)
		if isMasked(result.Output) || isRunning(result.Output) {
			continue
		}
		cut := maskItem(item)
		saving := estimateItem(item) - estimateItem(cut)
		if saving <= 0 {
			continue
		}
		current.committedPrefix[index] = cut
		if index < current.usageMark {
			counted += saving
		}
		masked++
	}
	if masked == 0 {
		return 0
	}
	current.uncount(counted)
	current.masked += masked
	return masked
}

// isMasked reports a result that was masked.
func isMasked(output []llm.ToolResultOutput) bool {
	return len(output) == 1 && output[0].Kind == llm.ToolResultText && strings.Contains(output[0].Value, maskedNote)
}

// maskItem replaces a result with its note: its images go, and a shell
// command's output paths are named.
func maskItem(item llm.Item) llm.Item {
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
	note := maskedNote
	if paths := outputPaths(text.String()); paths != "" {
		note += "; the whole output is in " + paths
	}
	note += "; TranscriptSearch finds it.]"
	if images > 0 {
		note = fmt.Sprintf("[%d image(s) were masked from the context; view them again if needed.]\n", images) + note
	}
	item.Data = llm.ToolResult{CallID: result.CallID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: note}}}
	return item
}
