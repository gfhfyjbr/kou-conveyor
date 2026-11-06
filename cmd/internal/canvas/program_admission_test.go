package canvas

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusLifecycleBatchNeverAdmitsBetweenResetAndMark(t *testing.T) {
	for _, mark := range []byte{'C', 'D'} {
		t.Run(string(mark), func(t *testing.T) {
			ti, node := programTestInstance()
			ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", Kind: "permission"}))
			ti.capturing = true
			if mark == 'C' {
				ti.launching.Store(true)
			}
			// Hold the same batch gate handle uses, then inspect the exact
			// old failure point: reset and mark have the same byte offset.
			ti.beginEvent()
			ti.programStatus(terminal.ProgramEvent{})
			ti.c.mu.Lock()
			st := ti.c.states[ti.id]
			if ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatal("reset admitted text before its co-located shell mark")
			}
			ti.c.mu.Unlock()
			ti.mark(terminal.Mark{Kind: mark, Code: 0})
			ti.endEvent()
			want := StateExited
			if mark == 'C' {
				want = StateStarting
			}
			ti.c.mu.Lock()
			defer ti.c.mu.Unlock()
			if st.status.State != want || st.status.Program != nil || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatalf("final boundary state/admission: %#v", st.status)
			}
		})
	}
}

func TestProgramStatusBootstrapBlockedBeforePublishingReady(t *testing.T) {
	ti, node := programTestInstance()
	program := terminal.ProgramState{Reported: true, Records: []terminal.ProgramStatus{{State: "blocked", App: "claude-code", Kind: "permission"}}}
	ti.c.mu.Lock()
	ti.handling = 1 // startup gate, released only after the observer drain
	state, detail, ok := ti.bootstrapProgramLocked(program)
	if !ok {
		t.Fatal("retained reporter was ignored")
	}
	ti.c.setStatusLocked(ti.id, state, detail)
	st := ti.c.states[ti.id]
	if state != StateWaiting || !ti.asking.Load() || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatalf("bootstrap published an admissible blocked terminal: %#v", st.status)
	}
	ti.c.mu.Unlock()
	// Applying Observe's same initial snapshot must not open a window.
	ti.handle(terminal.Event{Programs: []terminal.ProgramEvent{{State: program}}})
	ti.endEvent()
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	if ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("the initial observer drain lost blocked admission")
	}
}

func TestProgramStatusFirstAppReportKeepsDeliveredCause(t *testing.T) {
	ti, _ := programTestInstance()
	ti.harnessUp, ti.shell = false, true
	st := ti.c.states[ti.id]
	cause := &Message{ID: "m", Actor: "parent"}
	st.cause = cause
	wait := &replyWait{done: make(chan struct{})}
	ti.c.waits[cause.ID] = wait
	ti.markDelivered() // this program has never reported, until working below
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working", App: "claude-code"}))
	if st.cause != cause || st.status.Agent == nil {
		t.Fatal("app attribution discarded the already delivered prompt")
	}
	select {
	case <-wait.done:
		t.Fatal("app attribution prematurely settled CanvasSend")
	default:
	}
	// A later explicit failure still belongs to that original prompt.
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "error", App: "claude-code"}))
	select {
	case <-wait.done:
	default:
		t.Fatal("the report's eventual completion lost its original cause")
	}
}

func TestProgramStatusProbeCannotOverrideExecOrShellReporter(t *testing.T) {
	ti, _ := programTestInstance()
	ti.harnessUp, ti.shell = false, true
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", App: "claude-code", Kind: "auth"}))
	// No Session is needed: a valid reporting program must not even enter
	// foreground inference, including when it exec'ed over the shell PID.
	ti.probe()
	if st := ti.c.status(ti.id); st.State != StateWaiting || st.Agent == nil || !ti.asking.Load() {
		t.Fatalf("probe replaced explicit status: %#v", st)
	}
}

func TestProgramStatusRepeatedIdleSettlesOnlyReservedCause(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	st := ti.c.states[ti.id]
	ti.markDelivered()
	st.cause = &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	ti.c.waits["a"] = wait
	ti.beginEvent()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	ti.c.mu.Lock()
	if ti.c.readyLocked(node, st, DeliverNow) || st.cause != nil {
		t.Fatal("the repeated-idle transaction exposed a new cause before settling A")
	}
	select {
	case <-wait.done:
	default:
		t.Fatal("the repeated idle report did not finish its reserved prompt")
	}
	ti.c.mu.Unlock()
	ti.endEvent()
	ti.c.mu.Lock()
	defer ti.c.mu.Unlock()
	if !ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatal("completed repeated-idle transaction did not reopen admission")
	}
}
