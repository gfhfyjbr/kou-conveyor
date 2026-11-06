package canvas

import (
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func programTestInstance() (*terminalInstance, *Node) {
	n := &Node{ID: "n", Kind: KindTerminal, Preset: "test-agent", Title: "Agent"}
	c := &Canvas{
		doc: &Doc{Nodes: []*Node{n}}, ws: Workspace{ID: "w"},
		states: map[string]*nodeState{n.ID: newState(StateStarting, "")},
		hub:    newHub("test"), kick: make(chan struct{}, 1), waits: map[string]*replyWait{},
		e: &Engine{presets: map[string]presetCache{"w": {at: time.Now()}}},
	}
	ti := &terminalInstance{
		c: c, id: n.ID, harnessUp: true, probed: time.Now(),
		def:        harnessDef{Harness: plugin.CanvasHarness{ID: "test-agent", Title: "Agent", Status: []string{"hooks", "idle"}, Output: "none"}},
		lastActive: time.Now().Add(-time.Hour), activeSince: time.Now().Add(-time.Hour),
	}
	c.states[n.ID].inst = ti
	return ti, n
}

func rootReport(r terminal.ProgramStatus) terminal.ProgramEvent {
	return terminal.ProgramEvent{Root: true, State: terminal.ProgramState{Reported: true, Records: []terminal.ProgramStatus{r}}}
}

func TestProgramStatusDrivesCanvasAndOutranksHeuristics(t *testing.T) {
	ti, _ := programTestInstance()
	for _, tc := range []struct{ report, kind, state string }{
		{"working", "", StateBusy},
		{"blocked", "permission", StateWaiting},
		{"working", "", StateBusy},
		{"blocked", "auth", StateWaiting},
		{"idle", "", StateIdle},
		{"done", "", StateIdle},
		{"error", "", StateError},
	} {
		r := terminal.ProgramStatus{State: tc.report, Kind: tc.kind, Message: "status from program"}
		ti.programStatus(rootReport(r))
		got := ti.c.status(ti.id)
		if got.State != tc.state || got.Program == nil || !reflect.DeepEqual(*got.Program, r) || ti.asking.Load() != (tc.report == "blocked") {
			t.Fatalf("%s: status %#v, asking %v", tc.report, got, ti.asking.Load())
		}
		// Old hooks and silence timers used to invent idle after a long
		// tool call. Once this program reports, time alone says nothing.
		ti.heuristics()
		if after := ti.c.status(ti.id); !reflect.DeepEqual(after, got) {
			t.Fatalf("heuristics replaced explicit %s: %#v -> %#v", tc.report, got, after)
		}
	}
	// Reset is per program: the next program can use the old fallback.
	ti.programStatus(terminal.ProgramEvent{})
	if ti.reported.Load() || ti.asking.Load() || ti.c.status(ti.id).Program != nil {
		t.Fatal("lifecycle reset retained explicit status")
	}
	ti.c.setStatus(ti.id, StateBusy, "")
	ti.heuristics()
	if got := ti.c.status(ti.id); got.State != StateIdle {
		t.Fatalf("a new nonreporting program lost the existing idle fallback: %#v", got)
	}
}

func TestProgramStatusBlockedAndDeliveryReservation(t *testing.T) {
	ti, n := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	st := ti.c.states[ti.id]
	if !ti.c.readyLocked(n, st, DeliverQueue) {
		t.Fatal("reported idle should take a prompt")
	}
	ti.markDelivered()
	if st.status.State != StateIdle || ti.c.readyLocked(n, st, DeliverQueue) {
		t.Fatal("delivery fabricated busy or allowed another queued prompt before working")
	}
	child := rootReport(terminal.ProgramStatus{State: "idle"})
	child.Root = false
	child.State.Records = append(child.State.Records, terminal.ProgramStatus{ID: "worker", State: "blocked", Kind: "permission"})
	ti.programStatus(child)
	if !ti.awaitingReport.Load() || ti.asking.Load() || st.status.State != StateIdle {
		t.Fatal("a worker report acknowledged a parent prompt or blocked the parent")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	if ti.awaitingReport.Load() || ti.c.readyLocked(n, st, DeliverQueue) {
		t.Fatal("working did not acknowledge delivery or allowed a queued prompt")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", Kind: "permission"}))
	if ti.c.readyLocked(n, st, DeliverQueue) || ti.c.readyLocked(n, st, DeliverNow) {
		t.Fatal("automated dispatch could type into a reported permission question")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	if ti.asking.Load() {
		t.Fatal("blocked leaked into the next working report")
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "done"}))
	if !ti.c.readyLocked(n, st, DeliverQueue) {
		t.Fatal("done did not release the queued prompt")
	}
	// A prompt may complete immediately with another idle report.
	ti.markDelivered()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "idle"}))
	if ti.awaitingReport.Load() || !ti.c.readyLocked(n, st, DeliverQueue) {
		t.Fatal("an unchanged root idle report did not release the reservation")
	}
}

func TestProgramStatusDoneAndErrorSettleOnlyTheCompletedTurn(t *testing.T) {
	for _, state := range []string{"done", "idle", "error"} {
		t.Run(state, func(t *testing.T) {
			ti, _ := programTestInstance()
			ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
			st := ti.c.states[ti.id]
			st.cause = &Message{ID: "m", Actor: "parent", To: Port{Node: ti.id}}
			wait := &replyWait{done: make(chan struct{})}
			ti.c.waits["m"] = wait
			ti.programStatus(rootReport(terminal.ProgramStatus{State: state, Message: "reported result"}))
			select {
			case <-wait.done:
			default:
				t.Fatal("explicit completion left CanvasSend waiting")
			}
			if st.cause != nil || wait.reason == "" || st.unanswered != 1 {
				t.Fatalf("completion: cause %#v, reason %q, unanswered %d", st.cause, wait.reason, st.unanswered)
			}
			if state == "error" && (!strings.Contains(wait.reason, "reported result") || st.status.State != StateError) {
				t.Fatalf("error was reported as a successful/idle turn: %#v, %q", st.status, wait.reason)
			}
			ti.programStatus(rootReport(terminal.ProgramStatus{State: state}))
			if st.unanswered != 1 {
				t.Fatal("a repeated completion finished the same turn again")
			}
		})
	}
	// A done report can arrive before the hook containing the answer.
	ti, _ := programTestInstance()
	ti.def.Harness.Output = "hooks"
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working"}))
	st := ti.c.states[ti.id]
	st.cause = &Message{ID: "m", Actor: "parent"}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "done"}))
	if st.cause == nil || st.unanswered != 0 {
		t.Fatal("done prematurely discarded an answer that still comes through a hook")
	}
	if ti.c.readyLocked(ti.c.doc.node(ti.id), st, DeliverQueue) {
		t.Fatal("done let the next prompt steal the pending hook answer's cause")
	}
	ti.c.unansweredLocked(ti.id, "answer received")
	if !ti.c.readyLocked(ti.c.doc.node(ti.id), st, DeliverQueue) {
		t.Fatal("the completed hook turn did not release queued delivery")
	}
}

func TestProgramStatusClearAndNewProgramDoNotLeakFacts(t *testing.T) {
	ti, _ := programTestInstance()
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", Kind: "question"}))
	ti.programStatus(terminal.ProgramEvent{Root: true, State: terminal.ProgramState{Reported: true}})
	if !ti.reported.Load() || ti.asking.Load() || ti.c.status(ti.id).Program != nil || ti.c.status(ti.id).State != StateIdle {
		t.Fatal("clear kept blocked, or returned to heuristic state for the same program")
	}
	ti.heuristics()
	if ti.c.status(ti.id).State != StateIdle {
		t.Fatal("clear re-enabled heuristics")
	}
	ti.markDelivered()
	ti.programStatus(terminal.ProgramEvent{})
	if ti.reported.Load() || ti.awaitingReport.Load() || ti.c.status(ti.id).Program != nil {
		t.Fatal("a different foreground program inherited reporting or delivery state")
	}
}

func TestProgramStatusAppIdentifiesAnAgentWithoutTitleChanges(t *testing.T) {
	ti, _ := programTestInstance()
	ti.harnessUp, ti.shell = false, true
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "working", App: "claude-code"}))
	st := ti.c.status(ti.id)
	if st.Agent == nil || st.Agent.Program != "claude-code" || st.Agent.ID != "claude" || st.State != StateBusy {
		t.Fatalf("stable app id was not recognized independently of a title: %#v", st)
	}
	ti.programStatus(rootReport(terminal.ProgramStatus{State: "blocked", App: "claude-code", Kind: "permission"}))
	if got := ti.c.status(ti.id); got.Agent == nil || got.State != StateWaiting {
		t.Fatalf("reported permission state lost agent identity: %#v", got)
	}
}
