package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"time"
)

// A canvas changes by batches of operations, all of which apply or none:
//
//	node.add      {"node": {kind, preset, plugin, title, x, y, w, h, near, side, exact, config, access, prompt, connect, id, restore}}
//	node.update   {"id", "set": {title, x, y, w, h, z, config, access, proposed}}
//	node.remove   {"id", "keep": {"session"}, "grace_s"}
//	edge.add      {"edge": {from: {node, port}, to: {node, port}, template, mode, deliver, header, id}}
//	edge.update   {"id", "set": {template, mode, deliver, header}}
//	edge.remove   {"id"}
//	canvas.update {"set": {title, live, settings}}
//
// An ID may be "$<n>", what the batch's operation n added, or "self", the
// node of the agent that sends the batch. Places left out are found
// (place.go). A batch made against an older revision of the canvas
// (base_rev) still applies, the last change of a place winning, unless it
// names what is gone: that is a conflict. Pages hear of every batch as the
// changes it made, nodes and edges whole; undoing is theirs.

// Actor is who changes a canvas: the user, a node's program, or the
// engine itself.
type Actor struct {
	Kind string `json:"kind"`
	ID   string `json:"id,omitzero"`
	// Scope is what a node's program may do.
	Scope string `json:"-"`
}

var userActor = Actor{Kind: "user"}

func (a Actor) node() bool { return a.Kind == "node" }

// name is who an actor is, in a node's created_by.
func (a Actor) name() string {
	if a.node() {
		return a.ID
	}
	return "user"
}

// Batch is a batch of operations.
type Batch struct {
	BaseRev *int64 `json:"base_rev"`
	Ops     []Op   `json:"ops"`
	// Create makes the canvas when it is not there yet, titled Title: a
	// page makes a canvas with its first node.
	Create bool   `json:"create"`
	Title  string `json:"title"`
}

// Op is an operation of a batch.
type Op struct {
	Op     string         `json:"op"`
	Node   *NodeSpec      `json:"node"`
	ID     string         `json:"id"`
	Set    jsontext.Value `json:"set"`
	Keep   *Keep          `json:"keep"`
	GraceS *int           `json:"grace_s"`
	Edge   *EdgeSpec      `json:"edge"`
}

// Keep says what a node removed leaves.
type Keep struct {
	// Session keeps a kou agent's session, which the session list shows;
	// otherwise it is deleted with the node.
	Session bool `json:"session"`
	// Worktree is always kept: a worktree is removed on purpose only.
	Worktree bool `json:"worktree"`
}

// NodeSpec is a node to add.
type NodeSpec struct {
	// ID brings a removed node back, with Restore while it can come back
	// whole: its shell and its edges.
	ID      string `json:"id"`
	Restore bool   `json:"restore"`
	Kind    string `json:"kind"`
	Preset  string `json:"preset"`
	Plugin  string `json:"plugin"`
	Title   string `json:"title"`
	X       *int   `json:"x"`
	Y       *int   `json:"y"`
	W       *int   `json:"w"`
	H       *int   `json:"h"`
	// Near places it beside a node, on Side; Exact keeps the place given
	// even where it covers another node.
	Near   string         `json:"near"`
	Side   string         `json:"side"`
	Exact  bool           `json:"exact"`
	Config jsontext.Value `json:"config"`
	Access string         `json:"access"`
	// Prompt is sent to it once it runs.
	Prompt string `json:"prompt"`
	// Connect wires it: From's output to its input, its output to To's
	// input; "node" or "node:port".
	Connect *Connect `json:"connect"`
}

// Connect is how a new node is wired.
type Connect struct {
	From     string `json:"from"`
	To       string `json:"to"`
	Template string `json:"template"`
	Mode     string `json:"mode"`
}

// EdgeSpec is an edge to add.
type EdgeSpec struct {
	ID       string `json:"id"`
	From     Port   `json:"from"`
	To       Port   `json:"to"`
	Template string `json:"template"`
	Mode     string `json:"mode"`
	Deliver  string `json:"deliver"`
	Header   *bool  `json:"header"`
}

// Result is what a batch did: the canvas's revision after it, the IDs of
// what it added by the index of their operations, and the places of the
// nodes it placed.
type Result struct {
	Rev    int64             `json:"rev"`
	IDs    map[string]string `json:"ids"`
	Placed map[string]Rect   `json:"placed"`
}

// Change is what a batch changed, as the pages are told.
type Change struct {
	Op     string        `json:"op"`
	Node   *Node         `json:"node,omitzero"`
	Edge   *Edge         `json:"edge,omitzero"`
	ID     string        `json:"id,omitzero"`
	Canvas *CanvasChange `json:"canvas,omitzero"`
}

// CanvasChange is a canvas's own fields after a canvas.update.
type CanvasChange struct {
	Title    string   `json:"title"`
	Live     bool     `json:"live"`
	Settings Settings `json:"settings"`
}

const (
	maxOps      = 200
	maxConfig   = 64 << 10
	defaultKeep = 15 * time.Second
	maxGrace    = 5 * time.Minute
)

// applier applies a batch to a copy of the document.
type applier struct {
	c      *Canvas
	actor  Actor
	next   *Doc
	stale  bool
	ids    map[string]string
	placed map[string]Rect
	// What changed, in order, to tell the pages.
	nodes, edges []string
	canvas       bool
	// What to do once the batch is in.
	added    []addition
	removed  []removal
	approved []string
	retitled []string
	liveFrom *bool
}

type addition struct {
	id      string
	prompt  string
	restore *grave
}

type removal struct {
	node  *Node
	edges []*Edge // the edges that went with it
	keep  Keep
	// grace is how long its shell lives on, and it can come back.
	grace time.Duration
}

// apply applies a batch of operations by actor.
func (c *Canvas) apply(actor Actor, batch Batch) (Result, error) {
	if len(batch.Ops) == 0 {
		return Result{}, errInvalid("a batch needs operations")
	}
	if len(batch.Ops) > maxOps {
		return Result{}, errInvalid(fmt.Sprintf("a batch holds %d operations at most", maxOps))
	}
	catalog := c.e.catalog(c.ws)
	c.mu.Lock()
	if c.deleted {
		c.mu.Unlock()
		return Result{}, errNotFound("the canvas was deleted")
	}
	if c.readOnly {
		c.mu.Unlock()
		return Result{}, errConflict("a newer version of kou-conveyor made this canvas: it is read-only here")
	}
	a := &applier{
		c: c, actor: actor, next: c.doc.clone(), ids: map[string]string{}, placed: map[string]Rect{},
		stale: batch.BaseRev != nil && *batch.BaseRev != c.doc.Rev,
	}
	for i, op := range batch.Ops {
		if err := a.apply(i, op, catalog); err != nil {
			c.mu.Unlock()
			if op, ok := err.(*opError); ok && op.status == 409 {
				op.rev = c.doc.Rev
			}
			return Result{}, fmt.Errorf("operation %d (%s): %w", i, op.Op, err)
		}
	}
	if err := a.limits(); err != nil {
		c.mu.Unlock()
		return Result{}, err
	}
	a.next.Rev = c.doc.Rev + 1
	a.next.UpdatedAt = now()
	before := c.doc
	c.doc = a.next
	c.touchLocked()
	if actor.node() {
		for range a.added {
			c.spawns = append(c.spawns, time.Now())
		}
	}
	c.hub.publish(map[string]any{"type": "ops", "rev": c.doc.Rev, "actor": actor, "changes": a.changes(before)})
	after := a.commitLocked()
	c.mu.Unlock()
	for _, f := range after {
		f()
	}
	return Result{Rev: a.next.Rev, IDs: a.ids, Placed: a.placed}, nil
}

// resolve turns a reference into a node's ID.
func (a *applier) resolve(ref string) string {
	if rest, ok := strings.CutPrefix(ref, "$"); ok {
		return a.ids[rest]
	}
	if ref == "self" && a.actor.node() {
		return a.actor.ID
	}
	return ref
}

// need checks that the actor may do what scope allows.
func (a *applier) need(scope string) error {
	if !a.actor.node() || allows(a.actor.Scope, scope) {
		return nil
	}
	return errForbidden(fmt.Sprintf("your access is %s; this needs %s", a.actor.Scope, scope))
}

// own reports whether the actor may change a node as its own: a node it
// made, or any with admin.
func (a *applier) own(n *Node) bool {
	return !a.actor.node() || n.CreatedBy == a.actor.ID || allows(a.actor.Scope, ScopeAdmin)
}

func (a *applier) missing(what, id string) error {
	if a.stale {
		return errConflict(fmt.Sprintf("%s %s is gone: the canvas changed meanwhile", what, id))
	}
	return errNotFound(fmt.Sprintf("no %s %s", what, id))
}

func (a *applier) apply(i int, op Op, catalog presetCache) error {
	switch op.Op {
	case "node.add":
		return a.addNode(i, op.Node, catalog)
	case "node.update":
		return a.updateNode(op)
	case "node.remove":
		return a.removeNode(op)
	case "edge.add":
		if op.Edge == nil {
			return errInvalid("edge.add needs an edge")
		}
		id, err := a.addEdge(*op.Edge, catalog)
		if err == nil {
			a.ids[strconv.Itoa(i)] = id
		}
		return err
	case "edge.update":
		return a.updateEdge(op)
	case "edge.remove":
		return a.removeEdge(op)
	case "canvas.update":
		return a.updateCanvas(op)
	}
	return errInvalid(fmt.Sprintf("no operation %q", op.Op))
}

// defaultAccess is what a node's program may do when the user made it.
func defaultAccess(kind, preset string) string {
	switch {
	case kind == KindAgent && preset == "foreman":
		return ScopeAdmin
	case kind == KindAgent:
		return ScopeBuild
	case kind == KindTerminal && preset != "" && preset != "shell" && preset != "command":
		return ScopeBuild
	case kind == KindTerminal:
		return ScopeTalk
	}
	return ""
}

func (a *applier) addNode(i int, spec *NodeSpec, catalog presetCache) error {
	if err := a.need(ScopeBuild); err != nil {
		return err
	}
	if spec == nil {
		return errInvalid("node.add needs a node")
	}
	inferKind(spec, catalog)
	c, next := a.c, a.next
	var node *Node
	var restored *grave
	if spec.ID != "" {
		if !nodeIDPattern.MatchString(spec.ID) {
			return errInvalid("a node's ID is n_ and letters and digits")
		}
		if next.node(spec.ID) != nil {
			return errConflict("node " + spec.ID + " is there already")
		}
		if spec.Restore {
			g := c.graves[spec.ID]
			if g == nil {
				return errInvalid("node " + spec.ID + " can no longer come back: it is gone for good")
			}
			restored = g
			copied := *g.node
			node = &copied
		}
	}
	if node == nil {
		var err error
		if node, err = a.newNode(spec, catalog); err != nil {
			return err
		}
		if spec.ID != "" {
			node.ID = spec.ID
		}
	}
	w, h := node.W, node.H
	if spec.W != nil {
		w = clampSize(*spec.W, minWidth)
	}
	if spec.H != nil {
		h = clampSize(*spec.H, minHeight)
	}
	taken := make([]Rect, 0, len(next.Nodes))
	z := 0
	for _, other := range next.Nodes {
		taken = append(taken, other.rect())
		z = max(z, other.Z)
	}
	var r Rect
	switch {
	case spec.X != nil && spec.Y != nil:
		r = Rect{clampCoordinate(*spec.X), clampCoordinate(*spec.Y), w, h}
		if !spec.Exact {
			r = shiftFree(taken, r)
		}
	case restored != nil && spec.Near == "":
		r = Rect{node.X, node.Y, w, h}
	default:
		var anchor *Rect
		near := a.resolve(spec.Near)
		if near == "" && a.actor.node() {
			near = a.actor.ID
		}
		if near != "" {
			other := next.node(near)
			if other == nil {
				return a.missing("node", spec.Near)
			}
			at := other.rect()
			anchor = &at
		}
		if spec.Side != "" && !validSide(spec.Side) {
			return errInvalid("side is right, below, left or above")
		}
		r = placeNear(taken, anchor, spec.Side, w, h)
	}
	node.X, node.Y, node.W, node.H = r.X, r.Y, r.W, r.H
	if restored == nil || node.Z == 0 {
		node.Z = z + 1
	}
	next.Nodes = append(next.Nodes, node)
	a.ids[strconv.Itoa(i)] = node.ID
	a.placed[node.ID] = r
	a.nodes = append(a.nodes, node.ID)
	a.added = append(a.added, addition{id: node.ID, prompt: strings.TrimSpace(spec.Prompt), restore: restored})
	if restored != nil {
		for _, e := range restored.edges {
			if next.edge(e.ID) != nil || next.node(e.From.Node) == nil || next.node(e.To.Node) == nil {
				continue
			}
			copied := *e
			next.Edges = append(next.Edges, &copied)
			a.edges = append(a.edges, e.ID)
		}
	}
	if spec.Connect != nil {
		if from := spec.Connect.From; from != "" {
			ref, port := splitPort(from, "out")
			if _, err := a.addEdge(EdgeSpec{From: Port{ref, port}, To: Port{node.ID, "in"}, Template: spec.Connect.Template, Mode: spec.Connect.Mode}, catalog); err != nil {
				return err
			}
		}
		if to := spec.Connect.To; to != "" {
			ref, port := splitPort(to, "in")
			if _, err := a.addEdge(EdgeSpec{From: Port{node.ID, "out"}, To: Port{ref, port}, Template: spec.Connect.Template, Mode: spec.Connect.Mode}, catalog); err != nil {
				return err
			}
		}
	}
	return nil
}

// inferKind fills in the kind of a node to add that names only its preset,
// as agents' tools do: agent, foreman, note, a terminal's preset or a
// source's, "plugin/preset" for a plugin's.
func inferKind(spec *NodeSpec, catalog presetCache) {
	if spec.Plugin == "" {
		if plugin, preset, ok := strings.Cut(spec.Preset, "/"); ok {
			spec.Plugin, spec.Preset = plugin, preset
		}
	}
	if spec.Kind != "" {
		return
	}
	switch spec.Preset {
	case "", "agent":
		spec.Kind, spec.Preset = KindAgent, ""
	case "foreman":
		spec.Kind = KindAgent
	case "note":
		spec.Kind, spec.Preset = KindNote, ""
	default:
		plugin := spec.Plugin
		if plugin == canvasPlugin {
			plugin = ""
		}
		if _, ok := catalog.harness(plugin, spec.Preset); ok {
			spec.Kind = KindTerminal
		} else if _, ok := catalog.source(plugin, spec.Preset); ok {
			spec.Kind = KindSource
		}
	}
}

// splitPort reads "node:port", the port fallback when left out.
func splitPort(ref, fallback string) (string, string) {
	if node, port, ok := strings.Cut(ref, ":"); ok && port != "" {
		return node, port
	}
	return strings.TrimSuffix(ref, ":"), fallback
}

func clampSize(v, least int) int { return min(max(v, least), maxSize) }

func clampCoordinate(v int) int { return min(max(v, -maxCoordinate+maxSize), maxCoordinate-maxSize) }

// newNode makes a node of a spec, checked.
func (a *applier) newNode(spec *NodeSpec, catalog presetCache) (*Node, error) {
	c, next := a.c, a.next
	settings := next.Settings.Autonomy
	n := &Node{ID: newID("n_", 6), Kind: spec.Kind, Preset: spec.Preset, Plugin: spec.Plugin, CreatedBy: a.actor.name()}
	if n.Plugin == canvasPlugin {
		n.Plugin = ""
	}
	if n.Plugin != "" && !pluginPattern.MatchString(n.Plugin) {
		return nil, errInvalid("no plugin " + n.Plugin)
	}
	if n.Preset != "" && !presetPattern.MatchString(n.Preset) {
		return nil, errInvalid("no preset " + n.Preset)
	}
	title := ""
	switch n.Kind {
	case KindTerminal:
		if n.Preset == "" {
			n.Preset = "shell"
		}
		def, ok := catalog.harness(n.Plugin, n.Preset)
		if !ok {
			return nil, errInvalid(fmt.Sprintf("no terminal preset %q", presetName(n.Plugin, n.Preset)))
		}
		title = def.Harness.Title
		if n.Preset == "command" {
			title = "Command"
		}
	case KindAgent:
		if n.Preset != "" && n.Preset != "foreman" {
			return nil, errInvalid("an agent's preset is foreman or none")
		}
		title = "Agent"
		if n.Preset == "foreman" {
			title = "Foreman"
		}
	case KindSource:
		def, ok := catalog.source(n.Plugin, n.Preset)
		if !ok {
			return nil, errInvalid(fmt.Sprintf("no source %q", presetName(n.Plugin, n.Preset)))
		}
		title = def.Source.Title
	case KindNote:
		n.Preset, n.Plugin = "", ""
		title = "Note"
	default:
		return nil, errInvalid("a node's kind is terminal, agent, source or note")
	}
	if n.Title = cleanTitle(spec.Title); n.Title == "" {
		n.Title = title
	}
	if len(spec.Config) != 0 {
		if spec.Config.Kind() != '{' {
			return nil, errInvalid("a node's config is an object")
		}
		if len(spec.Config) > maxConfig {
			return nil, errInvalid("the config is too large")
		}
		n.Config = append(jsontext.Value(nil), spec.Config...)
	}
	if n.Kind == KindTerminal && n.Preset == "command" && n.Plugin == "" && strings.TrimSpace(n.configString("command")) == "" {
		return nil, errInvalid("a command node needs config.command")
	}
	n.W, n.H = defaultSize(n.Kind, n.Preset)
	access := defaultAccess(n.Kind, n.Preset)
	if spec.Access != "" {
		if _, ok := scopeRank[spec.Access]; !ok {
			return nil, errInvalid("access is none, observe, talk, build or admin")
		}
		access = spec.Access
	}
	if a.actor.node() {
		creator := next.node(a.actor.ID)
		if creator != nil {
			n.Depth = creator.Depth + 1
		}
		switch settings.Spawn {
		case "deny":
			return nil, errForbidden("this canvas does not let agents make nodes")
		case "ask":
			n.Proposed = true
		}
		if n.Depth > settings.MaxDepth {
			return nil, errForbidden(fmt.Sprintf("nodes are made %d agents deep at most here", settings.MaxDepth))
		}
		recent := 0
		for _, at := range c.spawns {
			if time.Since(at) < time.Minute {
				recent++
			}
		}
		if recent+len(a.added) >= settings.SpawnsPerMinute {
			return nil, errLimit(fmt.Sprintf("agents make %d nodes a minute at most here: wait a little", settings.SpawnsPerMinute))
		}
		// An agent gives what it makes talk, or build if it has it and the
		// node may make nodes in turn; never admin.
		switch {
		case access == ScopeAdmin:
			access = ScopeBuild
		case spec.Access == "":
			access = ScopeTalk
		}
		if access == ScopeBuild && (!allows(a.actor.Scope, ScopeBuild) || n.Depth >= settings.MaxDepth) {
			access = ScopeTalk
		}
		if n.Kind != KindTerminal && n.Kind != KindAgent {
			access = ""
		}
	}
	if n.Kind == KindSource || n.Kind == KindNote {
		access = ""
	}
	n.Access = access
	return n, nil
}

// limits checks what agents may make against the canvas's settings.
func (a *applier) limits() error {
	if !a.actor.node() || len(a.added) == 0 {
		return nil
	}
	settings := a.next.Settings.Autonomy
	terminals := 0
	for _, n := range a.next.Nodes {
		if n.Kind == KindTerminal {
			terminals++
		}
	}
	switch {
	case len(a.next.Nodes) > settings.MaxNodes:
		return errLimit(fmt.Sprintf("this canvas has %d nodes at most", settings.MaxNodes))
	case terminals > settings.MaxTerminals:
		return errLimit(fmt.Sprintf("this canvas has %d terminals at most", settings.MaxTerminals))
	}
	return nil
}

// decodeSet reads an operation's set: an object of the keys allowed.
func decodeSet(set jsontext.Value, allowed ...string) (map[string]jsontext.Value, error) {
	if len(set) == 0 {
		return nil, errInvalid("set is required")
	}
	var fields map[string]jsontext.Value
	if set.Kind() != '{' || json.Unmarshal(set, &fields) != nil {
		return nil, errInvalid("set is an object")
	}
	for key := range fields {
		if !slices.Contains(allowed, key) {
			return nil, errInvalid(fmt.Sprintf("%s cannot be set: only %s", key, strings.Join(allowed, ", ")))
		}
	}
	return fields, nil
}

func (a *applier) updateNode(op Op) error {
	id := a.resolve(op.ID)
	n := a.next.node(id)
	if n == nil {
		return a.missing("node", op.ID)
	}
	fields, err := decodeSet(op.Set, "title", "x", "y", "w", "h", "z", "config", "access", "proposed")
	if err != nil {
		return err
	}
	for _, key := range sortedKeys(fields) {
		value := fields[key]
		switch key {
		case "x", "y", "w", "h", "z":
			if err := a.need(ScopeBuild); err != nil {
				return err
			}
			var v int
			if err := json.Unmarshal(value, &v); err != nil {
				return errInvalid(key + " is a number")
			}
			switch key {
			case "x":
				n.X = clampCoordinate(v)
			case "y":
				n.Y = clampCoordinate(v)
			case "w":
				n.W = clampSize(v, minWidth)
			case "h":
				n.H = clampSize(v, minHeight)
			case "z":
				n.Z = v
			}
		case "title":
			if err := a.need(ScopeBuild); err != nil {
				return err
			}
			if !a.own(n) && n.ID != a.actor.ID {
				return errForbidden("«" + n.Title + "» is not yours to rename")
			}
			var title string
			if err := json.Unmarshal(value, &title); err != nil || cleanTitle(title) == "" {
				return errInvalid("title is a text")
			}
			n.Title = cleanTitle(title)
			a.retitled = append(a.retitled, n.ID)
		case "config":
			if err := a.need(ScopeBuild); err != nil {
				return err
			}
			if !a.own(n) {
				return errForbidden("«" + n.Title + "» is not yours to configure")
			}
			config, err := mergeConfig(n.Config, value)
			if err != nil {
				return err
			}
			n.Config = config
		case "access":
			var access string
			if err := json.Unmarshal(value, &access); err != nil {
				return errInvalid("access is a text")
			}
			if _, ok := scopeRank[access]; !ok {
				return errInvalid("access is none, observe, talk, build or admin")
			}
			if a.actor.node() {
				if err := a.need(ScopeAdmin); err != nil {
					return err
				}
				if access == ScopeAdmin {
					return errForbidden("only the user gives admin")
				}
			}
			if n.Kind != KindTerminal && n.Kind != KindAgent {
				return errInvalid("only terminals and agents have access")
			}
			n.Access = access
		case "proposed":
			var proposed bool
			if err := json.Unmarshal(value, &proposed); err != nil || proposed {
				return errInvalid("proposed can only be set to false: the node is approved")
			}
			if a.actor.node() {
				return errForbidden("only the user approves the nodes agents make")
			}
			if n.Proposed {
				n.Proposed = false
				a.approved = append(a.approved, n.ID)
			}
		}
	}
	a.nodes = append(a.nodes, n.ID)
	return nil
}

// mergeConfig merges a change into a node's configuration: keys set to
// null go.
func mergeConfig(config jsontext.Value, change jsontext.Value) (jsontext.Value, error) {
	var changes map[string]jsontext.Value
	if change.Kind() != '{' || json.Unmarshal(change, &changes) != nil {
		return nil, errInvalid("config is an object")
	}
	merged := map[string]jsontext.Value{}
	if len(config) != 0 {
		_ = json.Unmarshal(config, &merged)
	}
	for key, value := range changes {
		if value.Kind() == 'n' {
			delete(merged, key)
		} else {
			merged[key] = value
		}
	}
	data, err := json.Marshal(merged, json.Deterministic(true))
	if err != nil {
		return nil, errInvalid(err.Error())
	}
	if len(data) > maxConfig {
		return nil, errInvalid("the config is too large")
	}
	if len(merged) == 0 {
		return nil, nil
	}
	return data, nil
}

func (a *applier) removeNode(op Op) error {
	if err := a.need(ScopeBuild); err != nil {
		return err
	}
	id := a.resolve(op.ID)
	n := a.next.node(id)
	if n == nil {
		return a.missing("node", op.ID)
	}
	if a.actor.node() {
		if id == a.actor.ID {
			return errForbidden("a node does not remove itself")
		}
		if !a.own(n) {
			return errForbidden("«" + n.Title + "» is not yours to remove: you remove the nodes you made")
		}
	}
	keep := Keep{Session: true, Worktree: true}
	if op.Keep != nil {
		keep = Keep{Session: op.Keep.Session, Worktree: true}
	}
	grace := defaultKeep
	if op.GraceS != nil {
		grace = min(max(time.Duration(*op.GraceS)*time.Second, 0), maxGrace)
	}
	removed := *n
	r := removal{node: &removed, keep: keep, grace: grace}
	a.next.Nodes = slices.DeleteFunc(a.next.Nodes, func(other *Node) bool { return other.ID == id })
	a.next.Edges = slices.DeleteFunc(a.next.Edges, func(e *Edge) bool {
		if e.From.Node == id || e.To.Node == id {
			a.edges = append(a.edges, e.ID)
			copied := *e
			r.edges = append(r.edges, &copied)
			return true
		}
		return false
	})
	a.nodes = append(a.nodes, id)
	a.removed = append(a.removed, r)
	return nil
}

// portsOf lists a node's inputs and outputs.
func portsOf(n *Node, catalog presetCache) (inputs, outputs []string) {
	switch n.Kind {
	case KindTerminal:
		return []string{"in"}, []string{"out", "exit"}
	case KindAgent:
		return []string{"in"}, []string{"out"}
	case KindSource:
		if def, ok := catalog.source(n.Plugin, n.Preset); ok {
			for _, port := range def.Source.Outputs {
				outputs = append(outputs, port.ID)
			}
			return nil, outputs
		}
		return nil, []string{"out"}
	}
	return nil, nil
}

func (a *applier) addEdge(spec EdgeSpec, catalog presetCache) (string, error) {
	if err := a.need(ScopeBuild); err != nil {
		return "", err
	}
	from, to := a.resolve(spec.From.Node), a.resolve(spec.To.Node)
	if spec.From.Port == "" {
		spec.From.Port = "out"
	}
	if spec.To.Port == "" {
		spec.To.Port = "in"
	}
	source, target := a.next.node(from), a.next.node(to)
	switch {
	case source == nil:
		return "", a.missing("node", spec.From.Node)
	case target == nil:
		return "", a.missing("node", spec.To.Node)
	case from == to:
		return "", errInvalid("a node is not wired to itself")
	}
	if _, outputs := portsOf(source, catalog); !slices.Contains(outputs, spec.From.Port) {
		return "", errInvalid(fmt.Sprintf("«%s» has no output %q", source.Title, spec.From.Port))
	}
	if inputs, _ := portsOf(target, catalog); !slices.Contains(inputs, spec.To.Port) {
		return "", errInvalid(fmt.Sprintf("«%s» has no input %q", target.Title, spec.To.Port))
	}
	// An edge from a source reads its events as the source says, unless it
	// says otherwise.
	if spec.Template == "" && source.Kind == KindSource {
		if def, ok := catalog.source(source.Plugin, source.Preset); ok {
			spec.Template = def.Source.Template
		}
	}
	if err := checkEdgeFields(spec.Template, spec.Mode, spec.Deliver); err != nil {
		return "", err
	}
	for _, e := range a.next.Edges {
		if e.From == (Port{from, spec.From.Port}) && e.To == (Port{to, spec.To.Port}) {
			return e.ID, nil
		}
	}
	e := &Edge{
		ID: newID("e_", 6), From: Port{from, spec.From.Port}, To: Port{to, spec.To.Port},
		Template: spec.Template, Mode: spec.Mode, Deliver: spec.Deliver, Header: spec.Header,
	}
	if spec.ID != "" {
		if !edgeIDPattern.MatchString(spec.ID) {
			return "", errInvalid("an edge's ID is e_ and letters and digits")
		}
		if a.next.edge(spec.ID) != nil {
			return "", errConflict("edge " + spec.ID + " is there already")
		}
		e.ID = spec.ID
	}
	// Two agents answering each other run on forever: the edge that closes
	// such a loop has each message approved, unless it says otherwise.
	if e.Mode == "" && source.agentish() && target.agentish() && a.reaches(to, from) {
		e.Mode = ModeApprove
	}
	a.next.Edges = append(a.next.Edges, e)
	a.edges = append(a.edges, e.ID)
	return e.ID, nil
}

// reaches reports whether messages go from one node to another along the
// edges between agents.
func (a *applier) reaches(from, to string) bool {
	seen := map[string]bool{}
	queue := []string{from}
	for len(queue) > 0 {
		id := queue[0]
		queue = queue[1:]
		if id == to {
			return true
		}
		if seen[id] {
			continue
		}
		seen[id] = true
		for _, e := range a.next.Edges {
			if e.From.Node == id && e.mode() != ModeOff {
				if n := a.next.node(e.To.Node); n != nil && n.agentish() {
					queue = append(queue, e.To.Node)
				}
			}
		}
	}
	return false
}

func checkEdgeFields(template, mode, deliver string) error {
	if len(template) > maxTemplate {
		return errInvalid(fmt.Sprintf("a template is %d KiB at most", maxTemplate>>10))
	}
	switch mode {
	case "", ModeAuto, ModeApprove, ModeOff:
	default:
		return errInvalid("mode is auto, approve or off")
	}
	switch deliver {
	case "", DeliverQueue, DeliverNow:
	default:
		return errInvalid("deliver is queue or now")
	}
	return nil
}

func (a *applier) updateEdge(op Op) error {
	if err := a.need(ScopeBuild); err != nil {
		return err
	}
	e := a.next.edge(a.resolve(op.ID))
	if e == nil {
		return a.missing("edge", op.ID)
	}
	fields, err := decodeSet(op.Set, "template", "mode", "deliver", "header")
	if err != nil {
		return err
	}
	template, mode, deliver := e.Template, e.Mode, e.Deliver
	for key, value := range fields {
		switch key {
		case "template":
			err = json.Unmarshal(value, &template)
		case "mode":
			err = json.Unmarshal(value, &mode)
		case "deliver":
			err = json.Unmarshal(value, &deliver)
		case "header":
			if value.Kind() == 'n' {
				e.Header = nil
			} else {
				var header bool
				err = json.Unmarshal(value, &header)
				e.Header = &header
			}
		}
		if err != nil {
			return errInvalid(key + ": " + err.Error())
		}
	}
	if err := checkEdgeFields(template, mode, deliver); err != nil {
		return err
	}
	e.Template, e.Mode, e.Deliver = template, mode, deliver
	if e.Mode == ModeAuto {
		e.Mode = ""
	}
	if e.Deliver == DeliverQueue {
		e.Deliver = ""
	}
	a.edges = append(a.edges, e.ID)
	return nil
}

func (a *applier) removeEdge(op Op) error {
	if err := a.need(ScopeBuild); err != nil {
		return err
	}
	id := a.resolve(op.ID)
	if a.next.edge(id) == nil {
		return a.missing("edge", op.ID)
	}
	a.next.Edges = slices.DeleteFunc(a.next.Edges, func(e *Edge) bool { return e.ID == id })
	a.edges = append(a.edges, id)
	return nil
}

func (a *applier) updateCanvas(op Op) error {
	if err := a.need(ScopeAdmin); err != nil {
		return err
	}
	fields, err := decodeSet(op.Set, "title", "live", "settings")
	if err != nil {
		return err
	}
	for key, value := range fields {
		switch key {
		case "title":
			var title string
			if err := json.Unmarshal(value, &title); err != nil || cleanTitle(title) == "" {
				return errInvalid("title is a text")
			}
			a.next.Title = cleanTitle(title)
		case "live":
			var live bool
			if err := json.Unmarshal(value, &live); err != nil {
				return errInvalid("live is true or false")
			}
			if live != a.next.Live {
				was := a.next.Live
				a.liveFrom = &was
			}
			a.next.Live = live
		case "settings":
			settings := a.next.Settings
			if err := json.Unmarshal(value, &settings); err != nil {
				return errInvalid("settings: " + err.Error())
			}
			if err := settings.validate(); err != nil {
				return errInvalid(err.Error())
			}
			a.next.Settings = settings
		}
	}
	a.canvas = true
	return nil
}

// changes are what the batch changed, as the pages apply it: a node or an
// edge that is there is given whole, one that is not is removed.
func (a *applier) changes(before *Doc) []Change {
	var out []Change
	seen := map[string]bool{}
	for _, id := range a.nodes {
		if seen[id] {
			continue
		}
		seen[id] = true
		if n := a.next.node(id); n != nil {
			copied := *n
			kind := "node.update"
			if before.node(id) == nil {
				kind = "node.add"
			}
			out = append(out, Change{Op: kind, Node: &copied})
		} else if before.node(id) != nil {
			out = append(out, Change{Op: "node.remove", ID: id})
		}
	}
	for _, id := range a.edges {
		if seen[id] {
			continue
		}
		seen[id] = true
		if e := a.next.edge(id); e != nil {
			copied := *e
			kind := "edge.update"
			if before.edge(id) == nil {
				kind = "edge.add"
			}
			out = append(out, Change{Op: kind, Edge: &copied})
		} else if before.edge(id) != nil {
			out = append(out, Change{Op: "edge.remove", ID: id})
		}
	}
	if a.canvas {
		out = append(out, Change{Op: "canvas.update", Canvas: &CanvasChange{Title: a.next.Title, Live: a.next.Live, Settings: a.next.Settings}})
	}
	return out
}

// commitLocked does what the batch's changes call for: the nodes removed
// go to their graves, the nodes added start. It returns what is done after
// the lock is let go.
func (a *applier) commitLocked() []func() {
	c := a.c
	var after []func()
	for _, r := range a.removed {
		id := r.node.ID
		st := c.states[id]
		delete(c.states, id)
		c.dropPendingLocked(id, "its node was removed")
		if st == nil {
			st = newState(StateStopped, "")
		}
		g := &grave{node: r.node, edges: r.edges, state: st, keep: r.keep}
		if old := c.graves[id]; old != nil {
			old.timer.Stop()
		}
		c.graves[id] = g
		g.timer = time.AfterFunc(r.grace, func() { c.bury(id, g) })
		inst, terminalID := st.inst, r.node.Runtime.Terminal
		after = append(after, func() {
			if src, ok := inst.(sourceInstance); ok {
				src.stop()
			}
			if terminalID != "" {
				_ = c.e.o.Host.Terminals().CloseAfter(terminalID, r.grace)
			}
		})
	}
	for _, add := range a.added {
		id := add.id
		if g := add.restore; g != nil {
			if c.graves[id] == g {
				delete(c.graves, id)
				g.timer.Stop()
			}
			c.states[id] = g.state
			n := c.doc.node(id)
			st := g.state
			terminalID := n.Runtime.Terminal
			after = append(after, func() {
				if terminalID != "" {
					_ = c.e.o.Host.Terminals().Keep(terminalID)
				}
				if _, ok := st.inst.(sourceInstance); ok || st.inst == nil {
					c.attach(id, false)
				}
			})
			continue
		}
		n := c.doc.node(id)
		if n.Proposed {
			c.states[id] = newState(StatePaused, "made by an agent: waiting for your approval")
			c.noticeLocked("info", fmt.Sprintf("«%s» proposes a new node, «%s»: approve it or remove it.", a.creatorTitle(n), n.Title), id, "")
			continue
		}
		c.states[id] = newState(StateStarting, "")
		prompt, actor := add.prompt, a.actor
		after = append(after, func() {
			c.attach(id, true)
			if prompt != "" {
				_, _ = c.send(actor.name(), id, sendRequest{Text: prompt})
			}
		})
	}
	for _, id := range a.approved {
		c.setStatusLocked(id, StateStarting, "")
		after = append(after, func() { c.attach(id, true) })
	}
	for _, id := range a.retitled {
		if n := c.doc.node(id); n != nil && n.Kind == KindAgent && n.Runtime.Session != "" {
			session, title := n.Runtime.Session, n.Title
			after = append(after, func() {
				_ = setSessionTitle(c.ws.SessionDir, session, title, c.id, id)
			})
		}
	}
	if a.liveFrom != nil {
		live := c.doc.Live
		after = append(after, func() { c.setLive(live) })
	}
	c.kickDispatch()
	return after
}

// creatorTitle names who made a node.
func (a *applier) creatorTitle(n *Node) string {
	if creator := a.next.node(n.CreatedBy); creator != nil {
		return creator.Title
	}
	return "an agent"
}
