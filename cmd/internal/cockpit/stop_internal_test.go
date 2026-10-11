package cockpit

import (
	"context"
	"errors"
	"testing"
)

// why is what Stop was told, else the cockpit's shutting down, else that
// nobody said.
func TestWhyARunIsStopped(t *testing.T) {
	parent, shutDown := context.WithCancelCause(context.Background())
	told, stop := context.WithCancelCause(parent)
	stop(stopReason("the user stopped the run from the cockpit"))
	untold, cancel := context.WithCancelCause(parent)
	cancel(nil)
	if got := why(parent, told); got != "the user stopped the run from the cockpit" {
		t.Errorf("told: %q", got)
	}
	if got := why(parent, untold); got != "the cockpit stopped the run without saying why" {
		t.Errorf("untold: %q", got)
	}
	shutDown(errors.New("interrupt signal received"))
	running, cancelRun := context.WithCancelCause(parent)
	defer cancelRun(nil)
	if got := why(parent, running); got != "the cockpit stopped its runs as it shut down (interrupt signal received)" {
		t.Errorf("shutdown: %q", got)
	}
	// What Stop was told comes first, even once the cockpit shuts down.
	if got := why(parent, told); got != "the user stopped the run from the cockpit" {
		t.Errorf("told, then shutdown: %q", got)
	}
}
