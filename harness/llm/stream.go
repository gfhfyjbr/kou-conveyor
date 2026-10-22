package llm

import "context"

// StreamKind is what a piece of a streaming response is the text of.
type StreamKind string

const (
	StreamText      StreamKind = "text"      // a message's text
	StreamReasoning StreamKind = "reasoning" // the reasoning the model shows
)

// StreamEvent is a piece of a response as the provider streams it, before
// the response is complete.
type StreamEvent struct {
	// Index is the output item the piece is of, as the provider numbers the
	// items it streams. It need not be the item's index in the response's
	// Output, which leaves out what the harness does not keep.
	Index int
	Kind  StreamKind
	Text  string
	// Reset says that the response starts over, as when its request is
	// tried again: what streamed of it before is void.
	Reset bool
}

type streamReporter struct{}

// WithStreamReporter returns a context whose model requests call report with
// the pieces of their responses as they stream. Nothing records a response
// before it is complete; a reporter only shows it sooner. report is called
// on the goroutine that reads the response and must not hold it up.
func WithStreamReporter(ctx context.Context, report func(StreamEvent)) context.Context {
	return context.WithValue(ctx, streamReporter{}, report)
}

// ReportStream tells the context's reporter, if there is one, of a piece of
// a response. A request that was canceled reports nothing more: a turn that
// replaces it streams its own response.
func ReportStream(ctx context.Context, event StreamEvent) {
	if ctx.Err() != nil {
		return
	}
	if report, ok := ctx.Value(streamReporter{}).(func(StreamEvent)); ok && report != nil {
		report(event)
	}
}
