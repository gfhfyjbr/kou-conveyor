package canvas

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusQueuedObserverUpdateBlocksAdmissionBeforeDrain(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	// The observer may enqueue this while the initial snapshot/batch is
	// being drained. It must precede any newly admitted automated input.
	ti.observe(terminal.Event{Programs: []terminal.ProgramEvent{rootReport(terminal.ProgramStatus{State: "blocked", Kind: "permission"})}})
	ti.c.mu.Lock()
	st := ti.c.states[ti.id]
	if st.status.State != StateIdle || ti.handling != 0 {
		t.Fatal("test must inspect the pre-drain idle admission window")
	}
	if ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("already parsed blocked report was bypassed before the loop consumed it")
	}
	ti.c.mu.Unlock()
	ti.beginEvent()
	for _, ev := range ti.take() {
		ti.handle(ev)
	}
	ti.endEvent()
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	if st.status.State != StateWaiting || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("draining the observer update lost permission-prompt admission")
	}
}

func TestProgramStatusQueuedShellMarkBlocksAdmissionBeforeDrain(t *testing.T) {
	for _, kind := range []byte{'C', 'D'} {
		t.Run(string(kind), func(t *testing.T) {
			ti, node := programTestInstance()
			ti.c.setStatus(ti.id, StateIdle, "")
			if kind == 'C' {
				ti.launching.Store(true)
			}
			ti.observe(terminal.Event{Marks: []terminal.Mark{{Kind: kind}}})
			ti.c.mu.Lock()
			st := ti.c.states[ti.id]
			if ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatal("already parsed lifecycle mark was bypassed before drain")
			}
			ti.c.mu.Unlock()
			ti.beginEvent()
			for _, ev := range ti.take() {
				ti.handle(ev)
			}
			ti.endEvent()
			want := StateExited
			if kind == 'C' {
				want = StateStarting
			}
			if st.status.State != want || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatalf("lifecycle drain lost final admission: %#v", st.status)
			}
		})
	}
}
