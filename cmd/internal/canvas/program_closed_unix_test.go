//go:build unix

package canvas

import (
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusInferredExitPreservesBufferedAnswerCause(t *testing.T) {
	for _, result := range []string{"done", "error"} {
		t.Run(result, func(t *testing.T) {
			ti, node := programTestInstance()
			node.Config = []byte(`{"output":"explicit"}`) // direct wait, no output journal
			ti.def.Harness.Output = "screen"
			m := terminal.NewManager(nil, "test")
			defer m.Close()
			s, err := m.Start(terminal.Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh", Cols: 80, Rows: 24})
			if err != nil {
				t.Fatal(err)
			}
			ti.session = s
			if err := s.Write([]byte("printf 'buffered-answer-%s\\n' 7501\n")); err != nil {
				t.Fatal(err)
			}
			eventually(t, 5*time.Second, "buffered answer on screen", func() bool {
				return strings.Contains(s.Tail(24), "buffered-answer-7501")
			})
			ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
			st := ti.c.states[ti.id]
			cause := &Message{ID: "a", Actor: "parent"}
			wait := &replyWait{done: make(chan struct{})}
			st.cause, ti.c.waits["a"] = cause, wait
			ended := terminal.ProgramEvent{Ended: true, State: terminal.ProgramState{Closed: true}}
			ti.programStatus(ended) // an out-of-band probe/Observe got here before output
			select {
			case <-wait.done:
				t.Fatal("inferred exit settled the original cause before buffered output")
			default:
			}
			if st.cause != cause || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatal("inferred exit lost its cause or admitted shell input")
			}
			final := rootReport(terminal.ProgramStatus{State: result, Message: "final failure"})
			final.State.Closed = true
			// Covers both a later chunk and the same event's Ended→final→Ended.
			ti.handle(terminal.Event{Programs: []terminal.ProgramEvent{ended, final, ended}})
			select {
			case <-wait.done:
			default:
				t.Fatal("buffered final report did not settle its original wait")
			}
			if result == "done" && (wait.reason != "" || !strings.Contains(wait.answer, "buffered-answer-7501")) {
				t.Fatalf("the actual final screen answer was lost: answer %q, reason %q", wait.answer, wait.reason)
			}
			if result == "error" && !strings.Contains(wait.reason, "final failure") {
				t.Fatalf("the actual final error was lost: %q", wait.reason)
			}
			if st.cause != nil || st.status.State != StateExited || ti.c.readyLocked(node, st, DeliverQueue) || ti.c.readyLocked(node, st, DeliverNow) {
				t.Fatal("the departed agent could receive a queued prompt as a shell command")
			}
		})
	}
}

func TestProgramStatusClosedScopeMissingAnswerCleansUpOnlyOnNewScope(t *testing.T) {
	ti, node := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	st := ti.c.states[ti.id]
	cause := &Message{ID: "a", Actor: "parent"}
	wait := &replyWait{done: make(chan struct{})}
	st.cause, ti.c.waits["a"] = cause, wait
	ti.programStatus(terminal.ProgramEvent{Ended: true, State: terminal.ProgramState{Closed: true}})
	live := rootReport(terminal.ProgramStatus{State: "blocked", Kind: "permission"})
	live.State.Closed = true
	ti.programStatus(live)
	ti.heuristics()
	if st.status.State != StateExited || ti.c.readyLocked(node, st, DeliverNow) {
		t.Fatal("stale live output or silence reopened the closed scope")
	}
	select {
	case <-wait.done:
		t.Fatal("missing final output fabricated an answer")
	default:
	}
	ti.programStatus(terminal.ProgramEvent{}) // verified new C/foreground scope
	select {
	case <-wait.done:
	default:
		t.Fatal("new scope did not clean up the prior missing answer")
	}
	if st.cause != nil || wait.reason == "" || ti.closing.Load() {
		t.Fatal("new scope inherited the old answer/admission state")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	if !ti.c.readyLocked(node, st, DeliverQueue) {
		t.Fatal("the verified new idle scope could not accept its own prompt")
	}
}
