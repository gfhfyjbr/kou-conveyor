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
