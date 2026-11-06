package canvas

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusClearPreservesCauseUntilAnExplicitCompletion(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	st := ti.c.states[ti.id]
	cause := &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	st.cause, ti.c.waits["a"] = cause, wait
	ti.programStatus(terminal.ProgramEvent{Root: true, State: terminal.ProgramState{Reported: true}})
	select {
	case <-wait.done:
		t.Fatal("clear fabricated a completed answer")
	default:
	}
	ti.c.mu.Lock()
	if st.cause != cause || ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("clearing a record exposed its unfinished cause to the next prompt")
	}
	ti.c.mu.Unlock()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "done"}))
	select {
	case <-wait.done:
	default:
		t.Fatal("done after clear could not settle its original delivered cause")
	}
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	if st.cause != nil || wait.reason == "" || !ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatal("definitive completion did not finish and release the old cause")
	}
}
