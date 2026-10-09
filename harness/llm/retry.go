package llm

import (
	"context"
	"time"
)

// Retry describes a model request that failed and is sent again.
type Retry struct {
	Attempt     int           // the attempt that failed, from 1
	MaxAttempts int           // how many attempts the request has
	Delay       time.Duration // the wait before the next attempt
	Err         error         // why the attempt failed
}

type retryReporter struct{}

// WithRetryReporter returns a context whose model requests call report for
// every attempt that failed and is tried again, before they wait to try it.
// An adapter retries a request on its own, for minutes when the provider is
// unreachable, and nothing else says meanwhile that the model is not
// answering.
func WithRetryReporter(ctx context.Context, report func(Retry)) context.Context {
	return context.WithValue(ctx, retryReporter{}, report)
}

// ReportRetry tells the context's reporter, if there is one, that a request
// is tried again. Adapters call it before they wait to retry.
func ReportRetry(ctx context.Context, retry Retry) {
	if report, ok := ctx.Value(retryReporter{}).(func(Retry)); ok && report != nil {
		report(retry)
	}
}
