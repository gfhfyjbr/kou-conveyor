package canvas

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"slices"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// agentHistory gives a session the prompts it ran, as its runner records
// them.
func agentHistory(t *testing.T, dir, id string, prompts ...string) {
	t.Helper()
	ctx := t.Context()
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	store, err := localfile.New(dir)
	must(err)
	_, err = store.Create(ctx, session.ID(id))
	must(err)
	var previous session.TurnID
	for i, prompt := range prompts {
		payload, err := json.Marshal(prompt)
		must(err)
		must(store.AppendInput(ctx, session.ID(id), inbox.Input{ID: inbox.ID(fmt.Sprintf("prompt-%d", i)), Kind: inbox.InputExternal, Payload: payload}))
		turn := session.TurnID(fmt.Sprintf("turn-%d", i))
		must(store.AppendTurn(ctx, session.ID(id), session.Turn{ID: turn, PreviousTurnID: previous, Type: session.TurnRegular}))
		previous = turn
	}
}

// promptsIn lists the prompts of a session.
func promptsIn(t *testing.T, dir, id string) []string {
	t.Helper()
	tr, err := cockpit.LoadSession(dir, id)
	if err != nil {
		t.Fatal(err)
	}
	var texts []string
	for _, e := range tr.Entries {
		if e.Kind == cockpit.KindUser {
			texts = append(texts, e.Text)
		}
	}
	return texts
}

// A kou agent duplicated goes on from a copy of its whole conversation, a
// session of the copy's own.
func TestADuplicatedAgentGoesOnFromACopyOfItsSession(t *testing.T) {
	te := newEngine(t)
	dir := te.host.ws.SessionDir
	id, c := te.newCanvas(t, "Copies")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"agent","title":"Worker"}},
		{"op":"node.add","node":{"preset":"agent","title":"Fresh"}},
		{"op":"node.add","node":{"kind":"note","title":"Notes"}}
	]`)
	worker, fresh, note := res.IDs["0"], res.IDs["1"], res.IDs["2"]
	sWorker, sFresh := sessionOf(t, c, worker), sessionOf(t, c, fresh)
	agentHistory(t, dir, sWorker, "first task", "second task")
	count := func() int {
		t.Helper()
		sessions, err := cockpit.ListSessions(dir)
		if err != nil {
			t.Fatal(err)
		}
		return len(sessions)
	}

	res = te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Worker copy","branch":"`+worker+`"}}]`)
	copied := res.IDs["0"]
	sCopy := sessionOf(t, c, copied)
	if sCopy == sWorker {
		t.Fatal("the copy shares its original's session")
	}
	if got := promptsIn(t, dir, sCopy); !slices.Equal(got, []string{"first task", "second task"}) {
		t.Fatalf("the copy's history: %q", got)
	}
	// The copy is the node's own session: the session list leaves it to the
	// canvas, and it goes with the node.
	eventually(t, 5*time.Second, "the copy to name its node", func() bool {
		meta := cockpit.LoadMeta(dir, sCopy)
		return meta.Canvas == id && meta.Node == copied && !meta.Adopted
	})
	if n := c.node(copied); n == nil || n.Runtime.Adopted || n.Title != "Worker copy" {
		t.Fatalf("the copy: %+v", n)
	}
	if meta := cockpit.LoadMeta(dir, sWorker); meta.Canvas != id || meta.Node != worker {
		t.Fatalf("the original's session: %+v", meta)
	}
	if got := promptsIn(t, dir, sWorker); !slices.Equal(got, []string{"first task", "second task"}) {
		t.Fatalf("the original's history: %q", got)
	}

	// An agent that never ran is copied as a new agent, with a new session.
	res = te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","branch":"`+fresh+`"}}]`)
	if s := sessionOf(t, c, res.IDs["0"]); s == sFresh || s == sWorker || s == sCopy {
		t.Fatalf("the copy of an agent that never ran has the session %s", s)
	}

	// An agent at work is not copied, nor is what is no kou agent; an agent
	// does not copy sessions; and a batch that fails leaves no copy behind.
	before := count()
	ops := func(spec string) string { return `{"ops":[{"op":"node.add","node":` + spec + `}]}` }
	te.host.setRunning(sWorker, true)
	te.must(t, http.StatusConflict, "POST", "/api/w/w1/canvases/"+id+"/ops", ops(`{"kind":"agent","branch":"`+worker+`"}`), "", nil)
	te.host.setRunning(sWorker, false)
	te.must(t, http.StatusBadRequest, "POST", "/api/w/w1/canvases/"+id+"/ops", ops(`{"kind":"note","branch":"`+note+`"}`), "", nil)
	te.must(t, http.StatusBadRequest, "POST", "/api/w/w1/canvases/"+id+"/ops", ops(`{"kind":"agent","branch":"`+worker+`","session":"`+sWorker+`"}`), "", nil)
	te.must(t, http.StatusNotFound, "POST", "/api/w/w1/canvases/"+id+"/ops", ops(`{"kind":"agent","branch":"n_nothing"}`), "", nil)
	build := te.token(t, c, worker, ScopeBuild)
	te.must(t, http.StatusForbidden, "POST", "/api/canvas/ops", ops(`{"kind":"agent","branch":"`+worker+`"}`), build, nil)
	te.must(t, http.StatusBadRequest, "POST", "/api/w/w1/canvases/"+id+"/ops",
		`{"ops":[{"op":"node.add","node":{"kind":"agent","branch":"`+worker+`"}},{"op":"node.add","node":{"kind":"nothing"}}]}`, "", nil)
	if after := count(); after != before {
		t.Fatalf("%d sessions before the batches that failed, %d after", before, after)
	}

	// Removed, the copy takes its session with it; the original keeps its.
	te.ops(t, id, `[{"op":"node.remove","id":"`+copied+`","grace_s":0}]`)
	eventually(t, 5*time.Second, "the copy's session to go", func() bool { return gone(te, sCopy) })
	if gone(te, sWorker) {
		t.Fatal("the original's session went with its copy")
	}
}
