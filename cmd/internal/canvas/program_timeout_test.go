package canvas

import (
	"context"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusClosedScopeUsesExistingWaitTimeoutWithoutReopening(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	st := ti.c.states[ti.id]
	cause := &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	st.cause, ti.c.waits["a"] = cause, wait
	ti.programStatus(terminal.ProgramEvent{Ended: true, State: terminal.ProgramState{Closed: true}})
	answer, reason, timedOut, err := ti.c.awaitReply(context.Background(), "a", wait, time.Second)
	if !timedOut || err != nil || answer != "" || reason != "" {
		t.Fatalf("existing timeout changed: answer %q, reason %q, timedOut %v, err %v", answer, reason, timedOut, err)
	}
	ti.c.mu.Lock()
	if ti.c.waits["a"] != nil || st.cause != cause || ti.c.readyLocked(node, st, DeliverNow) || ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatal("timeout retained its wait entry, discarded attribution, or reopened shell delivery")
	}
	ti.c.mu.Unlock()
	ti.programStatus(terminal.ProgramEvent{}) // a verified new C/foreground
	if st.cause != nil || ti.closing.Load() {
		t.Fatal("verified new scope retained the old cause or admission gate")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	if !ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatal("new idle program cannot accept its own prompt")
	}
}
