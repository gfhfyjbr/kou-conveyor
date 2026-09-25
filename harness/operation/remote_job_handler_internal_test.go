package operation

import "testing"

func TestRemoteJobStatusTransitions(t *testing.T) {
	for _, test := range []struct {
		name    string
		current Status
		updated Status
		valid   bool
	}{
		{name: "ready awaiting", current: StatusReady, updated: StatusAwaiting, valid: true},
		{name: "ready canceling", current: StatusReady, updated: StatusCanceling, valid: true},
		{name: "ready completed", current: StatusReady, updated: StatusCompleted, valid: true},
		{name: "ready failed", current: StatusReady, updated: StatusFailed, valid: true},
		{name: "ready canceled", current: StatusReady, updated: StatusCanceled, valid: true},
		{name: "ready remains ready", current: StatusReady, updated: StatusReady},
		{name: "awaiting progress", current: StatusAwaiting, updated: StatusAwaiting, valid: true},
		{name: "awaiting canceling", current: StatusAwaiting, updated: StatusCanceling, valid: true},
		{name: "awaiting completed", current: StatusAwaiting, updated: StatusCompleted, valid: true},
		{name: "awaiting failed", current: StatusAwaiting, updated: StatusFailed, valid: true},
		{name: "awaiting canceled", current: StatusAwaiting, updated: StatusCanceled, valid: true},
		{name: "awaiting regresses", current: StatusAwaiting, updated: StatusReady},
		{name: "canceling progress", current: StatusCanceling, updated: StatusCanceling, valid: true},
		{name: "canceling failed", current: StatusCanceling, updated: StatusFailed, valid: true},
		{name: "canceling canceled", current: StatusCanceling, updated: StatusCanceled, valid: true},
		{name: "canceling completed", current: StatusCanceling, updated: StatusCompleted},
		{name: "terminal update", current: StatusCompleted, updated: StatusCompleted},
		{name: "unknown current", current: "unknown", updated: StatusFailed},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := validRemoteJobStatusTransition(test.current, test.updated); got != test.valid {
				t.Fatalf("validRemoteJobStatusTransition(%q, %q) = %t, want %t", test.current, test.updated, got, test.valid)
			}
		})
	}
}
