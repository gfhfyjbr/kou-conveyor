// Package coordinator defines the owner of the central event loop.
package coordinator

import (
	"context"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type Dependencies struct {
	ToolHeartbeatInterval time.Duration
	SessionID             session.ID
	Inbox                 *inbox.Inbox
	Restored              sessionstore.ResumeState
	Sessions              sessionstore.Store
	ContextBuilder        contextbuilder.Builder
	LLM                   llm.Adapter
	Tools                 tool.Registry
	Operations            operation.Manager

	// AutoCompactTokens is the estimated request size, in input tokens, from
	// which the coordinator compacts the conversation before an ordinary turn
	// and then continues with that turn. Zero disables automatic compaction;
	// a Compact control compacts on request either way.
	AutoCompactTokens int64
	// ContextWindow is how many input tokens a request can hold. A compaction
	// request that would not leave room for the summary is cut to fit; zero
	// sends it whole.
	ContextWindow int64
	// BeforeTurn, if set, runs on the coordinator's loop before the request
	// of every turn is built. It may change what the ContextBuilder sends
	// from then on — its tools, system prompt and skills — and what Tools
	// resolves, as when a plugin changed while the session runs.
	BeforeTurn func()
	// ToolWaitLimit bounds how long input delivered after tools (see
	// inbox.DeliverAfterTools) waits for the tool calls of the latest
	// response: past it, the input goes out with the results that are in,
	// and the calls still running show as such. Zero waits however long the
	// calls take; a heartbeat still wakes the model.
	ToolWaitLimit time.Duration
	// ToolGrace is how long the results of a response's tool calls are
	// gathered before they wake the model, from when the calls start:
	// results that land together arrive in one turn. Zero is a second.
	// ToolGraceLimit, when set, lets every result that arrives while other
	// calls of the response still run extend the wait by ToolGrace again,
	// up to ToolGraceLimit after the calls started: the model does not
	// answer results that were about to be joined by others.
	ToolGrace      time.Duration
	ToolGraceLimit time.Duration
	// Verifier, when set, checks the work once the model finishes a prompt:
	// its checks run before the run goes idle, and their failures go back
	// to the model, MaxVerifications times a prompt at most (zero is 3).
	Verifier         Verifier
	MaxVerifications int
	// NoRecovery leaves the model's answers as they come: without it, the
	// coordinator asks the model to go on after an answer the output limit
	// cut, asks again after a refusal or an empty answer, sends a request
	// again after a response that failed, a few times each, and waits for a
	// usage limit to lift (llm.RateLimitError) before it sends the request
	// again, twice in a row at most.
	NoRecovery bool
	// MaxTurns, MaxDuration and MaxCompactions bound the run: its ordinary
	// turns, the time since Run began and its compactions; zero sets no
	// bound. Once one is spent, the harness asks the model for its final
	// report, and the run stops after the response that answers it, the
	// tool calls still running canceled.
	MaxTurns       int
	MaxDuration    time.Duration
	MaxCompactions int
	// ReportEvery, when set, has the harness ask the model for a short
	// progress report this often while it works.
	ReportEvery time.Duration
	// CompactionReminder, when set, is what the harness tells the model
	// once the conversation nears AutoCompactTokens, once between
	// compactions: to bring its notes up to date before the summary
	// replaces the conversation, say.
	CompactionReminder string
	// Compacted, when set, is called after a compaction replaced the
	// conversation while the run goes on. What it returns, if anything, the
	// harness tells the model with the turn that follows.
	Compacted func(Compaction) string
}

// Compaction is a compaction that replaced the conversation while the run
// went on.
type Compaction struct {
	// Summary is what replaced the conversation.
	Summary string
	// Number counts the compactions of the run, from 1.
	Number int
	// FileChanges counts the calls of the tools that change files — Edit,
	// Write and apply_patch, under any name a gateway gave them — that
	// succeeded since the compaction before, or since the run began.
	// Commands change files too, which only the workspace can tell.
	FileChanges int
}

// Verifier checks the work the model did for a prompt before the run goes
// idle, as an operation the coordinator runs: the project's build, its
// tests. The runner gives one when the workspace's plugins or the
// environment name the checks.
type Verifier interface {
	// Spec is the operation that runs the checks; false when there are
	// none.
	Spec() (operation.Spec, bool)
	// Report reads a finished check: what the model reads of it, and
	// whether it passed.
	Report(operation.Operation) (report string, passed bool)
}

type Coordinator interface {
	// Run owns one session's decision loop until a stop control completes or
	// the context is canceled. It returns nil for a completed stop. It is
	// single-use; its caller must cancel the Inbox when Run returns.
	Run(context.Context) error
}

func New(dependencies Dependencies) Coordinator {
	return &coordinator{
		dependencies: dependencies,
		state:        newLoopState(),
	}
}
