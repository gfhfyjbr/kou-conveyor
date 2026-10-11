package cockpit_test

import (
	"context"
	"encoding/json/v2"
	"errors"
	"slices"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// Stop tells a runner that takes stop lines why it is stopped before it
// interrupts it, and the runner records the reason as a hard stop. A runner
// built before is only interrupted, as before.
func TestStopTellsTheRunnerWhy(t *testing.T) {
	for _, test := range []struct {
		name string
		old  bool
		want []string
	}{
		{name: "runner takes stop lines", want: []string{"the user stopped the run"}},
		{name: "runner built before", old: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			cockpit.ForgetRunners()
			t.Cleanup(cockpit.ForgetRunners)
			if test.old {
				t.Setenv(cockpittest.NoStop, "1")
			}
			job := start(t, t.Context(), t.TempDir(), "session-1", "wait for me")
			select {
			case <-job.Lines():
			case <-time.After(10 * time.Second):
				t.Fatal("runner produced no output")
			}
			job.Stop("the user  stopped\n the run")
			var reasons []string
			for line := range job.Lines() {
				var item sessionstore.Item
				if line.Stderr || json.Unmarshal([]byte(line.Text), &item) != nil {
					continue
				}
				if input, ok := item.Data.(inbox.Input); ok && input.Kind == inbox.InputControl {
					if control, err := input.DecodeControlMessage(); err == nil && control.Mode == inbox.StopHard {
						reasons = append(reasons, control.Reason)
					}
				}
			}
			if err := job.Err(); !errors.Is(err, context.Canceled) {
				t.Fatalf("err = %v", err)
			}
			if !slices.Equal(reasons, test.want) {
				t.Fatalf("recorded stops = %q, want %q", reasons, test.want)
			}
		})
	}
}
