package llm

import "encoding/json/jsontext"

type Role string

const (
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
	RoleSystem    Role = "system"
)

type ItemType string

const (
	ItemMessage    ItemType = "message"
	ItemToolCall   ItemType = "tool_call"
	ItemToolResult ItemType = "tool_result"
	ItemReasoning  ItemType = "reasoning"
)

type Item struct {
	ProviderID string
	Type       ItemType
	Data       any

	// Model is the model whose response the item came in, as the session
	// records it on the response's turn; it is empty for input, and for the
	// output of turns recorded before turns kept their model. ProviderID and
	// a reasoning item's Raw mean something to that model alone: an adapter
	// replaying the item to another model leaves them out.
	Model string `json:",omitzero"`
}

type Message struct {
	Role  Role
	Text  string
	Phase string
	// Images come with a user's message: pictures the model sees beside the
	// text, which refers to each by its label.
	Images []Image `json:",omitzero"`
}

// Image is a picture that goes with a message. URL references it the way a
// tool result's images do: a base64 data URL or an http(s) URL. Label is how
// the message's text refers to it, such as "[Image 1]"; the model reads it
// right before the picture.
type Image struct {
	Label string `json:",omitzero"`
	URL   string
}

type ToolCall struct {
	CallID    string
	Name      string
	Arguments string
}

type ToolResultKind string

const (
	ToolResultText  ToolResultKind = "text"
	ToolResultImage ToolResultKind = "image"
)

type ToolResultOutput struct {
	Kind  ToolResultKind
	Value string
}

type ToolResult struct {
	CallID string
	Output []ToolResultOutput
}

// Raw is the provider's verbatim reasoning item. A provider may attach state to
// it that the harness cannot reconstruct, such as encrypted reasoning content, so
// adapters replay Raw unchanged instead of re-encoding Summary.
type Reasoning struct {
	Summary []string       `json:",omitzero"`
	Raw     jsontext.Value `json:",omitzero"`
}

type ToolType string

const (
	ToolFunction ToolType = "function"
	ToolHosted   ToolType = "hosted"
)

type Tool struct {
	Type        ToolType
	Name        string
	Description string
	Parameters  map[string]any
}

type Model struct {
	ID              string
	MaxOutputTokens *int64
	ReasoningEffort ReasoningEffort
}

type ReasoningEffort string

const (
	ReasoningEffortLow    ReasoningEffort = "low"
	ReasoningEffortMedium ReasoningEffort = "medium"
	ReasoningEffortHigh   ReasoningEffort = "high"
	ReasoningEffortXHigh  ReasoningEffort = "xhigh"
	ReasoningEffortMax    ReasoningEffort = "max"
)

func (effort ReasoningEffort) Valid() bool {
	switch effort {
	case ReasoningEffortLow, ReasoningEffortMedium, ReasoningEffortHigh, ReasoningEffortXHigh, ReasoningEffortMax:
		return true
	default:
		return false
	}
}

type Request struct {
	Model Model
	Input []Item
	Tools []Tool
}

type StopReason string

const (
	StopComplete        StopReason = "complete"
	StopMaxOutputTokens StopReason = "max_output_tokens"
	StopRefused         StopReason = "refused"
)

type Response struct {
	ID      string
	Stop    StopReason
	Output  []Item `json:",omitzero"`
	Usage   Usage
	Failure *Failure
}

// InputTokens includes CachedInputTokens and CacheWriteInputTokens.
// OutputTokens includes ReasoningTokens.
type Usage struct {
	InputTokens           int64
	CachedInputTokens     int64
	CacheWriteInputTokens int64
	OutputTokens          int64
	ReasoningTokens       int64
	Raw                   jsontext.Value `json:",omitzero"`
}

type Failure struct {
	Code    string
	Message string
}
