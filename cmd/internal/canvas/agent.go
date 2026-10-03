package canvas

import (
	"errors"
	"fmt"
	"io/fs"
	"strings"
	"time"
	"unicode/utf8"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

// A kou agent node is a session of the server's own agent, which the canvas
// makes for it: its runs are the server's, its prompts the messages the
// node is sent, through the session's queue, and its answer at the end of
// each run its output. The session's metadata names the canvas and the node,
// so that the session list can tell it apart, and the runner of its runs is
// given the canvas's variables (RunEnv) — and with them the canvas-agent
// plugin's tools.

// attach binds a node to what runs for it; start starts what is not
// running yet.
func (c *Canvas) attach(id string, start bool) {
	n := c.node(id)
	if n == nil {
		return
	}
	if n.Proposed {
		c.setStatus(id, StatePaused, "made by an agent: waiting for your approval")
		return
	}
	switch n.Kind {
	case KindTerminal:
		c.startTerminal(n, start, false)
	case KindAgent:
		c.startAgent(n, start)
	case KindSource:
		c.startSource(n)
	case KindNote:
		c.setStatus(id, StateIdle, "")
	}
}

type agentInstance struct {
	c       *Canvas
	id      string
	session string
}

// startAgent binds an agent node to its session, making the session when
// it has none.
func (c *Canvas) startAgent(n *Node, start bool) {
	session := n.Runtime.Session
	if session == "" {
		if !start {
			c.setStatus(n.ID, StateStopped, "no session")
			return
		}
		session = uuid.New().String()
		if err := cockpit.SetCanvasMeta(c.ws.SessionDir, session, n.Title, c.id, n.ID); err != nil {
			c.setStatus(n.ID, StateError, "cannot make its session: "+err.Error())
			return
		}
		c.setRuntime(n.ID, func(r *Runtime) { r.Session = session })
	}
	c.e.indexSession(c, n.ID, session)
	inst := &agentInstance{c: c, id: n.ID, session: session}
	busy := c.e.o.Host.RunActive(c.ws, session)
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[n.ID]
	if st == nil {
		return
	}
	st.inst = inst
	if busy {
		c.setStatusLocked(n.ID, StateBusy, "")
	} else {
		c.setStatusLocked(n.ID, StateIdle, "")
	}
}

func (a *agentInstance) deliver(m *Message) error {
	n := a.c.node(a.id)
	if n == nil {
		return errors.New("the node is gone")
	}
	text := m.Text
	if !n.Runtime.Briefed {
		text = a.c.brief(a.id) + "\n\n" + text
	}
	// The run that answers the prompt says so by its ID: the message is
	// known by it first, as the run may end before Enqueue returns.
	prompt := uuid.New().String()
	a.c.remember(a.id, prompt, m)
	err := a.c.e.o.Host.Enqueue(a.c.e.ctx, a.c.ws, a.session, prompt, text, strings.TrimSpace(n.configString("model")), m.Deliver == DeliverNow)
	if err != nil {
		a.c.forget(a.id, prompt)
		return err
	}
	if !n.Runtime.Briefed {
		a.c.setRuntime(a.id, func(r *Runtime) { r.Briefed = true })
	}
	return nil
}

func (a *agentInstance) read(what string, lines int) (string, error) {
	switch what {
	case "output", "answer", "":
		if answer := a.c.lastOutput(a.id); answer != "" || what == "" {
			if answer != "" {
				return answer, nil
			}
		}
		tr, err := cockpit.LoadSession(a.c.ws.SessionDir, a.session)
		if errors.Is(err, fs.ErrNotExist) {
			return "", nil
		}
		if err != nil {
			return "", err
		}
		return lastAnswer(tr.Entries), nil
	}
	if lines <= 0 {
		lines = 40
	}
	tr, err := cockpit.LoadSession(a.c.ws.SessionDir, a.session)
	if errors.Is(err, fs.ErrNotExist) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	var b strings.Builder
	entries := tr.Entries
	if len(entries) > lines {
		entries = entries[len(entries)-lines:]
	}
	for _, e := range entries {
		item := compactEntry(e)
		switch item.Kind {
		case cockpit.KindUser:
			b.WriteString("› " + item.Text + "\n")
		case cockpit.KindAssistant:
			b.WriteString(item.Text + "\n")
		case cockpit.KindTool:
			if item.Tool != nil {
				fmt.Fprintf(&b, "▸ %s · %s · %s\n", item.Tool.Name, item.Tool.Summary, item.Tool.State)
			}
		case cockpit.KindError, cockpit.KindNotice:
			b.WriteString("! " + item.Text + "\n")
		}
	}
	return strings.TrimRight(b.String(), "\n"), nil
}

func (a *agentInstance) stop() {}

// FinalAnswer is the answer of a transcript's last run: what the node of
// its agent puts out when the run ends.
func FinalAnswer(entries []*cockpit.Entry) string { return lastAnswer(entries) }

// lastAnswer is a transcript's last answer: its last final answer, else
// the text the agent wrote after the last prompt.
func lastAnswer(entries []*cockpit.Entry) string {
	for i := len(entries) - 1; i >= 0; i-- {
		e := entries[i]
		if e.Kind == cockpit.KindUser {
			break
		}
		if e.Kind == cockpit.KindAssistant && e.Phase == "final_answer" && strings.TrimSpace(e.Text) != "" {
			return e.Text
		}
	}
	for i := len(entries) - 1; i >= 0; i-- {
		e := entries[i]
		if e.Kind == cockpit.KindUser {
			break
		}
		if e.Kind == cockpit.KindAssistant && strings.TrimSpace(e.Text) != "" {
			return e.Text
		}
	}
	return ""
}

// FeedEntry is an entry of an agent's transcript as its node shows it.
type FeedEntry struct {
	ID    string    `json:"id"`
	Kind  string    `json:"kind"`
	Phase string    `json:"phase,omitzero"`
	At    time.Time `json:"at,omitzero"`
	Text  string    `json:"text,omitzero"`
	Tool  *FeedTool `json:"tool,omitzero"`
	// Truncated says the text is only its start.
	Truncated bool `json:"truncated,omitzero"`
}

// FeedTool is a tool call, in brief.
type FeedTool struct {
	Name     string    `json:"name"`
	State    string    `json:"state"`
	Summary  string    `json:"summary,omitzero"`
	ExitCode *int      `json:"exit_code,omitzero"`
	Started  time.Time `json:"started,omitzero"`
	Finished time.Time `json:"finished,omitzero"`
}

// feedText bounds the text of a feed's entry.
const feedText = 2 << 10

// briefStart is how a brief starts. A node's first prompt is its brief,
// a blank line, then the message: its feed shows the message.
const briefStart = "You are node «"

// withoutBrief is a prompt without the brief that came first in it.
func withoutBrief(text string) string {
	if strings.HasPrefix(text, briefStart) {
		if _, rest, ok := strings.Cut(text, "\n\n"); ok {
			return rest
		}
	}
	return text
}

func compactEntry(e *cockpit.Entry) FeedEntry {
	item := FeedEntry{ID: e.ID, Kind: e.Kind, Phase: e.Phase, At: e.At, Text: e.Text}
	if e.Kind == cockpit.KindUser {
		item.Text = withoutBrief(item.Text)
	}
	if len(item.Text) > feedText {
		item.Text, item.Truncated = cut(item.Text, feedText)+"…", true
	}
	if e.Tool != nil {
		summary := e.Tool.Input
		if utf8.RuneCountInString(summary) > 200 {
			summary = string([]rune(summary)[:199]) + "…"
		}
		item.Tool = &FeedTool{
			Name: e.Tool.Name, State: e.Tool.State, Summary: summary, ExitCode: e.Tool.ExitCode,
			Started: e.Tool.Started, Finished: e.Tool.Finished,
		}
	}
	return item
}

// agentEnv is what the runner of a node's agent adds to its environment.
func (c *Canvas) agentEnv(id string) []string {
	n := c.node(id)
	if n == nil {
		return nil
	}
	// An agent without access has no token: the canvas-agent plugin, whose
	// tools would all be refused, is off for it.
	scope := n.Access
	if !allows(scope, ScopeObserve) {
		scope = ""
	}
	env := c.e.env(c, id, scope, n.Runtime.Epoch)
	if strings.TrimSpace(n.configString("sandbox")) == runconfig.SandboxWorktree {
		env = append(env, runconfig.SandboxEnvironment+"="+runconfig.SandboxWorktree)
	}
	return env
}

func (c *Canvas) agentRunStarted(id string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.setStatusLocked(id, StateBusy, "")
}

func (c *Canvas) agentActivity(id, activity string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.setActivityLocked(id, activity)
}

func (c *Canvas) agentEntry(id string, entry *cockpit.Entry) {
	item := compactEntry(entry)
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.states[id] != nil {
		c.hub.publish(map[string]any{"type": "agent", "node": id, "entry": item})
	}
}

// agentRunFinished takes in a run of a node's agent that ended: its answer
// answers the prompts it ran — those of messages from the canvas go where
// the messages came from, the user's stay with the user.
func (c *Canvas) agentRunFinished(id, answer, outcome string, prompts []string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	n, st := c.doc.node(id), c.states[id]
	if n == nil || st == nil {
		return
	}
	reason := ""
	switch outcome {
	case "failed":
		c.setStatusLocked(id, StateError, "the run failed")
		reason = "«" + n.Title + "»'s run failed"
	case "stopped":
		c.setStatusLocked(id, StateIdle, "stopped")
		reason = "«" + n.Title + "»'s run was stopped"
	default:
		c.setStatusLocked(id, StateIdle, "")
	}
	c.answerLocked(id, Output{Port: "out", Text: answer}, st.takePrompts(prompts), reason)
}

// bury ends a node removed for good: its grace is over.
func (c *Canvas) bury(id string, g *grave) {
	c.mu.Lock()
	if c.graves[id] != g {
		c.mu.Unlock()
		return
	}
	delete(c.graves, id)
	c.mu.Unlock()
	if g.state != nil && g.state.inst != nil {
		g.state.inst.stop()
	}
	if session := g.node.Runtime.Session; session != "" {
		c.e.unindexSession(c, session)
		c.dropSession(session, !g.keep.Session)
	}
	if g.node.Kind == KindSource {
		_ = removeAll(stateDir(c.ws.Path, c.id, id))
	}
}

// setLive starts or stops a canvas's sources as it goes live or pauses.
func (c *Canvas) setLive(live bool) {
	c.mu.Lock()
	var sources []string
	var stops []instance
	for _, n := range c.doc.Nodes {
		if n.Kind != KindSource {
			continue
		}
		sources = append(sources, n.ID)
		if st := c.states[n.ID]; st != nil && st.inst != nil && !live {
			stops = append(stops, st.inst)
			st.inst = nil
			c.setStatusLocked(n.ID, StateStopped, "the canvas is paused")
		}
	}
	c.mu.Unlock()
	for _, inst := range stops {
		inst.stop()
	}
	if live {
		for _, id := range sources {
			c.attach(id, true)
		}
	}
	c.kickDispatch()
}

// stopNode stops what a node does now: its agent's run, or its shell's
// command.
func (c *Canvas) stopNode(id string) error {
	n := c.node(id)
	if n == nil {
		return errNotFound("no such node")
	}
	switch n.Kind {
	case KindAgent:
		if n.Runtime.Session == "" || !c.e.o.Host.StopRun(c.ws, n.Runtime.Session) {
			return errConflict("«" + n.Title + "» is not running")
		}
		return nil
	case KindTerminal:
		c.mu.Lock()
		t, _ := c.states[id].inst.(*terminalInstance)
		c.mu.Unlock()
		if t == nil {
			return errConflict("«" + n.Title + "» has no shell running")
		}
		return t.keys([]string{"C-c"})
	}
	return errInvalid("«" + n.Title + "» runs nothing to stop")
}

// restart starts a node's program again: a shell that ended, a harness
// that exited (resume takes its session up again), a source.
func (c *Canvas) restart(id string, resume bool) error {
	n := c.node(id)
	if n == nil {
		return errNotFound("no such node")
	}
	if n.Proposed {
		return errConflict("«" + n.Title + "» waits for your approval")
	}
	switch n.Kind {
	case KindTerminal:
		c.mu.Lock()
		var t *terminalInstance
		if st := c.states[id]; st != nil {
			t, _ = st.inst.(*terminalInstance)
		}
		c.mu.Unlock()
		if t != nil && t.relaunch(n, resume) {
			return nil
		}
		go c.startTerminal(n, true, resume)
		return nil
	case KindSource:
		c.mu.Lock()
		var old instance
		if st := c.states[id]; st != nil {
			old, st.inst = st.inst, nil
		}
		c.mu.Unlock()
		if old != nil {
			old.stop()
		}
		c.attach(id, true)
		return nil
	case KindAgent:
		c.attach(id, true)
		return nil
	}
	return errInvalid("«" + n.Title + "» runs nothing")
}

// rotate voids a node's tokens: its next programs are given new ones.
func (c *Canvas) rotate(id string) error {
	n := c.node(id)
	if n == nil {
		return errNotFound("no such node")
	}
	c.setRuntime(id, func(r *Runtime) { r.Epoch++ })
	return nil
}

// errNodeGone is what a request for a node removed hears.
var errNodeGone = errors.New("the node is gone")
