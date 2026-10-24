package main

import (
	"context"
	"runtime/debug"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/agentrunner"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A runner reads its whole session before its first request to the model,
// and drops most of what it read once the request is built. On a long
// session that is a burst of hundreds of megabytes: the heap grows to hold
// it, and the runtime keeps the pages for minutes after — a runner on a
// 300 MB session held about 250 MB with 20 MB of it live. So while it reads,
// the collector runs four times as often (the peak is about a third lower,
// the start a few percent slower), and at the first request to the model it
// goes back to its default, and what the reading left free goes back to the
// system. A GOGC or GOMEMLIMIT of the user's own is left as it is.

// readingGCPercent is the collector's target while the runner reads its
// session.
const readingGCPercent = 25

// tuneMemory sets the collector for reading the session, and returns the
// providers, whose clients put it back at their first request.
func tuneMemory(getenv func(string) string, providers []agentrunner.Provider) []agentrunner.Provider {
	if getenv("GOGC") != "" || getenv("GOMEMLIMIT") != "" {
		return providers
	}
	previous := debug.SetGCPercent(readingGCPercent)
	var once sync.Once
	settle := func() {
		once.Do(func() {
			debug.SetGCPercent(previous)
			go debug.FreeOSMemory()
		})
	}
	for i := range providers {
		newClient := providers[i].NewClient
		providers[i].NewClient = func(apiKey, baseURL string, maxAttempts int, getenv func(string) string) (agentrunner.Client, error) {
			client, err := newClient(apiKey, baseURL, maxAttempts, getenv)
			if err != nil {
				return nil, err
			}
			return settling{client, settle}, nil
		}
	}
	return providers
}

// settling is a client that settles the runner's memory at its first
// request.
type settling struct {
	agentrunner.Client
	settle func()
}

func (c settling) Respond(ctx context.Context, request llm.Request, options llm.RequestOptions) (llm.Response, error) {
	c.settle()
	return c.Client.Respond(ctx, request, options)
}
