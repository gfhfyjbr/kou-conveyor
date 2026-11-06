package canvas

import (
	"reflect"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

// programStatus uses only the root record for the node's state. A worker
// can be blocked while its parent is idle; those are separate facts.
func (t *terminalInstance) programStatus(ev terminal.ProgramEvent) {
	wasReported := t.reported.Swap(ev.State.Reported)
	previous := t.program
	if !ev.State.Reported {
		t.program = nil
		t.awaitingReport.Store(false)
		if wasReported {
			t.asking.Store(false)
			t.c.mu.Lock()
			if st := t.c.states[t.id]; st != nil && st.inst == t {
				st.status.Program = nil
				t.c.setStatusLocked(t.id, StateIdle, "")
				t.c.hub.publish(map[string]any{"type": "status", "node": t.id, "status": st.status})
			}
			t.c.mu.Unlock()
		}
		return
	}
	report := ev.State.Record("")
	awaited := false
	if ev.Root {
		awaited = t.awaitingReport.Swap(false)
	}
	if wasReported && !ev.Root && reflect.DeepEqual(previous, report) {
		return
	}
	t.program = report
	t.asking.Store(report != nil && report.State == "blocked")
	if report != nil && report.App != "" && t.shell && !t.harnessUp {
		if f, ok := agentIn(t.c.e.catalog(t.c.ws), []string{report.App}); ok {
			// A stable app id also identifies agents whose launcher or
			// process title isn't recognizable on this platform.
			t.foundAgent(f, false)
		}
	}
	state, detail := StateIdle, ""
	if report != nil {
		detail = cleanTitle(report.Message)
		switch report.State {
		case "working":
			state = StateBusy
		case "blocked":
			state = StateWaiting
			if detail == "" {
				detail = report.Kind
			}
		case "done":
			if detail == "" {
				detail = "done"
			}
		case "error":
			state = StateError
		}
	}
	finished := report != nil && ev.Root && (report.State == "idle" || report.State == "done" || report.State == "error") &&
		(awaited || previous != nil && (previous.State == "working" || previous.State == "blocked"))
	var out Output
	reason := ""
	if finished {
		switch {
		case report.State == "error":
			reason = "the program reported an error"
			if detail != "" {
				reason += ": " + detail
			}
		case t.agent != nil || t.def.Harness.Output == "screen":
			out = Output{Port: "out", Text: strings.Trim(t.session.Tail(t.session.Info().Rows), "\n")}
		case t.def.Harness.Output == "none":
			reason = "the program's answer is on its screen alone: read it"
		}
		// A hooks harness still gives its answer through the hook. The
		// explicit report may precede it, so don't settle that wait early.
	}
	t.c.mu.Lock()
	defer t.c.mu.Unlock()
	st := t.c.states[t.id]
	if st == nil || st.inst != t || t.c.deleted {
		return
	}
	if finished && st.cause != nil && (out.Text != "" || reason != "") {
		t.c.answerLocked(t.id, out, []*Message{st.cause}, reason)
	}
	changed := !reflect.DeepEqual(st.status.Program, report) || st.status.Quiet
	st.status.Program = report
	st.status.Quiet = false
	before := st.status
	t.c.setStatusLocked(t.id, state, detail)
	if changed && before.State == state && before.Detail == detail {
		t.c.hub.publish(map[string]any{"type": "status", "node": t.id, "status": st.status})
		st.wake()
	}
	if ev.Root && state == StateIdle {
		t.c.kickDispatch()
	}
}
