// Package contextbuilder defines I/O-pure, in-memory model request construction.
package contextbuilder

import (
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type ChangeKind string

const (
	ChangeOmitted   ChangeKind = "omitted"
	ChangeTruncated ChangeKind = "truncated"
	ChangeCompacted ChangeKind = "compacted"
)

type Change struct {
	Kind   ChangeKind
	Source string
	Reason string
}

type Report struct {
	Changes []Change
}

type Result struct {
	Request llm.Request
	Report  Report
	// EstimatedTokens approximates the size of the request in input tokens:
	// what the provider reported for the latest request and its response,
	// plus an estimate of everything added since.
	EstimatedTokens int64
	// PendingTokens is the part of EstimatedTokens taken by input the model
	// has not answered yet, which a compaction keeps as it is.
	PendingTokens int64
	// Compactable reports that a compaction would summarize something: the
	// model has responded since the conversation was last compacted.
	Compactable bool
}

// Builder retains model request state without performing I/O.
type Builder interface {
	AddExternalInput(inbox.Input) error
	AddControlMessage(inbox.ControlMessage)
	SetModel(llm.Model)
	SetSystemPrompt(string)
	AddModelResponse(llm.Response)
	AddReasoning(llm.Reasoning)
	AddTool(llm.Tool)
	// SetTools replaces the tools the model can call, and SetSkills the
	// skills the system prompt lists, for the requests built from then on.
	SetTools([]llm.Tool)
	SetSkills([]tool.Skill)
	AddToolResult(string, []llm.ToolResultOutput, bool)
	Commit()
	Build() (Result, error)
	// BuildCompaction returns the request of a compaction turn: the request
	// Build returns, followed by instructions to summarize the conversation.
	// The optional focus tells the summary what matters most. A positive
	// budget bounds the request's estimated tokens: past it, the largest tool
	// results are cut short and then the oldest items left out, as the Report
	// says.
	BuildCompaction(focus string, budget int64) (Result, error)
	// SetTranscript names the file that keeps the whole conversation; the
	// message that stands in for a compacted conversation points to it.
	SetTranscript(path string)
	// Compact replaces the conversation a compaction turn summarized with the
	// summary its response holds. Messages the model has not answered yet and
	// everything that arrived during the compaction stay as they are. It
	// reports false, leaving the context unchanged, for a response without a
	// summary.
	Compact(llm.Response) bool
}
