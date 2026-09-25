package contextbuilder

import (
	"cmp"
	_ "embed"
	"encoding/json/v2"
	"fmt"
	"regexp"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Compaction swaps a long conversation for a summary the model writes of it.
// A compaction turn sends the conversation as an ordinary turn would, with
// instructions to summarize it appended; the response replaces the
// conversation with one message that holds the summary, the user's earlier
// messages word for word as far as they fit, and the tool calls still
// running. Messages the model has not answered yet follow it unchanged, so
// the turn after a compaction answers them as if nothing had happened.

//go:embed prompts/compaction.md
var compactionFile string

var compactionPrompt = strings.TrimSpace(compactionFile)

// compactionReminder closes the compaction prompt, after the user's focus:
// the model reads the end of a long request most closely.
const compactionReminder = "Reminder: respond with text only, an <analysis> block followed by a <summary> block. Do not call any tools."

//go:embed prompts/compacted.md
var compactedFile string

var compactedPreface = strings.TrimSpace(compactedFile)

// compactedResume ends the message that stands in for a compacted
// conversation.
const compactedResume = "Continue from where the summary leaves off without asking the user any further questions. Resume directly: do not acknowledge the summary, do not recap it, and do not preface your reply with something like \"I'll continue\"; pick up the last task as if the break had not happened. Messages that follow this one take precedence."

const (
	// keptInputsBudget bounds the characters of the user's earlier messages
	// a compaction keeps word for word, newest first; keptInputLimit bounds
	// one message, which keeps its beginning and end.
	keptInputsBudget = 40_000
	keptInputLimit   = 12_000

	// trimmedResultBytes is what a tool result keeps of each text when a
	// compaction request has to be cut to fit its budget.
	trimmedResultBytes = 2000

	// Token estimates: the providers' tokenizers average close to four bytes
	// per token of English text and code, and bill an image by its size,
	// which a reference does not tell.
	bytesPerToken = 4
	itemTokens    = 4
	imageTokens   = 1600
)

func (current *builder) BuildCompaction(focus string, budget int64) (Result, error) {
	result, err := current.Build()
	if err != nil {
		return Result{}, err
	}
	prompt := compactionPrompt
	if focus = strings.TrimSpace(focus); focus != "" {
		prompt += "\n\nThe user asked the summary to focus on:\n" + focus
	}
	prompt += "\n\n" + compactionReminder
	promptTokens := itemTokens + estimateText(prompt)
	if budget > 0 && result.EstimatedTokens+promptTokens > budget {
		var changes []Change
		result.Request.Input, result.EstimatedTokens, changes = current.fit(result.Request.Input, result.EstimatedTokens, budget-promptTokens)
		result.Report.Changes = append(result.Report.Changes, changes...)
	}
	result.Request.Input = append(result.Request.Input, userMessage(prompt))
	result.EstimatedTokens += promptTokens
	return result, nil
}

// fit shrinks the input of a compaction request, estimated at estimated
// tokens, to about budget: the conversation may have outgrown the context
// window before it could be compacted. The largest tool results are cut short
// first, then the oldest items are left out, keeping the system prompt and
// the summary of an earlier compaction. The items' size estimates are scaled
// to estimated, which may rest on the provider's count.
func (current *builder) fit(input []llm.Item, estimated, budget int64) ([]llm.Item, int64, []Change) {
	sized := current.toolTokens
	for _, item := range input {
		sized += estimateItem(item)
	}
	scale := float64(estimated) / float64(max(sized, 1))
	tokens := func(item llm.Item) int64 { return int64(float64(estimateItem(item)) * scale) }

	var changes []Change
	results := make([]int, 0)
	for index, item := range input {
		if item.Type == llm.ItemToolResult {
			results = append(results, index)
		}
	}
	slices.SortStableFunc(results, func(a, b int) int { return cmp.Compare(estimateItem(input[b]), estimateItem(input[a])) })
	cut := 0
	for _, index := range results {
		if estimated <= budget {
			break
		}
		trimmed := trimResult(input[index])
		if saved := tokens(input[index]) - tokens(trimmed); saved > 0 {
			input[index] = trimmed
			estimated -= saved
			cut++
		}
	}
	if cut > 0 {
		changes = append(changes, Change{Kind: ChangeTruncated, Source: "tool results", Reason: fmt.Sprintf("%d tool results were cut short to fit the context window", cut)})
	}

	first := 1
	if current.compacted > 0 {
		first = 2 // the summary of the earlier compaction
	}
	dropped := 0
	for index := first; estimated > budget && index < len(input)-1; index++ {
		estimated -= tokens(input[index])
		dropped++
	}
	if dropped > 0 {
		note := userMessage(fmt.Sprintf("[%d earlier items of the conversation are left out here: they did not fit the context window.]", dropped))
		kept := slices.Concat(input[:first], []llm.Item{note}, input[first+dropped:])
		input = current.detachResults(kept, true)
		estimated += itemTokens + estimateText(messageText(note))
		changes = append(changes, Change{Kind: ChangeOmitted, Source: "conversation", Reason: fmt.Sprintf("%d earlier items were left out to fit the context window", dropped)})
	}
	return input, estimated, changes
}

// trimResult cuts the texts of a tool result short and leaves its images out.
func trimResult(item llm.Item) llm.Item {
	result := item.Data.(llm.ToolResult)
	output := make([]llm.ToolResultOutput, len(result.Output))
	for index, part := range result.Output {
		switch part.Kind {
		case llm.ToolResultImage:
			output[index] = llm.ToolResultOutput{Kind: llm.ToolResultText, Value: "[An image was left out to fit the context window.]"}
		default:
			output[index] = llm.ToolResultOutput{Kind: part.Kind, Value: clip(part.Value, trimmedResultBytes)}
		}
	}
	item.Data = llm.ToolResult{CallID: result.CallID, Output: output}
	return item
}

func messageText(item llm.Item) string {
	message, _ := item.Data.(llm.Message)
	return message.Text
}

func (current *builder) Compact(response llm.Response) bool {
	summary := Summary(response)
	if summary == "" {
		return false
	}
	message := current.compactedMessage(summary)
	current.compacted = len(current.committedPrefix) - 1 - len(current.unanswered)
	prefix := make([]llm.Item, 0, 2+len(current.unanswered))
	prefix = append(prefix, current.committedPrefix[0], message)
	current.committedPrefix = append(prefix, current.unanswered...)
	current.compactable = false
	current.usage, current.usageMark = 0, 0
	return true
}

var (
	analysisBlock = regexp.MustCompile(`(?s)<analysis>.*?</analysis>`)
	summaryBlock  = regexp.MustCompile(`(?s)<summary>(.*?)</summary>`)
)

// Summary is the summary a compaction response wrote: its <summary> block,
// without the <analysis> that came before, or the whole answer from a model
// that did not use the blocks. A summary cut off by the output limit is kept
// as far as it goes; an answer cut off before its summary began has none,
// and neither has an empty one.
func Summary(response llm.Response) string {
	var parts []string
	for _, item := range response.Output {
		message, ok := item.Data.(llm.Message)
		if !ok || message.Role != llm.RoleAssistant && message.Role != "" {
			continue
		}
		if text := strings.TrimSpace(message.Text); text != "" {
			parts = append(parts, text)
		}
	}
	text := analysisBlock.ReplaceAllString(strings.Join(parts, "\n\n"), "")
	if match := summaryBlock.FindStringSubmatch(text); match != nil {
		return strings.TrimSpace(match[1])
	}
	if _, rest, open := strings.Cut(text, "<summary>"); open {
		return strings.TrimSpace(rest)
	}
	if strings.Contains(text, "<analysis>") {
		return ""
	}
	return strings.TrimSpace(text)
}

// compactedMessage is the message that stands in for the conversation a
// compaction summarized.
func (current *builder) compactedMessage(summary string) llm.Item {
	var text strings.Builder
	text.WriteString(compactedPreface)
	if kept, omitted := keptInputs(current.answered); len(kept) != 0 {
		text.WriteString("\n\nThe user's messages before the compaction, oldest first")
		if omitted > 0 {
			fmt.Fprintf(&text, " (%d earlier ones are left out)", omitted)
		}
		text.WriteString(":\n<user_messages>\n")
		for _, message := range kept {
			text.WriteString("<message>\n" + message + "\n</message>\n")
		}
		text.WriteString("</user_messages>")
	}
	if running := current.runningCalls(); len(running) != 0 {
		text.WriteString("\n\nTool calls made before the compaction that are still running; their results arrive later:\n<running_tool_calls>\n")
		for _, call := range running {
			fmt.Fprintf(&text, "- %s\n", describeCall(call))
		}
		text.WriteString("</running_tool_calls>")
	}
	text.WriteString("\n\nThe summary:\n<summary>\n" + summary + "\n</summary>")
	if current.transcript != "" {
		fmt.Fprintf(&text, "\n\nIf you need details from before the compaction, such as exact code, error messages, command output or what you wrote, read them from the session's full transcript at %s (JSON Lines, one session record per line).", current.transcript)
	}
	text.WriteString("\n\n" + compactedResume)
	return userMessage(text.String())
}

// SetTranscript names the file that keeps the whole conversation, which a
// compaction points the model to.
func (current *builder) SetTranscript(path string) {
	current.transcript = path
}

// keptInputs returns the newest messages that fit the budget, oldest first,
// and how many older ones do not.
func keptInputs(messages []string) ([]string, int) {
	var kept []string
	budget := keptInputsBudget
	omitted := 0
	for index := len(messages) - 1; index >= 0; index-- {
		message := clip(strings.TrimSpace(messages[index]), keptInputLimit)
		if message == "" {
			continue
		}
		if omitted > 0 || len(message) > budget {
			omitted++
			continue
		}
		budget -= len(message)
		kept = append(kept, message)
	}
	slices.Reverse(kept)
	return kept, omitted
}

// clip shortens text to about limit bytes, keeping its beginning and end.
func clip(text string, limit int) string {
	if len(text) <= limit {
		return text
	}
	head, tail := limit*2/3, len(text)-limit/3
	for head > 0 && !utf8.RuneStart(text[head]) {
		head--
	}
	for tail < len(text) && !utf8.RuneStart(text[tail]) {
		tail++
	}
	return text[:head] + fmt.Sprintf("\n[… %d characters left out …]\n", utf8.RuneCountInString(text[head:tail])) + text[tail:]
}

// runningCalls lists the calls of the committed conversation whose latest
// result there is the placeholder of a running call: compaction removes both,
// and nothing else would say the call is still running. A placeholder that is
// not committed yet stays, and speaks for itself.
func (current *builder) runningCalls() []llm.ToolCall {
	var calls []llm.ToolCall
	running := make(map[string]bool)
	for index, item := range slices.Concat(current.committedPrefix, current.stagedSuffix) {
		switch data := item.Data.(type) {
		case llm.ToolCall:
			calls = append(calls, data)
		case llm.ToolResult:
			running[data.CallID] = index < len(current.committedPrefix) && isRunning(data.Output)
		}
	}
	return slices.DeleteFunc(calls, func(call llm.ToolCall) bool { return !running[call.CallID] })
}

// detachResults rewrites the results of calls the model made that are no
// longer in the input as messages from the user: a compaction summarized the
// calls away, or fit left them out, and providers reject a result without its
// call. Unless always is set, only a compacted conversation is checked.
func (current *builder) detachResults(input []llm.Item, always bool) []llm.Item {
	if current.compacted == 0 && !always {
		return input
	}
	present := make(map[string]bool)
	for _, item := range input {
		if call, ok := item.Data.(llm.ToolCall); ok && item.Type == llm.ItemToolCall {
			present[call.CallID] = true
		}
	}
	for index, item := range input {
		result, ok := item.Data.(llm.ToolResult)
		if !ok || item.Type != llm.ItemToolResult || present[result.CallID] {
			continue
		}
		if call, known := current.calls[result.CallID]; known {
			input[index] = detachedResult(call, result)
		}
	}
	return input
}

func detachedResult(call llm.ToolCall, result llm.ToolResult) llm.Item {
	subject := describeCall(call)
	if isRunning(result.Output) {
		return userMessage(fmt.Sprintf("The tool call %s, made before the conversation was compacted, is still running; its result arrives in a later turn.", subject))
	}
	var text strings.Builder
	fmt.Fprintf(&text, "Result of the tool call %s, made before the conversation was compacted:", subject)
	for _, part := range result.Output {
		if part.Kind == llm.ToolResultImage {
			text.WriteString("\n[An image was attached here. It cannot be shown without its call; view it again if it is still needed.]")
			continue
		}
		text.WriteString("\n" + part.Value)
	}
	return userMessage(text.String())
}

func describeCall(call llm.ToolCall) string {
	arguments := strings.Join(strings.Fields(call.Arguments), " ")
	if runes := []rune(arguments); len(runes) > 300 {
		arguments = string(runes[:299]) + "…"
	}
	return fmt.Sprintf("%s (%s %s)", call.CallID, call.Name, arguments)
}

func isRunning(output []llm.ToolResultOutput) bool {
	return len(output) == 1 && output[0].Kind == llm.ToolResultText && output[0].Value == ToolCallRunningPayload
}

func userMessage(text string) llm.Item {
	return llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: text}}
}

// estimate approximates the input tokens of the next request: the provider's
// count for the latest request and its response, plus what came after it.
// Without a count, everything is estimated from its size.
func (current *builder) estimate() int64 {
	tokens, from := current.usage, min(current.usageMark, len(current.committedPrefix))
	if tokens <= 0 {
		tokens, from = current.toolTokens, 0
	}
	for _, item := range current.committedPrefix[from:] {
		tokens += estimateItem(item)
	}
	for _, item := range current.stagedSuffix {
		tokens += estimateItem(item)
	}
	return tokens
}

func estimateItem(item llm.Item) int64 {
	tokens := int64(itemTokens)
	switch data := item.Data.(type) {
	case llm.Message:
		tokens += estimateText(data.Text) + int64(len(data.Images))*imageTokens
	case llm.ToolCall:
		tokens += estimateText(data.Name) + estimateText(data.Arguments)
	case llm.ToolResult:
		for _, part := range data.Output {
			if part.Kind == llm.ToolResultImage {
				tokens += imageTokens
			} else {
				tokens += estimateText(part.Value)
			}
		}
	case llm.Reasoning:
		if len(data.Raw) != 0 {
			tokens += int64(len(data.Raw)) / bytesPerToken
		} else {
			for _, summary := range data.Summary {
				tokens += estimateText(summary)
			}
		}
	}
	return tokens
}

func estimateTool(tool llm.Tool) int64 {
	parameters, _ := json.Marshal(tool.Parameters, json.Deterministic(true))
	return itemTokens + estimateText(tool.Name) + estimateText(tool.Description) + int64(len(parameters))/bytesPerToken
}

func estimateText(text string) int64 {
	return int64(len(text)+bytesPerToken-1) / bytesPerToken
}
