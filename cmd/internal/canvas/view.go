package canvas

import (
	"context"
	"slices"
	"strings"
	"time"
)

// What an agent on a canvas sees of it (kou-canvas view, CanvasView): the
// canvas, itself, the nodes with their places and states, the edges, and
// free places beside itself to put new nodes.

// View is the canvas as an agent sees it.
type View struct {
	Canvas ViewCanvas `json:"canvas"`
	Me     *ViewNode  `json:"me,omitzero"`
	Scope  string     `json:"scope,omitzero"`
	Nodes  []ViewNode `json:"nodes"`
	Edges  []ViewEdge `json:"edges"`
	// Free are places beside the agent's node where a new node fits.
	Free []FreePlace `json:"free,omitzero"`
}

// ViewCanvas is the canvas's own.
type ViewCanvas struct {
	ID    string `json:"id"`
	Title string `json:"title"`
	Live  bool   `json:"live"`
	Rev   int64  `json:"rev"`
}

// ViewNode is a node as an agent sees it.
type ViewNode struct {
	ID        string   `json:"id"`
	Kind      string   `json:"kind"`
	Preset    string   `json:"preset,omitzero"`
	Plugin    string   `json:"plugin,omitzero"`
	Title     string   `json:"title"`
	Status    string   `json:"status"`
	Detail    string   `json:"detail,omitzero"`
	X         int      `json:"x"`
	Y         int      `json:"y"`
	W         int      `json:"w"`
	H         int      `json:"h"`
	Inputs    []string `json:"inputs,omitzero"`
	Outputs   []string `json:"outputs,omitzero"`
	Worktree  string   `json:"worktree,omitzero"`
	Branch    string   `json:"branch,omitzero"`
	Program   string   `json:"program,omitzero"`
	Access    string   `json:"access,omitzero"`
	CreatedBy string   `json:"created_by,omitzero"`
	Proposed  bool     `json:"proposed,omitzero"`
	Pending   int      `json:"pending,omitzero"`
	Session   string   `json:"session,omitzero"`
	Text      string   `json:"text,omitzero"` // a note's
}

// ViewEdge is an edge as an agent sees it.
type ViewEdge struct {
	ID       string `json:"id"`
	From     string `json:"from"`
	To       string `json:"to"`
	Mode     string `json:"mode"`
	Deliver  string `json:"deliver"`
	Template string `json:"template,omitzero"`
}

// FreePlace is a free place beside a node.
type FreePlace struct {
	Side string `json:"side"`
	X    int    `json:"x"`
	Y    int    `json:"y"`
}

// view builds what an agent of a node sees; full adds the edges'
// templates and the notes' texts.
func (c *Canvas) view(me string, scope string, full bool) View {
	catalog := c.e.catalog(c.ws)
	c.mu.Lock()
	defer c.mu.Unlock()
	v := View{Canvas: ViewCanvas{c.id, c.doc.Title, c.doc.Live, c.doc.Rev}, Scope: scope, Nodes: []ViewNode{}, Edges: []ViewEdge{}}
	pending := map[string]int{}
	for _, m := range c.pending {
		pending[m.To.Node]++
	}
	for _, n := range c.doc.Nodes {
		item := ViewNode{
			ID: n.ID, Kind: n.Kind, Preset: n.Preset, Plugin: n.Plugin, Title: n.Title, X: n.X, Y: n.Y, W: n.W, H: n.H,
			Worktree: n.Runtime.Worktree, Branch: n.Runtime.Branch, Access: n.Access, CreatedBy: n.CreatedBy,
			Proposed: n.Proposed, Pending: pending[n.ID], Session: n.Runtime.Session,
		}
		item.Inputs, item.Outputs = portsOf(n, catalog)
		if st := c.states[n.ID]; st != nil {
			item.Status, item.Detail = st.status.State, st.status.Detail
			if st.status.Activity != "" {
				item.Detail = st.status.Activity
			}
			if st.terminal != nil {
				item.Program = st.terminal.Info().Running
			}
		}
		if n.Kind == KindNote && full {
			item.Text = n.configString("text")
		}
		v.Nodes = append(v.Nodes, item)
		if n.ID == me {
			copied := item
			v.Me = &copied
		}
	}
	for _, e := range c.doc.Edges {
		item := ViewEdge{ID: e.ID, From: e.From.String(), To: e.To.String(), Mode: e.mode(), Deliver: e.deliver()}
		if full {
			item.Template = e.Template
		}
		v.Edges = append(v.Edges, item)
	}
	if n := c.doc.node(me); n != nil {
		taken := make([]Rect, 0, len(c.doc.Nodes))
		for _, other := range c.doc.Nodes {
			taken = append(taken, other.rect())
		}
		w, h := defaultSize(KindTerminal, "shell")
		anchor := n.rect()
		for _, side := range []string{"right", "below"} {
			if r, ok := placeBeside(taken, anchor, side, w, h); ok {
				v.Free = append(v.Free, FreePlace{Side: side, X: r.X, Y: r.Y})
			}
		}
	}
	return v
}

// read reads what a node shows.
func (c *Canvas) read(id, what string, lines int) (string, error) {
	c.mu.Lock()
	n, st := c.doc.node(id), c.states[id]
	var inst instance
	if st != nil {
		inst = st.inst
	}
	var note string
	if n != nil && n.Kind == KindNote {
		note = n.configString("text")
	}
	c.mu.Unlock()
	switch {
	case n == nil:
		return "", errNotFound("no such node")
	case n.Kind == KindNote:
		return note, nil
	case inst == nil:
		if what == "output" || what == "answer" {
			return c.lastOutput(id), nil
		}
		return "", errConflict("«" + n.Title + "» is not running")
	}
	switch what {
	case "", "screen", "tail", "output", "answer":
	default:
		return "", errInvalid("what is screen, tail, output or answer")
	}
	if what == "" {
		what = "tail"
	}
	return inst.read(what, lines)
}

// WaitResult is how a wait for a node ended.
type WaitResult struct {
	Node     string `json:"node"`
	Status   Status `json:"status"`
	Outputs  int    `json:"outputs"`
	TimedOut bool   `json:"timed_out,omitzero"`
}

// maxWait bounds a wait.
const maxWait = 30 * time.Minute

// wait waits for a node to be idle, to give an output (after outputs it
// gave already: after < 0 is the outputs it has now), or to end.
func (c *Canvas) wait(ctx context.Context, id, until string, after int, timeout time.Duration) (WaitResult, error) {
	switch until {
	case "idle", "output", "exit":
	default:
		return WaitResult{}, errInvalid("until is idle, output or exit")
	}
	timeout = min(max(timeout, time.Second), maxWait)
	deadline := time.NewTimer(timeout)
	defer deadline.Stop()
	for {
		c.mu.Lock()
		st := c.states[id]
		if st == nil {
			c.mu.Unlock()
			return WaitResult{}, errNotFound("no such node")
		}
		if after < 0 {
			after = st.outputs
		}
		result := WaitResult{Node: id, Status: st.status, Outputs: st.outputs}
		done := false
		switch until {
		case "idle":
			switch st.status.State {
			case StateIdle, StateWaiting, StateExited, StateStopped, StateError:
				done = !slices.ContainsFunc(c.pending, func(m *Message) bool {
					return m.To.Node == id && (m.State == MessagePending || m.State == messageDelivering)
				})
			}
		case "output":
			done = st.outputs > after
		case "exit":
			done = st.status.State == StateExited || st.status.State == StateStopped
		}
		changed := st.changed
		c.mu.Unlock()
		if done {
			return result, nil
		}
		select {
		case <-changed:
		case <-deadline.C:
			result.TimedOut = true
			return result, nil
		case <-ctx.Done():
			return result, ctx.Err()
		}
	}
}

// describe is a node's line in the text view kou-canvas prints.
func describe(n ViewNode) string {
	return strings.TrimSpace(n.ID + " " + n.Title)
}
