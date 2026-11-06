package canvas

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusFirstCompletionAfterDeliverySettlesItsCause(t *testing.T) {
	for _, state := range []string{"idle", "done", "error"} {
		t.Run(state, func(t *testing.T) {
			ti, _ := programTestInstance()
			if state == "error" {
				ti.def.Harness.Output = "hooks"
			}
			st := ti.c.states[ti.id]
			st.cause = &Message{ID: "a", Actor: "parent"}
			wait := &replyWait{done: make(chan struct{})}
			ti.c.waits["a"] = wait
			ti.markDelivered()
			if ti.awaitingReport.Load() {
				t.Fatal("the program had never reported at delivery")
			}
			ti.programStatus(rootReport(terminal.ProgramStatus{State: state}))
			select {
			case <-wait.done:
			default:
				t.Fatalf("first %s report lost its delivered cause", state)
			}
			if st.cause != nil || wait.reason == "" {
				t.Fatalf("first completion left a stale cause: %#v, %q", st.cause, wait.reason)
			}
		})
	}
	// Merely announcing initial idle without a delivered cause is not an
	// answer and must not advance completion counters.
	ti, _ := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	if st := ti.c.states[ti.id]; st.outputs != 0 || st.unanswered != 0 {
		t.Fatal("initial idle invented an answer")
	}
}

func TestProgramStatusForegroundEndKeepsAnExitedHarnessUnready(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", Kind: "permission"}))
	ti.programStatus(terminal.ProgramEvent{Ended: true, State: terminal.ProgramState{Closed: true}})
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	st := ti.c.states[ti.id]
	if st.status.State != StateExited || ti.c.readyLocked(node, st, DeliverNow) || ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatalf("a departed harness could receive text as shell input: %#v", st.status)
	}
}
