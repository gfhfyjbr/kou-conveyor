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
	// SetMemory says where what a compaction keeps besides the summary is:
	// the notes the model keeps, the earlier summaries (Memory).
	SetMemory(Memory)
	// SetPruning says when the oldest tool results of the conversation
	// are cut down to notes, to keep the context small between
	// compactions (PruneOptions).
	SetPruning(PruneOptions)
	// Resumed says that the conversation was restored from a session's
	// record. What the provider counted for the latest recorded response
	// was the conversation as the run that sent it pruned it; restored,
	// its results are whole again and, for a builder that leaves the
	// thinking of older turns out, all of its thinking is back. Until the
	// next response such a builder estimates the conversation at no less
	// than its size.
	Resumed()
	// Compact replaces the conversation a compaction turn summarized with the
	// summary its response holds. Messages the model has not answered yet and
	// everything that arrived during the compaction stay as they are. It
	// returns the message that stands in for the conversation, which the
	// record of the compaction keeps: given back as kept, by a run that
	// restores the session, it stands in as it was (Compacted). It reports
	// false, leaving the context unchanged, for a response without a
	// summary.
	Compact(response llm.Response, kept *Compacted) (Compacted, bool)
	// NotesUpdate is a message that shows the notes as they are now, when
	// they changed since the conversation last showed them; false when they
	// did not, or no compaction showed them yet. Sent as a heartbeat, the
	// message counts as shown.
	NotesUpdate() (string, bool)
}
