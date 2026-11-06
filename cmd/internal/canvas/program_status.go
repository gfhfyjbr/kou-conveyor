package canvas

import (
	"reflect"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

// A report and co-located shell marks form one admission transition. Status
// publications may wake dispatch, but no prompt can enter halfway through it.
func (t *terminalInstance) beginEvent() {
	t.c.mu.Lock()
	t.handling++
	t.c.mu.Unlock()
}

func (t *terminalInstance) endEvent() {
	t.c.mu.Lock()
	t.handling--
	if t.handling == 0 {
		t.c.kickDispatch()
	}
	t.c.mu.Unlock()
}

func programCanvasStatus(report *terminal.ProgramStatus) (state, detail string) {
	state = StateIdle
	if report == nil {
		return
	}
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
	return
}

func (t *terminalInstance) programExitedDetail() string {
	if t.def.Harness.Title != "" {
		return t.def.Harness.Title + " exited"
	}
	return "the program exited"
}

// App attribution is not command-start detection: the first report may
// arrive only after a prompt was delivered. It must not settle that prompt.
func (t *terminalInstance) programAgentLocked(report *terminal.ProgramStatus) {
	if report == nil || report.App == "" || !t.shell || t.harnessUp {
		return
	}
	f, ok := agentIn(t.c.e.catalog(t.c.ws), []string{report.App})
	if !ok {
		return
	}
	if f.input == nil {
		f.input = &agentInput
	}
	if f.idle <= 0 {
		f.idle = agentIdle
	}
	t.agent, t.interactive = &f, true
	t.mu.Lock()
	t.input = f.input
	t.mu.Unlock()
	t.c.setAgentLocked(t.id, f.agent)
}

// bootstrapProgramLocked runs before an adopted terminal is published ready,
// using the snapshot supplied atomically with observer registration.
func (t *terminalInstance) bootstrapProgramLocked(program terminal.ProgramState) (string, string, bool) {
	t.c.states[t.id].status.Program = nil
	t.reported.Store(program.Reported)
	t.closing.Store(program.Closed)
	if !program.Reported {
		if program.Closed {
			return StateExited, t.programExitedDetail(), true
		}
		return "", "", false
	}
	t.program = program.Record("")
	t.asking.Store(t.program != nil && t.program.State == "blocked")
	t.c.states[t.id].status.Program = t.program
	t.programAgentLocked(t.program)
	if program.Closed {
		return StateExited, t.programExitedDetail(), true
	}
	state, detail := programCanvasStatus(t.program)
	return state, detail, true
}

// programStatus uses only the root record for the node's state. A worker
// can be blocked while its parent is idle; those are separate facts. The
// reservation, cause, report and admission flags change under the same lock.
func (t *terminalInstance) programStatus(ev terminal.ProgramEvent) {
	t.c.mu.Lock()
	defer t.c.mu.Unlock()
	st := t.c.states[t.id]
	if st == nil || st.inst != t || t.c.deleted {
		return
	}
	wasReported := t.reported.Swap(ev.State.Reported)
	wasClosed := t.closing.Swap(ev.State.Closed)
	previous := t.program
	if !ev.State.Reported {
		t.program = nil
		t.awaitingReport.Store(false)
		if wasClosed && !ev.State.Closed {
			// A new program can no longer answer the previous one's prompt.
			t.c.unansweredLocked(t.id, t.programExitedDetail()+" before answering")
		}
		if wasReported || wasClosed || ev.State.Closed {
			t.asking.Store(false)
			changed := st.status.Program != nil
			st.status.Program = nil
			state, detail := StateBusy, ""
			if !t.shell {
				state, detail = StateStarting, "starting "+t.def.Harness.Title
			}
			if ev.State.Closed {
				state, detail = StateExited, t.programExitedDetail()
				// Do not settle the cause here: the exited process's done
				// report and answer may still be buffered in the PTY.
			}
			before := st.status
			t.c.setStatusLocked(t.id, state, detail)
			if changed && before.State == state && before.Detail == detail {
				t.c.hub.publish(map[string]any{"type": "status", "node": t.id, "status": st.status})
			}
		}
		return
	}
	report := ev.State.Record("")
	if ev.Root {
		t.awaitingReport.Store(false)
	}
	if wasReported && !ev.Root && wasClosed == ev.State.Closed && reflect.DeepEqual(previous, report) {
		return
	}
	t.program = report
	t.asking.Store(report != nil && report.State == "blocked")
	t.programAgentLocked(report)
	state, detail := programCanvasStatus(report)
	if ev.State.Closed {
		state, detail = StateExited, t.programExitedDetail()
	}
	finished := report != nil && ev.Root && st.cause != nil && (report.State == "idle" || report.State == "done" || report.State == "error")
	var out Output
	reason := ""
	if finished {
		switch {
		case report.State == "error":
			reason = "the program reported an error"
			if message := cleanTitle(report.Message); message != "" {
				reason += ": " + message
			}
		case t.agent != nil || t.def.Harness.Output == "screen":
			out = Output{Port: "out", Text: strings.Trim(t.session.Tail(t.session.Info().Rows), "\n")}
			if strings.TrimSpace(out.Text) == "" {
				reason = "the program answered nothing"
			}
		case t.def.Harness.Output == "none":
			reason = "the program's answer is on its screen alone: read it"
		}
		// A hooks harness still gives its answer through the hook. The
		// explicit report may precede it, so don't settle that wait early.
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
