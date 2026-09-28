package contextbuilder

import (
	_ "embed"
	"fmt"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// ToolCallRunningPayload is the result a running call shows until it completes.
const ToolCallRunningPayload = "Tool call is still running. Its result arrives in a later turn: continue with independent work, or end your turn to wait for it."

//go:embed prompts/preamble.md
var preambleFile string

var preamble = strings.TrimSpace(preambleFile)

type builder struct {
	request         llm.Request
	preamble        string
	systemPrompt    string
	committedPrefix []llm.Item
	stagedSuffix    []llm.Item

	// calls holds every tool call the model made, so a result that arrives
	// after compaction removed its call can still say what it answers.
	calls map[string]llm.ToolCall
	// stagedInputs are the external inputs in stagedSuffix, and unanswered
	// those committed since the model last responded: a compaction keeps
	// them after its summary. answered holds the text of all earlier ones.
	stagedInputs []userInput
	unanswered   []userInput
	answered     []string
	// compactable is set once the model responds and cleared by compaction.
	compactable bool
	// compacted counts the items the latest compaction replaced.
	compacted int
	// usage is what the provider reported for the latest request and its
	// response; usageMark is len(committedPrefix) right after that response.
	usage      int64
	usageMark  int
	toolTokens int64
	// transcript is the file that keeps the whole conversation.
	transcript string
}

var _ Builder = (*builder)(nil)

func NewBuilder(skills ...tool.Skill) Builder {
	current := &builder{
		preamble:        preambleWith(skills),
		committedPrefix: make([]llm.Item, 1),
		calls:           make(map[string]llm.ToolCall),
	}
	current.SetSystemPrompt("")
	return current
}

// preambleWith is the preamble with the skills it lists.
func preambleWith(skills []tool.Skill) string {
	if skillPrompt := formatSkillsForPrompt(skills); skillPrompt != "" {
		return preamble + "\n\n" + skillPrompt
	}
	return preamble
}

// SetSkills replaces the skills the system prompt lists, for the requests
// built from now on.
func (current *builder) SetSkills(skills []tool.Skill) {
	current.preamble = preambleWith(skills)
	current.SetSystemPrompt(current.systemPrompt)
}

func (current *builder) AddExternalInput(input inbox.Input) error {
	if input.Kind != inbox.InputExternal {
		return fmt.Errorf(
			"external input %q has input kind %q",
			input.ID,
			input.Kind,
		)
	}

	message, err := DecodeMessage(input.Payload)
	if err != nil {
		return fmt.Errorf("decode external input %q: %w", input.ID, err)
	}
	// The files the message links follow its text.
	item := llm.Item{
		Type: llm.ItemMessage,
		Data: llm.Message{Role: llm.RoleUser, Text: linkedText(message.Text, message.Files), Images: message.Images},
	}
	current.stagedSuffix = append(current.stagedSuffix, item)
	current.stagedInputs = append(current.stagedInputs, userInput{item: item, answered: answeredText(message)})
	return nil
}

// userInput is an external input of the conversation: the item the model
// reads, and what a compaction keeps of it once the model answered it.
type userInput struct {
	item     llm.Item
	answered string
}

// inputItems are the items of inputs.
func inputItems(inputs []userInput) []llm.Item {
	items := make([]llm.Item, len(inputs))
	for index, input := range inputs {
		items[index] = input.item
	}
	return items
}

func (current *builder) SetModel(model llm.Model) {
	current.request.Model = model
}

func (current *builder) AddControlMessage(request inbox.ControlMessage) {
	switch request.Mode {
	case inbox.UpdateSettings:
		settings := request.Parameters.(inbox.Settings)
		current.request.Model.ReasoningEffort = settings.ReasoningEffort
	case inbox.Heartbeat:
		current.stagedSuffix = append(current.stagedSuffix, llm.Item{
			Type: llm.ItemMessage,
			Data: llm.Message{Role: llm.RoleUser, Text: request.Reason},
		})
	}
}

func (current *builder) SetSystemPrompt(prompt string) {
	current.systemPrompt = prompt
	current.committedPrefix[0] = llm.Item{Type: llm.ItemMessage, Data: llm.Message{
		Role: llm.RoleSystem,
		Text: strings.TrimSpace(current.preamble + "\n\n" + current.systemPrompt),
	}}
}

func (current *builder) AddModelResponse(response llm.Response) {
	current.committedPrefix = append(current.committedPrefix, response.Output...)
	for _, item := range response.Output {
		if call, ok := item.Data.(llm.ToolCall); ok {
			current.calls[call.CallID] = call
		}
	}
	if len(response.Output) != 0 {
		current.compactable = true
	}
	for _, input := range current.unanswered {
		current.answered = append(current.answered, input.answered)
	}
	current.unanswered = nil
	current.usage, current.usageMark = 0, 0
	if reported := response.Usage.InputTokens + response.Usage.OutputTokens; reported > 0 {
		current.usage, current.usageMark = reported, len(current.committedPrefix)
	}
}

func (current *builder) AddReasoning(reasoning llm.Reasoning) {
	current.stagedSuffix = append(current.stagedSuffix, llm.Item{
		Type: llm.ItemReasoning,
		Data: reasoning,
	})
}

func (current *builder) AddTool(tool llm.Tool) {
	current.request.Tools = append(current.request.Tools, tool)
	current.toolTokens += estimateTool(tool)
}

// SetTools replaces the tools the model can call, for the requests built
// from now on: a plugin's tools come, change or go while a session runs.
func (current *builder) SetTools(tools []llm.Tool) {
	current.request.Tools = slices.Clone(tools)
	current.toolTokens = 0
	for _, tool := range tools {
		current.toolTokens += estimateTool(tool)
	}
}

func (current *builder) AddToolResult(
	callID string,
	payload []llm.ToolResultOutput,
	running bool,
) {
	runningOutput := []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: ToolCallRunningPayload}}
	if running {
		payload = runningOutput
	}
	current.stagedSuffix = slices.DeleteFunc(current.stagedSuffix, func(item llm.Item) bool {
		if item.Type != llm.ItemToolResult {
			return false
		}
		result := item.Data.(llm.ToolResult)
		return result.CallID == callID && slices.Equal(result.Output, runningOutput)
	})
	current.stagedSuffix = append(current.stagedSuffix, llm.Item{
		Type: llm.ItemToolResult,
		Data: llm.ToolResult{CallID: callID, Output: payload},
	})
}

func (current *builder) Commit() {
	current.committedPrefix = append(current.committedPrefix, current.stagedSuffix...)
	current.stagedSuffix = nil
	current.unanswered = append(current.unanswered, current.stagedInputs...)
	current.stagedInputs = nil
}

func (current *builder) Build() (Result, error) {
	request := current.request
	input := make([]llm.Item, 0, len(current.committedPrefix)+len(current.stagedSuffix))
	input = append(input, current.committedPrefix...)
	input = append(input, current.stagedSuffix...)
	request.Input = current.detachResults(input, false)
	request.Tools = append([]llm.Tool(nil), request.Tools...)
	result := Result{Request: request, EstimatedTokens: current.estimate(), Compactable: current.compactable}
	for _, input := range slices.Concat(current.unanswered, current.stagedInputs) {
		result.PendingTokens += estimateItem(input.item)
	}
	if current.compacted > 0 {
		result.Report.Changes = append(result.Report.Changes, Change{
			Kind:   ChangeCompacted,
			Source: "conversation",
			Reason: fmt.Sprintf("a summary replaced %d earlier items", current.compacted),
		})
	}
	return result, nil
}
