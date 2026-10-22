package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

// setRunning says whether a session's agent runs now.
func (h *fakeHost) setRunning(session string, running bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.active[session] = running
}

// sentBy is the last message a node sent, straight or along its edges.
func sentBy(t *testing.T, c *Canvas, node string) Message {
	t.Helper()
	var found *Message
	eventually(t, 5*time.Second, "a message from "+node, func() bool {
		c.mu.Lock()
		defer c.mu.Unlock()
		for i := len(c.messages) - 1; i >= 0; i-- {
			if m := c.messages[i]; m.Actor == node || m.From.Node == node {
				copied := *m
				found = &copied
				return true
			}
		}
		return false
	})
	return *found
}

func TestAnEdgeDeliversAsWhatItLeadsToTakesIt(t *testing.T) {
	agent, term := &Node{Kind: KindAgent}, &Node{Kind: KindTerminal}
	for _, test := range []struct {
		deliver string
		to      *Node
		want    string
	}{
		{"", agent, DeliverNow},
		{"", term, DeliverQueue},
		{"", nil, DeliverQueue},
		{DeliverQueue, agent, DeliverQueue},
		{DeliverNow, term, DeliverNow},
	} {
		if got := (&Edge{Deliver: test.deliver}).deliverTo(test.to); got != test.want {
			t.Errorf("an edge that says %q, to %+v: %q, want %q", test.deliver, test.to, got, test.want)
		}
	}
}

func TestAKouAgentReadsWhatComesToItBetweenItsToolCalls(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Now")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"kind":"agent","title":"Coder"}},
		{"op":"node.add","node":{"kind":"agent","title":"Patient"}},
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$2"},"deliver":"queue"}}
	]`)
	source, coder, patient, lead := res.IDs["0"], res.IDs["1"], res.IDs["2"], res.IDs["3"]
	sc, sp := sessionOf(t, c, coder), sessionOf(t, c, patient)
	fire := func(text string) {
		t.Helper()
		te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"`+text+`"}`, "", nil)
	}
	forced := func(session string, i int) bool {
		t.Helper()
		eventually(t, 5*time.Second, "the prompt", func() bool { return len(te.host.promptsOf(session)) > i })
		return te.host.promptsOf(session)[i].force
	}

	// The view says how each edge delivers: now to a kou agent, unless the
	// edge says queue.
	for _, e := range c.view("", ScopeAdmin, false).Edges {
		want := DeliverNow
		if e.To == patient+":in" {
			want = DeliverQueue
		}
		if e.Deliver != want {
			t.Fatalf("the edge to %s delivers %q, want %q", e.To, e.Deliver, want)
		}
	}

	// Idle, they take it as their queues have it: a paused queue holds it.
	fire("one")
	if forced(sc, 0) || forced(sp, 0) {
		t.Fatal("a message to an idle agent went past its queue")
	}

	// At work, the coder reads the next between its tool calls; the edge
	// that says queue has the other wait for its run's end.
	te.host.setRunning(sc, true)
	te.host.setRunning(sp, true)
	fire("two")
	if !forced(sc, 1) {
		t.Fatal("a busy agent was not given the message at once")
	}
	if forced(sp, 1) {
		t.Fatal("an edge that says queue gave the message at once")
	}

	// A node's CanvasSend goes at once too, unless it says otherwise; the
	// user's waits, unless the user says now.
	talk := te.token(t, c, lead, ScopeTalk)
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+coder+"/send", `{"text":"three"}`, talk, nil)
	if !forced(sc, 2) {
		t.Fatal("a node's send to a busy agent waited")
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+coder+"/send", `{"text":"four","when":"queue"}`, talk, nil)
	if forced(sc, 3) {
		t.Fatal("a send that says queue went at once")
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+coder+"/send", `{"text":"five"}`, "", nil)
	if forced(sc, 4) {
		t.Fatal("the user's send went at once")
	}
	// Said, now goes at once even to an idle agent: past its paused queue.
	te.host.setRunning(sc, false)
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+coder+"/send", `{"text":"six","when":"now"}`, talk, nil)
	if !forced(sc, 5) {
		t.Fatal("a send that says now waited")
	}
}

func TestAForemanAnswersNodesWithCanvasSendAlone(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Crew")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"foreman","title":"Boss"}},
		{"op":"node.add","node":{"kind":"agent","title":"Coder"}}
	]`)
	boss, coder := res.IDs["0"], res.IDs["1"]
	sb, sc := sessionOf(t, c, boss), sessionOf(t, c, coder)
	talk := te.token(t, c, coder, ScopeTalk)

	// What the coder reports, the foreman answers to the user: nothing goes
	// back to the coder.
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+boss+"/send", `{"text":"done with 7"}`, talk, nil)
	eventually(t, 5*time.Second, "the foreman told", func() bool { return len(te.host.promptsOf(sb)) == 1 })
	te.answer(t, sb, "The coder is done with issue 7.")
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sc); len(prompts) != 0 {
		t.Fatalf("the foreman's answer to the user went to the coder: %+v", prompts)
	}

	// A send that waits has its answer all the same.
	asked := te.sendAsync(boss, `{"text":"may I merge?","wait":true}`, talk)
	eventually(t, 5*time.Second, "the foreman asked", func() bool { return len(te.host.promptsOf(sb)) == 2 })
	te.answer(t, sb, "yes")
	if got := receive(t, asked); !got.Answered || got.Answer != "yes" {
		t.Fatalf("the send answered %+v", got)
	}

	// Its config says reply auto: its answers go back again.
	te.ops(t, id, `[{"op":"node.update","id":"`+boss+`","set":{"config":{"reply":"auto"}}}]`)
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+boss+"/send", `{"text":"done with 8"}`, talk, nil)
	eventually(t, 5*time.Second, "the foreman told again", func() bool { return len(te.host.promptsOf(sb)) == 3 })
	te.answer(t, sb, "noted")
	eventually(t, 5*time.Second, "the reply", func() bool { return len(te.host.promptsOf(sc)) == 1 })

	// And an agent whose config says reply off answers none.
	te.ops(t, id, `[{"op":"node.update","id":"`+boss+`","set":{"config":{"reply":"off"}}}]`)
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+boss+"/send", `{"text":"done with 9"}`, talk, nil)
	eventually(t, 5*time.Second, "the foreman told once more", func() bool { return len(te.host.promptsOf(sb)) == 4 })
	te.answer(t, sb, "fine")
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sc); len(prompts) != 1 {
		t.Fatalf("an answer went back with reply off: %+v", prompts)
	}
}

func TestAnAgentsSendStartsAChainOfItsOwnAndIsSeenWhenRead(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Chains")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"kind":"agent","title":"Triage"}},
		{"op":"node.add","node":{"kind":"agent","title":"Coder"}},
		{"op":"node.add","node":{"kind":"agent","title":"Reviewer"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$2"}}}
	]`)
	source, triage, coder, reviewer := res.IDs["0"], res.IDs["1"], res.IDs["2"], res.IDs["3"]
	st, sc, sr := sessionOf(t, c, triage), sessionOf(t, c, coder), sessionOf(t, c, reviewer)

	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 9"}`, "", nil)
	eventually(t, 5*time.Second, "the triage given the issue", func() bool { return len(te.host.promptsOf(st)) == 1 })
	te.answer(t, st, "for the coder")
	eventually(t, 5*time.Second, "the coder given it", func() bool { return len(te.host.promptsOf(sc)) == 1 })
	along := sentBy(t, c, triage)
	if along.Hops != 1 {
		t.Fatalf("along an edge, hop %d: %+v", along.Hops, along)
	}

	// The coder, at work on a message one hop on, asks the reviewer: its
	// own message, which counts no hops of what it works on.
	talk := te.token(t, c, coder, ScopeTalk)
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+reviewer+"/send", `{"text":"review this"}`, talk, nil)
	eventually(t, 5*time.Second, "the reviewer asked", func() bool { return len(te.host.promptsOf(sr)) == 1 })
	own := sentBy(t, c, coder)
	if own.Hops != 0 || own.Chain == "" || own.Chain == along.Chain {
		t.Fatalf("a node's send went on the chain it works on: %+v", own)
	}

	// Delivered is not read: the message is seen once the runner wrote
	// its prompt down.
	if m, _ := c.message(own.ID); m.DeliveredAt.IsZero() || !m.SeenAt.IsZero() {
		t.Fatalf("delivered %v, seen %v", m.DeliveredAt, m.SeenAt)
	}
	prompt := te.host.promptsOf(sr)[0].id
	te.RunEntry("w1", sr, &cockpit.Entry{ID: "input:" + prompt, Kind: cockpit.KindUser, State: cockpit.Pending})
	if m, _ := c.message(own.ID); !m.SeenAt.IsZero() {
		t.Fatal("seen while it was pending")
	}
	te.RunEntry("w1", sr, &cockpit.Entry{ID: "input:" + prompt, Kind: cockpit.KindUser})
	if m, _ := c.message(own.ID); m.SeenAt.Before(m.DeliveredAt) {
		t.Fatalf("delivered %v, seen %v", m.DeliveredAt, m.SeenAt)
	}
}

func TestAnAgentRunsWithTheEffortItsConfigSays(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Effort")
	coder := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Coder","config":{"effort":"high"}}}]`).IDs["0"]
	sc := sessionOf(t, c, coder)
	send := func(text string) {
		t.Helper()
		before := len(te.host.promptsOf(sc))
		te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+coder+"/send", `{"text":"`+text+`"}`, "", nil)
		eventually(t, 5*time.Second, "the coder given "+text, func() bool { return len(te.host.promptsOf(sc)) > before })
	}
	effort := func() string { return cockpit.LoadMeta(te.host.ws.SessionDir, sc).Effort }

	send("think")
	if got := effort(); got != "high" {
		t.Fatalf("its session's effort %q", got)
	}
	// What the user chooses in its composer holds, until its config
	// changes.
	if err := cockpit.SetSessionEffort(te.host.ws.SessionDir, sc, "low"); err != nil {
		t.Fatal(err)
	}
	send("again")
	if got := effort(); got != "low" {
		t.Fatalf("the user's effort was overridden: %q", got)
	}
	te.ops(t, id, `[{"op":"node.update","id":"`+coder+`","set":{"config":{"effort":"max"}}}]`)
	send("harder")
	if got := effort(); got != "max" {
		t.Fatalf("its session's effort %q after the config's change", got)
	}
}

func TestAnAgentWorksInTheFolderItsConfigSays(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Folders")
	app := filepath.Join(te.host.ws.Path, "app")
	if err := os.MkdirAll(app, 0o755); err != nil {
		t.Fatal(err)
	}
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Here","config":{"cwd":"app"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Elsewhere","config":{"cwd":"`+app+`"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Home"}}
	]`)
	here, elsewhere, home := sessionOf(t, c, res.IDs["0"]), sessionOf(t, c, res.IDs["1"]), sessionOf(t, c, res.IDs["2"])
	want := runconfig.WorkdirEnvironment + "=" + app
	for _, session := range []string{here, elsewhere} {
		if env := te.RunEnv("w1", session); !slices.Contains(env, want) {
			t.Fatalf("its runs are not told the folder: %v", env)
		}
	}
	if env := te.RunEnv("w1", home); slices.ContainsFunc(env, func(v string) bool { return strings.HasPrefix(v, runconfig.WorkdirEnvironment+"=") }) {
		t.Fatalf("a node without a folder was given one: %v", env)
	}
}
