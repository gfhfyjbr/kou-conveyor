// Package tool defines model-visible tool selection and pure translation
// between model tool calls and durable operations.
package tool

import (
	"encoding/json/jsontext"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

type CallStatus struct {
	Error          string
	ErrorTruncated bool           `json:",omitzero"`
	WaitingFor     []operation.ID `json:",omitzero"`
}

// Context is turn-local and coordinator-owned. Submit allocates an ID and
// records inert data without performing I/O or handing work to another queue.
type Context interface {
	Submit(operation.Spec) operation.ID
}

type ResultTranslator interface {
	TranslateResult(string, CallStatus, []operation.Operation) (llm.ToolResult, error)
}

type Translator interface {
	ResultTranslator
	Translate(Context, llm.ToolCall) CallStatus
}

type Definition struct {
	Tool     llm.Tool
	Metadata jsontext.Value
}

type RegistrationID = uuid.UUID

type Registry interface {
	// StaticDefinitions returns the definitions of the enabled tools: the
	// built-in ones, then those registered.
	StaticDefinitions() []Definition
	Resolve(string) (Translator, bool)
	// RegisterTool adds a tool beside the built-in ones, such as a plugin's.
	// Its name may not be taken by an enabled tool.
	RegisterTool(Definition, Translator) error
	RegisterSkill(Skill) (RegistrationID, error)
	UnregisterSkill(RegistrationID)
	Skills() []Skill
}

type Skill struct {
	Name        string
	Description string
	Path        string
}
