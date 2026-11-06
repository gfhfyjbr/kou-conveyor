//go:build unix

package canvas

import (
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

func TestProgramStatusEmptyScreenCompletionSettlesItsCause(t *testing.T) {
	m := terminal.NewManager(nil, "test")
	defer m.Close()
	// cat is deliberately quiet: no prompt or startup output can make an
	// empty rendered screen accidentally look like a nonempty answer.
	s, err := m.Start(terminal.Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/cat", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	for _, state := range []string{"done", "idle"} {
		t.Run(state, func(t *testing.T) {
			ti, node := programTestInstance()
			node.Config = []byte(`{"output":"explicit"}`)
			ti.session, ti.def.Harness.Output = s, "screen"
			if screen := s.Tail(s.Info().Rows); strings.TrimSpace(screen) != "" {
				t.Fatalf("fixture must have an empty rendered screen: %q", screen)
			}
			st := ti.c.states[ti.id]
			cause := &Message{ID: "a", Actor: "parent"}
			wait := &replyWait{done: make(chan struct{})}
			st.cause, ti.c.waits[cause.ID] = cause, wait
			ti.programStatus(rootReport(terminal.ProgramStatus{State: state}))
			select {
			case <-wait.done:
			default:
				t.Fatal("empty explicit completion left its cause/wait stuck")
			}
			if st.cause != nil || wait.answer != "" || wait.reason != "the program answered nothing" || st.unanswered != 1 {
				t.Fatalf("empty completion: cause %#v, answer %q, reason %q", st.cause, wait.answer, wait.reason)
			}
			if !ti.c.readyLocked(node, st, DeliverQueue) {
				t.Fatal("empty completion did not release the completed cause's admission guard")
			}
		})
	}
}
