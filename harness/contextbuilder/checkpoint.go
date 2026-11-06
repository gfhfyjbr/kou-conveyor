package contextbuilder

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"slices"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Checkpointer saves the conversation after compaction without old tool
// outputs, old calls, or an unbounded copy of earlier user messages.
// Configuration (tools, model, system prompt, memory paths) stays with Builder.
type Checkpointer interface {
	Checkpoint(liveCalls []string) (jsontext.Value, error)
	RestoreCheckpoint(jsontext.Value) error
}

type builderCheckpoint struct {
	Version         int
	Committed       []llm.Item
	Staged          []llm.Item
	Calls           map[string]llm.ToolCall
	StagedInputs    []checkpointInput
	Unanswered      []checkpointInput
	Answered        []string
	AnsweredOmitted int
	Compactable     bool
	Compacted       int
	Touched         []checkpointFile
	ShownNotes      *string
	ReasoningEffort llm.ReasoningEffort
}

type checkpointInput struct {
	Item     llm.Item
	Answered string
}

type checkpointFile struct {
	Path    string
	Read    bool
	Changed bool
}

func (current *builder) Checkpoint(liveCalls []string) (jsontext.Value, error) {
	// Checkpoints are made immediately after Compact: its prefix is already
	// the summary plus unanswered inputs and its usage/pruning counts are zero.
	if current.compactable || current.usage != 0 || current.usageMark != 0 ||
		current.pruned != 0 || current.masked != 0 || current.thinkingLeftOut != 0 || current.imagesLeftOut != 0 {
		return nil, fmt.Errorf("context checkpoint must follow compaction")
	}
	answered, omitted := keptPreparedInputs(current.answered, current.answeredPrepared)
	staged := current.stagedSuffix
	if current.stagedRefreshed {
		staged = current.recordedStagedSuffix
	}
	value := builderCheckpoint{
		Version: 1, Committed: current.committedPrefix[1:], Staged: staged,
		Calls: make(map[string]llm.ToolCall), Answered: answered,
		AnsweredOmitted: current.answeredOmitted + omitted,
		Compactable:     current.compactable, Compacted: current.checkpointCompacted,
		ShownNotes: current.shownNotes, ReasoningEffort: current.request.Model.ReasoningEffort,
	}
	keepCall := func(id string) {
		if call, exists := current.calls[id]; exists {
			value.Calls[id] = call
		}
	}
	for _, id := range liveCalls {
		keepCall(id)
	}
	for _, items := range [][]llm.Item{current.committedPrefix, current.stagedSuffix} {
		for _, item := range items {
			switch data := item.Data.(type) {
			case llm.ToolCall:
				keepCall(data.CallID)
			case llm.ToolResult:
				keepCall(data.CallID)
			}
		}
	}
	for _, input := range current.stagedInputs {
		value.StagedInputs = append(value.StagedInputs, checkpointInput{Item: input.item, Answered: input.answered})
	}
	for _, input := range current.unanswered {
		value.Unanswered = append(value.Unanswered, checkpointInput{Item: input.item, Answered: input.answered})
	}
	for _, file := range current.touched {
		value.Touched = append(value.Touched, checkpointFile{Path: file.path, Read: file.read, Changed: file.changed})
	}
	return json.Marshal(value)
}

func (current *builder) RestoreCheckpoint(encoded jsontext.Value) error {
	var value builderCheckpoint
	if err := json.Unmarshal(encoded, &value); err != nil {
		return fmt.Errorf("decode context checkpoint: %w", err)
	}
	if value.Version != 1 || value.Compacted < 0 || value.Compactable || len(value.Committed) == 0 ||
		len(value.Touched) > touchedLimit || value.AnsweredOmitted < 0 {
		return fmt.Errorf("unsupported or invalid context checkpoint")
	}
	summary, isMessage := value.Committed[0].Data.(llm.Message)
	if value.Committed[0].Type != llm.ItemMessage || !isMessage || summary.Role != llm.RoleUser || summary.Text == "" {
		return fmt.Errorf("context checkpoint has no compacted user summary")
	}
	kept, omitted := keptPreparedInputs(value.Answered, len(value.Answered))
	if omitted != 0 || len(kept) != len(value.Answered) {
		return fmt.Errorf("context checkpoint exceeds the earlier-prompt budget")
	}
	current.committedPrefix = slices.Concat(current.committedPrefix[:1], value.Committed)
	current.stagedSuffix, current.calls = value.Staged, value.Calls
	if current.calls == nil {
		current.calls = make(map[string]llm.ToolCall)
	}
	current.stagedInputs, current.unanswered = nil, nil
	for _, input := range value.StagedInputs {
		current.stagedInputs = append(current.stagedInputs, userInput{item: input.Item, answered: input.Answered})
	}
	for _, input := range value.Unanswered {
		current.unanswered = append(current.unanswered, userInput{item: input.Item, answered: input.Answered})
	}
	current.answered, current.answeredPrepared, current.answeredOmitted = value.Answered, len(value.Answered), value.AnsweredOmitted
	current.compactable, current.compacted = value.Compactable, value.Compacted
	current.checkpointCompacted = value.Compacted
	current.recordedPrefixDelta = 0
	current.failedCalls = nil
	current.recordedStagedSuffix, current.stagedRefreshed = nil, false
	current.touched = nil
	for _, file := range value.Touched {
		current.touched = append(current.touched, touchedFile{path: file.Path, read: file.Read, changed: file.Changed})
	}
	current.shownNotes = value.ShownNotes
	current.request.Model.ReasoningEffort = value.ReasoningEffort
	current.pruned, current.masked, current.thinkingLeftOut, current.imagesLeftOut = 0, 0, 0, 0
	current.usage, current.usageMark = 0, 0
	return nil
}
