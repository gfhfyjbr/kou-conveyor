package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

// States of a node: what its program does.
const (
	StateStarting = "starting"
	StateIdle     = "idle"
	StateBusy     = "busy"
	StateWaiting  = "waiting" // for the user: a question, a permission
	StatePaused   = "paused"  // made by an agent, waiting for the user's approval
	StateError    = "error"
	StateExited   = "exited"  // its program ended
	StateStopped  = "stopped" // nothing runs for it
)

// Status is what a node's program does, and since when.
type Status struct {
	State  string    `json:"state"`
	Detail string    `json:"detail,omitzero"`
	Since  time.Time `json:"since"`
	// Activity is what an agent does now: Thinking, Running go test…
	Activity string `json:"activity,omitzero"`
	// Agent is the agent a terminal node runs: its harness, or one found
	// running in its shell.
	Agent *Agent `json:"agent,omitzero"`
	// Quiet says that a node at work has shown nothing for a while: a
	// command that waits, a server that serves.
	Quiet bool `json:"quiet,omitzero"`
}

// saveDelay is how long changes gather before they are written.
const saveDelay = 250 * time.Millisecond

// recentMessages is how many messages a canvas keeps for its pages.
const recentMessages = 500

// Canvas is a canvas loaded: its document, and what runs for its nodes.
type Canvas struct {
	e   *Engine
	ws  Workspace
	id  string
	dir string

	saving sync.Mutex // one write of the files at a time

	mu                sync.Mutex
	doc               *Doc
	readOnly          bool
	hub               *hub
	states            map[string]*nodeState
	graves            map[string]*grave
	messages          []*Message // the latest, the oldest first
	pending           []*Message // those that wait for delivery or approval, in order
	journal           []*Message // to be added to the journal
	rates             map[string][]time.Time
	failures          map[string]int
	transcriptCursors map[string]int64
	spawns            []time.Time
	// waits are the sends of nodes that wait for the answer to their
	// message, by the message's ID.
	waits map[string]*replyWait
	// dirty: the document is to be written; pendingDirty: the messages
	// that wait are.
	dirty, pendingDirty bool
	saveTimer           *time.Timer
	deleted, closed     bool
	kick                chan struct{}
	done                chan struct{}
}

// nodeState is what runs for a node, and what the engine knows of it.
type nodeState struct {
	inst   instance
	status Status
	// changed is closed and replaced whenever the status or the output
	// changes: what waits for the node waits on it.
	changed chan struct{}
	outputs int    // how many outputs it gave
	output  string // its last output's text
	// unanswered counts the runs that ended without an answer to what the
	// canvas asked — the model refused, the run was stopped or failed —
	// which end a wait for an answer as an output does; noAnswer says why
	// the last one did, until the node gives an output again.
	unanswered int
	noAnswer   string
	// cause is the message delivered to it last, whose chain its next
	// output goes on.
	cause *Message
	// prompts are the messages an agent's prompts came from, by the
	// prompt's ID, until the run that answers them ends; asked lists their
	// IDs, the oldest first.
	prompts map[string]*Message
	asked   []string
	// sent is the ID of the prompt each sender — the user, or a node — gave
	// a kou agent last, straight: the one its send with replace takes back
	// (replaced). Not kept: after a restart there is none to replace.
	sent map[string]string
	// active is when a kou agent's run last showed it at work: started,
	// wrote an entry or changed its activity.
	active time.Time
	// terminal is the node's shell, for a terminal node.
	terminal *terminal.Session
}

func newState(state, detail string) *nodeState {
	return &nodeState{status: Status{State: state, Detail: detail, Since: now()}, changed: make(chan struct{})}
}

func (st *nodeState) wake() {
	close(st.changed)
	st.changed = make(chan struct{})
}

// grave is a node removed, which can come back for a while: its shell
// lives on until then.
type grave struct {
	node  *Node
	edges []*Edge
	state *nodeState
	keep  Keep
	timer *time.Timer
}

func now() time.Time { return time.Now().UTC().Truncate(time.Millisecond) }

func newCanvas(e *Engine, ws Workspace, doc *Doc, readOnly bool) *Canvas {
	c := &Canvas{
		e: e, ws: ws, id: doc.ID, dir: dataDir(ws.Path, doc.ID),
		doc: doc, readOnly: readOnly, hub: newHub(e.instance),
		states: map[string]*nodeState{}, graves: map[string]*grave{},
		rates: map[string][]time.Time{}, failures: map[string]int{},
		transcriptCursors: loadTranscriptCursors(dataDir(ws.Path, doc.ID)), waits: map[string]*replyWait{},
		kick: make(chan struct{}, 1), done: make(chan struct{}),
	}
	c.messages = readJournal(c.dir, recentMessages)
	known := map[string]bool{}
	for _, m := range c.messages {
		known[m.ID] = true
	}
	for _, m := range loadPending(c.dir) {
		if doc.node(m.To.Node) == nil {
			continue
		}
		c.pending = append(c.pending, m)
		if !known[m.ID] {
			c.messages = append(c.messages, m)
		}
	}
	return c
}

// boot binds the nodes to what runs for them, and starts what should run.
func (c *Canvas) boot() {
	c.mu.Lock()
	nodes := append([]*Node(nil), c.doc.Nodes...)
	for _, n := range nodes {
		c.states[n.ID] = newState(StateStopped, "")
	}
	c.mu.Unlock()
	go c.dispatch()
	for _, n := range nodes {
		c.attach(n.ID, false)
	}
}

// node returns a copy of a node, or nil.
func (c *Canvas) node(id string) *Node {
	c.mu.Lock()
	defer c.mu.Unlock()
	if n := c.doc.node(id); n != nil {
		copied := *n
		return &copied
	}
	return nil
}

// setStatus sets a node's status.
func (c *Canvas) setStatus(id, state, detail string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.setStatusLocked(id, state, detail)
}

func (c *Canvas) setStatusLocked(id, state, detail string) {
	st := c.states[id]
	if st == nil || c.deleted {
		return
	}
	if st.status.State == state && st.status.Detail == detail {
		return
	}
	activity := ""
	if state == st.status.State {
		activity = st.status.Activity
	}
	st.status = Status{State: state, Detail: detail, Since: now(), Activity: activity, Agent: st.status.Agent}
	c.hub.publish(map[string]any{"type": "status", "node": id, "status": st.status})
	st.wake()
	if state == StateIdle || state == StateWaiting {
		c.kickDispatch()
	}
}

// setActivity says what a node's agent does now.
func (c *Canvas) setActivityLocked(id, activity string) {
	st := c.states[id]
	if st == nil || st.status.Activity == activity {
		return
	}
	st.status.Activity = activity
	c.hub.publish(map[string]any{"type": "status", "node": id, "status": st.status})
}

// setAgent says which agent a terminal node runs, nil for none. An Agent
// is never changed once set: another takes its place.
func (c *Canvas) setAgent(id string, agent *Agent) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.setAgentLocked(id, agent)
}

func (c *Canvas) setAgentLocked(id string, agent *Agent) {
	st := c.states[id]
	if st == nil || c.deleted || sameAgent(st.status.Agent, agent) {
		return
	}
	st.status.Agent = agent
	c.hub.publish(map[string]any{"type": "status", "node": id, "status": st.status})
}

// setQuiet says whether a node at work has shown nothing for a while.
func (c *Canvas) setQuiet(id string, quiet bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[id]
	if st == nil || c.deleted || st.status.Quiet == quiet || quiet && st.status.State != StateBusy {
		return
	}
	st.status.Quiet = quiet
	c.hub.publish(map[string]any{"type": "status", "node": id, "status": st.status})
}

// status returns a node's status.
func (c *Canvas) status(id string) Status {
	c.mu.Lock()
	defer c.mu.Unlock()
	if st := c.states[id]; st != nil {
		return st.status
	}
	return Status{State: StateStopped}
}

// setRuntime changes what runs for a node, and tells the pages.
func (c *Canvas) setRuntime(id string, change func(*Runtime)) {
	c.mu.Lock()
	defer c.mu.Unlock()
	n := c.doc.node(id)
	if n == nil {
		return
	}
	before := n.Runtime
	change(&n.Runtime)
	if n.Runtime != before {
		c.touchLocked()
		c.hub.publish(map[string]any{"type": "runtime", "node": id, "runtime": n.Runtime})
	}
}

// notice tells the pages of something that happened.
func (c *Canvas) noticeLocked(level, text, node, edge string) {
	event := map[string]any{"type": "notice", "level": level, "text": text, "at": now()}
	if node != "" {
		event["node"] = node
	}
	if edge != "" {
		event["edge"] = edge
	}
	c.hub.publish(event)
}

func (c *Canvas) notice(level, text, node, edge string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.noticeLocked(level, text, node, edge)
}

// touchLocked has the document written soon.
func (c *Canvas) touchLocked() {
	c.dirty = true
	c.scheduleSaveLocked()
}

func (c *Canvas) scheduleSaveLocked() {
	if c.saveTimer == nil && !c.deleted && !c.closed {
		c.saveTimer = time.AfterFunc(saveDelay, c.save)
	}
}

// save writes what changed: the document, the messages that wait, the
// journal. A canvas closed was written as the server stopped, for the
// last time.
func (c *Canvas) save() {
	c.saving.Lock()
	defer c.saving.Unlock()
	c.mu.Lock()
	c.saveTimer = nil
	if c.deleted || c.readOnly || c.closed {
		c.mu.Unlock()
		return
	}
	var doc []byte
	var err error
	if c.dirty {
		c.dirty = false
		doc, err = json.Marshal(c.doc, jsontext.WithIndent("  "))
	}
	var pending []*Message
	pendingDirty := c.pendingDirty
	if pendingDirty {
		c.pendingDirty = false
		for _, m := range c.pending {
			copied := *m
			pending = append(pending, &copied)
		}
	}
	journal := c.journal
	c.journal = nil
	c.mu.Unlock()
	if err == nil && doc != nil {
		err = writeAtomic(docPath(c.ws.Path, c.id), append(doc, '\n'), 0o600)
	}
	if err != nil {
		c.e.o.Logf("canvas %s: save: %v", c.id, err)
	}
	if pendingDirty {
		if err := savePending(c.dir, pending); err != nil {
			c.e.o.Logf("canvas %s: save the messages that wait: %v", c.id, err)
		}
	}
	if err := appendJournal(c.dir, journal...); err != nil {
		c.e.o.Logf("canvas %s: journal: %v", c.id, err)
	}
}

// flush writes at once what waits to be written.
func (c *Canvas) flush() {
	c.mu.Lock()
	if c.saveTimer != nil {
		c.saveTimer.Stop()
	}
	c.mu.Unlock()
	c.save()
}

// shutdown flushes the canvas and stops what follows its nodes: the server
// stops.
func (c *Canvas) shutdown() {
	c.flush()
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return
	}
	c.closed = true
	close(c.done)
	var instances []instance
	for _, st := range c.states {
		if st.inst != nil {
			instances = append(instances, st.inst)
		}
	}
	for _, g := range c.graves {
		if g.state != nil && g.state.inst != nil {
			instances = append(instances, g.state.inst)
		}
	}
	c.mu.Unlock()
	for _, inst := range instances {
		inst.stop()
	}
}

// DeleteOptions say what deleting a canvas leaves.
type DeleteOptions struct {
	// KeepTerminals leaves the shells running, as the sidebar's; else
	// they end.
	KeepTerminals bool
	// KeepSessions leaves the agents' sessions, which the session list
	// shows again; else those that ran are deleted too.
	KeepSessions bool
}

// remove deletes the canvas: what runs for it stops, and its files go.
func (c *Canvas) remove(o DeleteOptions) error {
	c.mu.Lock()
	if c.deleted {
		c.mu.Unlock()
		return fs.ErrNotExist
	}
	c.deleted = true
	if c.saveTimer != nil {
		c.saveTimer.Stop()
	}
	type left struct {
		inst     instance
		terminal string
		session  string
		adopted  bool
	}
	var all []left
	for _, n := range c.doc.Nodes {
		st := c.states[n.ID]
		item := left{terminal: n.Runtime.Terminal, session: n.Runtime.Session, adopted: n.Runtime.Adopted}
		if st != nil {
			item.inst = st.inst
		}
		all = append(all, item)
	}
	for _, g := range c.graves {
		g.timer.Stop()
		item := left{terminal: g.node.Runtime.Terminal, session: g.node.Runtime.Session, adopted: g.node.Runtime.Adopted}
		if g.state != nil {
			item.inst = g.state.inst
		}
		all = append(all, item)
	}
	c.graves = map[string]*grave{}
	c.hub.close()
	if !c.closed {
		c.closed = true
		close(c.done)
	}
	c.mu.Unlock()
	// The runs of the sessions that go with the canvas are all told to stop
	// first: each session is deleted once its run is over (dropSession).
	for _, item := range all {
		if item.session != "" && !o.KeepSessions && !item.adopted {
			c.e.o.Host.StopRun(c.ws, item.session)
		}
	}
	for _, item := range all {
		if item.inst != nil {
			item.inst.stop()
		}
		if item.terminal != "" {
			if o.KeepTerminals {
				_ = c.e.o.Host.Terminals().Disown(item.terminal)
			} else {
				_ = c.e.o.Host.Terminals().Kill(item.terminal)
			}
		}
		if item.session != "" {
			c.e.unindexSession(c, item.session)
			c.dropSession(item.session, !o.KeepSessions && !item.adopted)
		}
	}
	c.e.forget(c)
	return removeCanvasFiles(c.ws.Path, c.id)
}

// dropSession lets go of an agent's session. Deleted, it is gone, its run
// stopped first; kept, or put on the canvas by the user (adopted), or when
// it cannot be deleted, the session list shows it as any other.
func (c *Canvas) dropSession(session string, deleteIt bool) {
	dir := c.ws.SessionDir
	meta := cockpit.LoadMeta(dir, session)
	if deleteIt && !meta.Adopted {
		err := c.deleteSession(session)
		if err == nil {
			return
		}
		c.e.o.Logf("canvas %s: the session %s of a node removed goes to the session list: %v", c.id, session, err)
	}
	if _, err := os.Stat(cockpit.SessionPath(dir, session)); errors.Is(err, fs.ErrNotExist) {
		_ = cockpit.DropMeta(dir, session)
		return
	}
	if err := cockpit.SetCanvasMeta(dir, session, meta.Title, "", ""); err != nil {
		c.e.o.Logf("canvas %s: let go of the session %s: %v", c.id, session, err)
	}
}

// sessionStops bounds how long deleting a session waits for its run to end.
const sessionStops = 30 * time.Second

// deleteSession deletes an agent's session. A run of it still going is told
// to stop, and the session goes once the run is over and its runner lets
// go of it.
func (c *Canvas) deleteSession(session string) error {
	dir, host := c.ws.SessionDir, c.e.o.Host
	deadline := time.Now().Add(sessionStops)
	stopped := false
	for {
		var err error
		if host.RunActive(c.ws, session) {
			if !stopped {
				stopped = true
				host.StopRun(c.ws, session)
			}
			err = errors.New("its run did not stop")
		} else {
			if _, statErr := os.Stat(cockpit.SessionPath(dir, session)); errors.Is(statErr, fs.ErrNotExist) {
				// An agent that never ran has no session file.
				return cockpit.DropMeta(dir, session)
			}
			if err = cockpit.DeleteSession(dir, session); !errors.Is(err, cockpit.ErrSessionBusy) {
				return err
			}
		}
		if time.Now().After(deadline) {
			return err
		}
		select {
		case <-c.e.ctx.Done():
			return err
		case <-time.After(250 * time.Millisecond):
		}
	}
}

// adopts reports whether a batch puts sessions the user has on the canvas.
func adopts(batch Batch) bool {
	for _, op := range batch.Ops {
		if op.Op == "node.add" && op.Node != nil && op.Node.Session != "" {
			return true
		}
	}
	return false
}

// adoptions checks the sessions a batch puts on the canvas and gives their
// titles, as the session list shows them. It runs before the canvas is
// locked: whether a session is the agent of another canvas's node may take
// loading that canvas.
func (c *Canvas) adoptions(actor Actor, batch Batch) (map[string]string, error) {
	dir := c.ws.SessionDir
	titles := map[string]string{}
	for i, op := range batch.Ops {
		if op.Op != "node.add" || op.Node == nil || op.Node.Session == "" {
			continue
		}
		session := op.Node.Session
		err := func() error {
			switch {
			case actor.node():
				return errForbidden("only the user puts a session on a canvas")
			case !cockpit.ValidSessionID(session):
				return errInvalid(fmt.Sprintf("%q is not a session's ID", session))
			}
			if _, twice := titles[session]; twice {
				return errInvalid("session " + session + " is put on the canvas twice")
			}
			if _, err := os.Stat(cockpit.SessionPath(dir, session)); err != nil {
				return errNotFound("no session " + session + " in this workspace")
			}
			if ref, ok := c.e.session(c.ws.ID, session, true); ok {
				node := "a node removed a moment ago"
				if n := ref.canvas.node(ref.node); n != nil {
					node = "«" + n.Title + "»"
				}
				return errConflict(fmt.Sprintf("session %s is already the agent of %s on the canvas «%s»: a session is on one canvas at a time", session, node, ref.canvas.summary().Title))
			}
			titles[session] = cockpit.ListedTitle(dir, session)
			return nil
		}()
		if err != nil {
			return nil, fmt.Errorf("operation %d (node.add): %w", i, err)
		}
	}
	return titles, nil
}

// branches copies the sessions of the kou agents a batch's new nodes copy
// (NodeSpec.Branch), by operation. It runs before the canvas is locked, as
// copying a session takes a while.
func (c *Canvas) branches(actor Actor, batch Batch) (map[int]string, error) {
	var branched map[int]string
	for i, op := range batch.Ops {
		if op.Op != "node.add" || op.Node == nil || op.Node.Branch == "" {
			continue
		}
		var session string
		var err error
		if spec := op.Node; spec.Session != "" || spec.ID != "" || spec.Restore {
			err = errInvalid("a node that copies an agent's session (branch) is a new node, with no session of the user's")
		} else {
			session, err = c.branch(actor, spec.Branch)
		}
		if err != nil {
			c.dropBranches(branched)
			return nil, fmt.Errorf("operation %d (node.add): %w", i, err)
		}
		if session != "" {
			if branched == nil {
				branched = map[int]string{}
			}
			branched[i] = session
		}
	}
	return branched, nil
}

// branch copies the session of a kou agent's node, whole, into a new
// session: none when the agent never ran.
func (c *Canvas) branch(actor Actor, id string) (string, error) {
	if actor.node() {
		return "", errForbidden("only the user copies an agent's session")
	}
	n := c.node(id)
	if n == nil {
		return "", errNotFound("no node " + id + " to copy")
	}
	if n.Kind != KindAgent {
		return "", errInvalid("«" + n.Title + "» is no kou agent: only an agent's session is copied")
	}
	dir, session := c.ws.SessionDir, n.Runtime.Session
	if session == "" {
		return "", nil
	}
	if _, err := os.Stat(cockpit.SessionPath(dir, session)); errors.Is(err, fs.ErrNotExist) {
		return "", nil
	}
	busy := errConflict("«" + n.Title + "» is at work: copy it once its run is over, or stop it")
	if c.e.o.Host.RunActive(c.ws, session) {
		return "", busy
	}
	branch, err := cockpit.BranchSession(c.e.ctx, dir, session, "")
	switch {
	case errors.Is(err, cockpit.ErrBranchRunning):
		return "", busy
	case err != nil:
		return "", fmt.Errorf("copy the session of «%s»: %w", n.Title, err)
	}
	return branch.SessionID, nil
}

// dropBranches deletes the copies of sessions made for a batch that was
// not applied.
func (c *Canvas) dropBranches(branched map[int]string) {
	for _, session := range branched {
		if err := cockpit.DeleteSession(c.ws.SessionDir, session); err != nil {
			c.e.o.Logf("canvas %s: delete the copy %s of a session: %v", c.id, session, err)
		}
	}
}

// snapshot is a canvas as a page starts from.
type snapshot struct {
	Type      string            `json:"type"`
	Workspace string            `json:"workspace"`
	Doc       *Doc              `json:"doc"`
	ReadOnly  bool              `json:"read_only,omitzero"`
	Status    map[string]Status `json:"status"`
	// Pending counts the messages that wait for each node; Queue lists
	// them.
	Pending  map[string]int `json:"pending"`
	Queue    []*Message     `json:"queue"`
	Messages []*Message     `json:"messages"`
	// Terminals describe the nodes' shells.
	Terminals map[string]terminal.Info `json:"terminals"`
	// Hooks are the paths that fire the webhook sources.
	Hooks map[string]string `json:"hooks"`
}

func (c *Canvas) snapshotLocked(kind string) snapshot {
	s := snapshot{
		Type: kind, Workspace: c.ws.ID, Doc: c.doc, ReadOnly: c.readOnly,
		Status: map[string]Status{}, Pending: map[string]int{}, Queue: []*Message{}, Messages: []*Message{},
		Terminals: map[string]terminal.Info{}, Hooks: map[string]string{},
	}
	for id, st := range c.states {
		s.Status[id] = st.status
		if st.terminal != nil {
			s.Terminals[id] = st.terminal.Info()
		}
	}
	for _, m := range c.pending {
		s.Pending[m.To.Node]++
		s.Queue = append(s.Queue, m.preview())
	}
	start := max(0, len(c.messages)-100)
	for _, m := range c.messages[start:] {
		s.Messages = append(s.Messages, m.preview())
	}
	for _, n := range c.doc.Nodes {
		if n.Kind == KindSource && n.Preset == "webhook" && n.Plugin == "" {
			s.Hooks[n.ID] = "/api/canvas/hooks/" + c.hookID(n.ID)
		}
	}
	return s
}

// Summary describes a canvas in the list of a workspace's.
type Summary struct {
	ID        string    `json:"id"`
	Title     string    `json:"title"`
	Live      bool      `json:"live"`
	UpdatedAt time.Time `json:"updated_at"`
	CreatedAt time.Time `json:"created_at"`
	Nodes     int       `json:"nodes"`
	Agents    int       `json:"agents"`
	Terminals int       `json:"terminals"`
	Busy      int       `json:"busy"`
	Waiting   int       `json:"waiting"`
	Loaded    bool      `json:"loaded"`
	ReadOnly  bool      `json:"read_only,omitzero"`
}

func summaryOf(doc *Doc) Summary {
	s := Summary{ID: doc.ID, Title: doc.Title, Live: doc.Live, UpdatedAt: doc.UpdatedAt, CreatedAt: doc.CreatedAt, Nodes: len(doc.Nodes)}
	for _, n := range doc.Nodes {
		switch {
		case n.agentish():
			s.Agents++
		}
		if n.Kind == KindTerminal {
			s.Terminals++
		}
	}
	return s
}

func (c *Canvas) summary() Summary {
	c.mu.Lock()
	defer c.mu.Unlock()
	s := summaryOf(c.doc)
	s.Loaded, s.ReadOnly = true, c.readOnly
	for _, st := range c.states {
		switch st.status.State {
		case StateBusy, StateStarting:
			// What runs on quietly — a server, a watcher — is not busy.
			if !st.status.Quiet {
				s.Busy++
			}
		case StateWaiting:
			s.Waiting++
		}
	}
	return s
}
