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
	// Progress refreshes are not session items. Keep their pre-refresh
	// staged projection for checkpoints, including its original order.
	recordedStagedSuffix []llm.Item
	stagedRefreshed      bool
	recordedPrefixDelta  int
	checkpointCompacted  int

	// calls holds every tool call the model made, so a result that arrives
	// after compaction removed its call can still say what it answers.
	calls map[string]llm.ToolCall
	// stagedInputs are the external inputs in stagedSuffix, and unanswered
	// those committed since the model last responded: a compaction keeps
	// them after its summary. answered holds the text of all earlier ones.
	stagedInputs []userInput
	unanswered   []userInput
	answered     []string
	// A resumed checkpoint keeps only the budgeted earlier prompts. Those
	// strings have already been clipped; clipping them twice changes text.
	answeredPrepared int
	answeredOmitted  int
	// compactable is set once the model responds and cleared by compaction.
	compactable bool
	// compacted counts the items the latest compaction replaced.
	compacted int
	// usage is what the provider reported for the latest request and its
	// response; usageMark is len(committedPrefix) right after that response.
	// resumed is set when the conversation was restored from a session's
	// record, until the next response: the latest count is of the
	// conversation as the run that sent it pruned it (Resumed).
	usage      int64
	usageMark  int
	resumed    bool
	toolTokens int64
	// transcript is the file that keeps the whole conversation; memory
	// is what a compaction keeps besides the summary, and touched the
	// files the compacted conversation read or changed.
	transcript  string
	memory      Memory
	touched     []touchedFile
	failedCalls map[string]bool // Canonical committed errors, before masking/pruning.
	// shownNotes is what the conversation last showed of the notes, in
	// the message of its latest compaction or a NotesUpdate since; nil
	// before any.
	shownNotes *string
	// pruning says when the oldest tool results are cut down; pruned
	// counts those cut so far, thinkingLeftOut the thinking of older turns
	// that went with them, and imagesLeftOut the images left out
	// (images.go). masking says how many of the latest results stay whole;
	// masked counts those replaced with notes so far.
	pruning         PruneOptions
	pruned          int
	thinkingLeftOut int
	imagesLeftOut   int
	masking         MaskOptions
	masked          int
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
	if current.stagedRefreshed {
		current.recordedStagedSuffix = append(current.recordedStagedSuffix, item)
	}
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
		item := llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: request.Reason}}
		current.stagedSuffix = append(current.stagedSuffix, item)
		if current.stagedRefreshed {
			current.recordedStagedSuffix = append(current.recordedStagedSuffix, item)
		}
		if notes, update := notesIn(request.Reason); update {
			current.shownNotes = &notes
		}
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
	current.usage, current.usageMark, current.resumed = 0, 0, false
	if reported := response.Usage.InputTokens + response.Usage.OutputTokens; reported > 0 {
		current.usage, current.usageMark = reported, len(current.committedPrefix)
	}
}

// Resumed marks the conversation as restored, until the next response
// (estimate).
func (current *builder) Resumed() {
	current.resumed = true
}

func (current *builder) AddReasoning(reasoning llm.Reasoning) {
	item := llm.Item{Type: llm.ItemReasoning, Data: reasoning}
	current.stagedSuffix = append(current.stagedSuffix, item)
	if current.stagedRefreshed {
		current.recordedStagedSuffix = append(current.recordedStagedSuffix, item)
	}
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
	if running {
		payload = runningPayload(payload)
	}
	current.stagedSuffix = withToolResult(current.stagedSuffix, callID, payload)
	if current.stagedRefreshed {
		current.recordedStagedSuffix = withToolResult(current.recordedStagedSuffix, callID, payload)
	}
}

// RefreshToolResult changes a running placeholder without changing what the
// session's recorded items would replay. Only the coordinator's progress path
// uses this method; recorded statuses still use AddToolResult.
func (current *builder) RefreshToolResult(callID string, payload []llm.ToolResultOutput) {
	if !current.stagedRefreshed {
		current.recordedStagedSuffix = slices.Clone(current.stagedSuffix)
		current.stagedRefreshed = true
	}
	current.stagedSuffix = withToolResult(current.stagedSuffix, callID, runningPayload(payload))
}

func withToolResult(items []llm.Item, callID string, payload []llm.ToolResultOutput) []llm.Item {
	items = slices.DeleteFunc(items, func(item llm.Item) bool {
		if item.Type != llm.ItemToolResult {
			return false
		}
		result := item.Data.(llm.ToolResult)
		return result.CallID == callID && isRunning(result.Output)
	})
	return append(items, llm.Item{
		Type: llm.ItemToolResult,
		Data: llm.ToolResult{CallID: callID, Output: payload},
	})
}

// runningPayload is what a running call shows: that it runs, and what the
// tool says of it meanwhile, such as where its output so far is.
func runningPayload(payload []llm.ToolResultOutput) []llm.ToolResultOutput {
	text := ToolCallRunningPayload
	for _, part := range payload {
		if part.Kind == llm.ToolResultText && strings.TrimSpace(part.Value) != "" {
			text += "\n" + strings.TrimSpace(part.Value)
			break
		}
	}
	return []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: text}}
}

func (current *builder) Commit() {
	current.failedCalls = failedToolCalls(current.stagedSuffix, current.failedCalls)
	if current.stagedRefreshed {
		current.recordedPrefixDelta += len(current.recordedStagedSuffix) - len(current.stagedSuffix)
	}
	current.recordedStagedSuffix, current.stagedRefreshed = nil, false
	current.committedPrefix = append(current.committedPrefix, current.stagedSuffix...)
	current.stagedSuffix = nil
	current.unanswered = append(current.unanswered, current.stagedInputs...)
	current.stagedInputs = nil
}

func (current *builder) Build() (Result, error) {
	current.boundImages()
	current.mask()
	current.prune()
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
	if current.masked > 0 {
		result.Report.Changes = append(result.Report.Changes, Change{
			Kind:   ChangeOmitted,
			Source: "tool results",
			Reason: fmt.Sprintf("%d tool results beyond the latest %d were masked to save context", current.masked, current.masking.Results),
		})
	}
	if current.pruned > 0 {
		result.Report.Changes = append(result.Report.Changes, Change{
			Kind:   ChangeTruncated,
			Source: "tool results",
			Reason: fmt.Sprintf("%d older tool results were pruned to save context", current.pruned),
		})
	}
	if current.thinkingLeftOut > 0 {
		result.Report.Changes = append(result.Report.Changes, Change{
			Kind:   ChangeOmitted,
			Source: "thinking",
			Reason: fmt.Sprintf("%d thinking blocks of older turns were left out to save context", current.thinkingLeftOut),
		})
	}
	if current.imagesLeftOut > 0 {
		result.Report.Changes = append(result.Report.Changes, Change{
			Kind:   ChangeOmitted,
			Source: "images",
			Reason: fmt.Sprintf("%d older images were left out to keep the requests within the provider's limits", current.imagesLeftOut),
		})
	}
	return result, nil
}
