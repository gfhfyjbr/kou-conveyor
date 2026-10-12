package canvas

import (
	"context"
	"fmt"
	"reflect"
	"slices"
	"strconv"
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

// WaitNode is a node a wait for several watched, as the wait ended.
type WaitNode struct {
	Node    string `json:"node"`
	Title   string `json:"title"`
	Status  Status `json:"status"`
	Outputs int    `json:"outputs"`
	// Gone says that the node was removed.
	Gone bool `json:"gone,omitzero"`
}

// WaitAnyResult is how a wait for several nodes ended: Fired is the node
// whose event ended it (none when it timed out, or when they were all idle
// already), Next the cursor to wait on from, as after.
type WaitAnyResult struct {
	Fired    string     `json:"fired,omitzero"`
	Until    string     `json:"until"`
	Nodes    []WaitNode `json:"nodes"`
	Next     string     `json:"next"`
	TimedOut bool       `json:"timed_out,omitzero"`
}

// maxWaitNodes bounds the nodes a wait watches.
const maxWaitNodes = 64

// waitAny waits for something to happen to any of several nodes:
//
//	answer (output) one of them gives an output past the cursor
//	idle            all of them are idle — the last to stop working fires
//	exit            one of them exits, or is stopped or removed
//	any             one of them gives an output, changes its state, or goes
//
// after is the cursor a wait gave as next ("node:outputs,…"); the outputs of
// a node it does not name count from now.
func (c *Canvas) waitAny(ctx context.Context, ids []string, until, after string, timeout time.Duration) (WaitAnyResult, error) {
	switch until {
	case "", "answer":
		until = "answer"
	case "output", "idle", "exit", "any":
	default:
		return WaitAnyResult{}, errInvalid("until is answer, idle, exit or any")
	}
	if len(ids) == 0 {
		return WaitAnyResult{}, errInvalid("name the nodes to wait for")
	}
	if len(ids) > maxWaitNodes {
		return WaitAnyResult{}, errInvalid(fmt.Sprintf("a wait watches %d nodes at most", maxWaitNodes))
	}
	cursor := parseCursor(after)
	timeout = min(max(timeout, time.Second), maxWait)
	deadline := time.NewTimer(timeout)
	defer deadline.Stop()
	// What the nodes were when the wait began, to tell a change, and which
	// were at work the round before, to tell the last to stop.
	start := map[string]string{}
	wasBusy := map[string]bool{}
	first := true
	for {
		c.mu.Lock()
		result := WaitAnyResult{Until: until, Nodes: make([]WaitNode, 0, len(ids))}
		cases := make([]reflect.SelectCase, 0, len(ids)+2)
		fired := ""
		busy := map[string]bool{}
		next := make([]string, 0, len(ids))
		for _, id := range ids {
			st := c.states[id]
			n := c.doc.node(id)
			if st == nil || n == nil {
				if first {
					c.mu.Unlock()
					return WaitAnyResult{}, errNotFound("no node " + id)
				}
				result.Nodes = append(result.Nodes, WaitNode{Node: id, Gone: true})
				if fired == "" && (until == "exit" || until == "any") {
					fired = id
				}
				continue
			}
			seen, ok := cursor[id]
			if !ok {
				seen = st.outputs
				cursor[id] = seen
			}
			result.Nodes = append(result.Nodes, WaitNode{Node: id, Title: n.Title, Status: st.status, Outputs: st.outputs})
			next = append(next, fmt.Sprintf("%s:%d", id, st.outputs))
			if first {
				start[id] = st.status.State
			}
			stopped := st.status.State == StateExited || st.status.State == StateStopped
			if !c.idleLocked(id, st) {
				busy[id] = true
			}
			if fired != "" {
				continue
			}
			switch until {
			case "answer", "output":
				if st.outputs > seen {
					fired = id
				}
			case "exit":
				if stopped {
					fired = id
				}
			case "any":
				if st.outputs > seen || st.status.State != start[id] {
					fired = id
				}
			}
			cases = append(cases, reflect.SelectCase{Dir: reflect.SelectRecv, Chan: reflect.ValueOf(st.changed)})
		}
		c.mu.Unlock()
		result.Next = strings.Join(next, ",")
		switch {
		case until == "idle" && len(busy) == 0:
			// The last of them to stop working is what ended the wait.
			for i := len(ids) - 1; i >= 0; i-- {
				if wasBusy[ids[i]] {
					result.Fired = ids[i]
					break
				}
			}
			return result, nil
		case until != "idle" && fired != "":
			result.Fired = fired
			return result, nil
		case len(cases) == 0:
			// They are all gone: nothing is left to wait for.
			return result, nil
		}
		wasBusy, first = busy, false
		cases = append(cases,
			reflect.SelectCase{Dir: reflect.SelectRecv, Chan: reflect.ValueOf(deadline.C)},
			reflect.SelectCase{Dir: reflect.SelectRecv, Chan: reflect.ValueOf(ctx.Done())})
		chosen, _, _ := reflect.Select(cases)
		switch chosen {
		case len(cases) - 2:
			result.TimedOut = true
			return result, nil
		case len(cases) - 1:
			return result, ctx.Err()
		}
	}
}

// idleLocked says whether a node is idle: not at work, and no message on
// its way to it.
func (c *Canvas) idleLocked(id string, st *nodeState) bool {
	switch st.status.State {
	case StateIdle, StateWaiting, StateExited, StateStopped, StateError:
		return !slices.ContainsFunc(c.pending, func(m *Message) bool {
			return m.To.Node == id && (m.State == MessagePending || m.State == messageDelivering)
		})
	}
	return false
}

// parseCursor reads a wait's cursor: "node:outputs,…".
func parseCursor(after string) map[string]int {
	cursor := map[string]int{}
	for _, part := range strings.Split(after, ",") {
		id, count, ok := strings.Cut(strings.TrimSpace(part), ":")
		if !ok {
			continue
		}
		if n, err := strconv.Atoi(count); err == nil && n >= 0 {
			cursor[id] = n
		}
	}
	return cursor
}

// children are the nodes a node looks after: those its agent made, and
// those wired from it.
func (c *Canvas) children(id string) []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	var ids []string
	seen := map[string]bool{id: true}
	for _, n := range c.doc.Nodes {
		if n.CreatedBy == id && !seen[n.ID] {
			seen[n.ID] = true
			ids = append(ids, n.ID)
		}
	}
	for _, e := range c.doc.Edges {
		if e.From.Node == id && !seen[e.To.Node] && c.doc.node(e.To.Node) != nil {
			seen[e.To.Node] = true
			ids = append(ids, e.To.Node)
		}
	}
	return ids
}

// describe is a node's line in the text view kou-canvas prints.
func describe(n ViewNode) string {
	return strings.TrimSpace(n.ID + " " + n.Title)
}
