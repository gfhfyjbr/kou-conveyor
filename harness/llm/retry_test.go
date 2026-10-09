package llm

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestReportRetryReachesTheContextsReporter(t *testing.T) {
	ReportRetry(t.Context(), Retry{Attempt: 1}) // no reporter: nothing to tell
	var got []Retry
	ctx := WithRetryReporter(t.Context(), func(r Retry) { got = append(got, r) })
	child, cancel := context.WithCancel(ctx)
	defer cancel()
	want := Retry{Attempt: 2, MaxAttempts: 5, Delay: 4 * time.Second, Err: errors.New("connection refused")}
	ReportRetry(child, want)
	if len(got) != 1 || got[0] != want {
		t.Fatalf("reported %+v", got)
	}
}
