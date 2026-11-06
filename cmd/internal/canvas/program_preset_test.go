package canvas

import (
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusPresetProbePreservesCauseBeforeFirstReport(t *testing.T) {
	ti, _ := programTestInstance()
	ti.harnessUp = false
	ti.launching.Store(true)
	st := ti.c.states[ti.id]
	cause := &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	st.cause, ti.c.waits[cause.ID] = cause, wait
	// No Session: a preset never enters destructive shell-agent discovery,
	// even before PS0 or the program's first OSC status has arrived.
	ti.probe()
	if st.cause != cause || ti.agent != nil || !ti.launching.Load() {
		t.Fatal("preset discovery changed its owned launch or delivered cause")
	}
	select {
	case <-wait.done:
		t.Fatal("preset discovery settled an unanswered prompt")
	default:
	}
}

func TestProgramStatusPresetWithoutStartMarkTracksManualActivity(t *testing.T) {
	for _, tc := range []struct {
		name, state, status, want string
	}{
		{"silence", StateIdle, "idle", StateBusy},
		{"question", StateWaiting, "hooks", StateBusy},
		{"hooks", StateIdle, "hooks", StateIdle},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ti, _ := programTestInstance()
			ti.harnessUp, ti.inLine = false, true // B, but old Bash never sends C
			ti.launching.Store(true)
			ti.def.Harness.Status = []string{tc.status}
			at := time.Now()
			st := ti.c.states[ti.id]
			st.status = Status{State: tc.state, Since: at.Add(-5 * time.Second)}
			ti.activeSince, ti.lastActive = at.Add(-2*time.Second), at.Add(-100*time.Millisecond)
			ti.followHarness(at, st.status)
			if st.status.State != tc.want || ti.agent != nil {
				t.Fatalf("no-C preset activity: %#v, discovered %#v", st.status, ti.agent)
			}
		})
	}
}

func TestProgramStatusPresetWithoutStartMarkSettlesOnlyItsCommandEnd(t *testing.T) {
	ti, node := programTestInstance()
	ti.harnessUp = false
	ti.launching.Store(true) // fallback typed the launch before the first real prompt
	st := ti.c.states[ti.id]
	st.status = Status{State: StateBusy}
	cause := &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	st.cause, ti.c.waits[cause.ID] = cause, wait
	ti.handle(terminal.Event{Marks: []terminal.Mark{{Kind: 'D', Code: 0}}})
	if st.cause != cause || !ti.launching.Load() || st.status.State != StateBusy {
		t.Fatal("the initial shell D consumed a not-yet-started preset launch")
	}
	select {
	case <-wait.done:
		t.Fatal("the initial shell D settled the preset's prompt")
	default:
	}
	ti.mark(terminal.Mark{Kind: 'B'})
	ti.handle(terminal.Event{Marks: []terminal.Mark{{Kind: 'D', Code: 7}}})
	select {
	case <-wait.done:
	default:
		t.Fatal("the preset's no-C command end left its wait open")
	}
	if st.cause != nil || ti.launching.Load() || ti.harnessUp || ti.capturing || ti.inLine || st.status.Agent != nil || st.status.State != StateExited || !strings.Contains(wait.reason, "Agent exited (7)") {
		t.Fatalf("no-C preset exit: %#v, cause %#v, reason %q", st.status, st.cause, wait.reason)
	}
	if ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("an exited preset admitted a queued prompt into its shell")
	}
	ti.mark(terminal.Mark{Kind: 'D'})
	if st.status.State != StateExited || st.status.Detail != "Agent exited (7)" {
		t.Fatal("a repeated D finalized the same preset launch again")
	}
}
