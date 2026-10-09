// Package canvas is the engine of the browser cockpit's canvas: a board of
// nodes — terminals, agents, sources of events, notes — wired output to
// input. A canvas is a document of its workspace (.harness/canvases), which
// the engine keeps, changes by batches of operations (ops.go), and streams
// to the pages that show it (hub.go); what runs for its nodes — shells,
// agents' sessions, sources' processes — the engine starts, follows and
// stops, and messages go along its edges from one node's output to
// another's input (router.go).
package canvas

import (
	"crypto/rand"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// Version is the version of the documents this engine writes; it opens
// newer ones read-only.
const Version = 1

// Kinds of nodes.
const (
	KindTerminal = "terminal"
	KindAgent    = "agent"
	KindSource   = "source"
	KindNote     = "note"
)

// Scopes of what a node's program may do with the canvas, each with what
// the one before allows.
const (
	ScopeNone    = "none"
	ScopeObserve = "observe" // view, read, wait
	ScopeTalk    = "talk"    // + send to nodes, emit its own output
	ScopeBuild   = "build"   // + create nodes, remove its own, connect, move
	ScopeAdmin   = "admin"   // + remove any node, change the canvas's settings
	// scopeSource is a source's process: it emits its node's events.
	scopeSource = "source"
)

var scopeRank = map[string]int{ScopeNone: 0, ScopeObserve: 1, ScopeTalk: 2, ScopeBuild: 3, ScopeAdmin: 4}

// allows reports whether scope has what need asks.
func allows(scope, need string) bool {
	have, ok := scopeRank[scope]
	return ok && have >= scopeRank[need]
}

// Doc is a canvas as it is saved: its nodes and edges, and settings.
type Doc struct {
	Version   int       `json:"version"`
	ID        string    `json:"id"`
	Title     string    `json:"title"`
	CreatedAt time.Time `json:"created_at"`
	UpdatedAt time.Time `json:"updated_at"`
	// Rev grows with every batch of operations.
	Rev int64 `json:"rev"`
	// Live canvases run their sources and deliver their messages, the page
	// open or not; a paused one keeps its messages for later.
	Live     bool     `json:"live"`
	Settings Settings `json:"settings"`
	Nodes    []*Node  `json:"nodes"`
	Edges    []*Edge  `json:"edges"`
	// Unknown keeps what a newer version wrote, to write it back.
	Unknown jsontext.Value `json:",embed"`
}

// Settings bound what the canvas's agents may do, and how much goes along
// its edges.
type Settings struct {
	Autonomy Autonomy `json:"autonomy"`
	Routing  Routing  `json:"routing"`
}

// Autonomy bounds the nodes agents make.
type Autonomy struct {
	// Spawn is allow, ask (a node an agent makes waits for the user's
	// approval) or deny.
	Spawn           string `json:"spawn"`
	MaxNodes        int    `json:"max_nodes"`
	MaxTerminals    int    `json:"max_terminals"`
	SpawnsPerMinute int    `json:"spawns_per_minute"`
	// MaxDepth bounds how many agents deep a node an agent makes can be.
	MaxDepth int `json:"max_depth"`
}

// Routing bounds the messages.
type Routing struct {
	// MaxHops bounds a chain of messages, each caused by the one before:
	// what stops two agents answering each other forever.
	MaxHops             int `json:"max_hops"`
	EdgeRatePerMinute   int `json:"edge_rate_per_minute"`
	CanvasRatePerMinute int `json:"canvas_rate_per_minute"`
}

// DefaultSettings are a new canvas's.
func DefaultSettings() Settings {
	return Settings{
		Autonomy: Autonomy{Spawn: "allow", MaxNodes: 48, MaxTerminals: 16, SpawnsPerMinute: 6, MaxDepth: 3},
		Routing:  Routing{MaxHops: 32, EdgeRatePerMinute: 30, CanvasRatePerMinute: 120},
	}
}

// fill gives the settings left out their defaults.
func (s *Settings) fill() {
	d := DefaultSettings()
	if s.Autonomy.Spawn == "" {
		s.Autonomy.Spawn = d.Autonomy.Spawn
	}
	fillInt(&s.Autonomy.MaxNodes, d.Autonomy.MaxNodes)
	fillInt(&s.Autonomy.MaxTerminals, d.Autonomy.MaxTerminals)
	fillInt(&s.Autonomy.SpawnsPerMinute, d.Autonomy.SpawnsPerMinute)
	fillInt(&s.Autonomy.MaxDepth, d.Autonomy.MaxDepth)
	fillInt(&s.Routing.MaxHops, d.Routing.MaxHops)
	fillInt(&s.Routing.EdgeRatePerMinute, d.Routing.EdgeRatePerMinute)
	fillInt(&s.Routing.CanvasRatePerMinute, d.Routing.CanvasRatePerMinute)
}

func fillInt(value *int, fallback int) {
	if *value <= 0 {
		*value = fallback
	}
}

// validate checks settings a user or an agent gives.
func (s Settings) validate() error {
	switch s.Autonomy.Spawn {
	case "allow", "ask", "deny":
	default:
		return fmt.Errorf("autonomy.spawn must be allow, ask or deny")
	}
	for name, value := range map[string]int{
		"autonomy.max_nodes": s.Autonomy.MaxNodes, "autonomy.max_terminals": s.Autonomy.MaxTerminals,
		"autonomy.spawns_per_minute": s.Autonomy.SpawnsPerMinute, "autonomy.max_depth": s.Autonomy.MaxDepth,
		"routing.max_hops": s.Routing.MaxHops, "routing.edge_rate_per_minute": s.Routing.EdgeRatePerMinute,
		"routing.canvas_rate_per_minute": s.Routing.CanvasRatePerMinute,
	} {
		if value < 1 || value > 10000 {
			return fmt.Errorf("%s must be between 1 and 10000", name)
		}
	}
	if s.Autonomy.MaxTerminals > 64 {
		return fmt.Errorf("autonomy.max_terminals must be 64 or fewer")
	}
	return nil
}

// Node is a node of the canvas.
type Node struct {
	ID   string `json:"id"`
	Kind string `json:"kind"`
	// Preset is the harness a terminal runs (shell, command, claude-code…),
	// the source a source node is, or foreman for the canvas's own agent;
	// Plugin is the plugin that declares it, when not the canvas's own.
	Preset string `json:"preset,omitzero"`
	Plugin string `json:"plugin,omitzero"`
	Title  string `json:"title"`
	// Where it is on the board, in world pixels, and which is on top.
	X int `json:"x"`
	Y int `json:"y"`
	W int `json:"w"`
	H int `json:"h"`
	Z int `json:"z,omitzero"`
	// Config is the node's own settings, as its kind or preset takes them.
	Config jsontext.Value `json:"config,omitzero"`
	// Access is what its program may do with the canvas.
	Access string `json:"access,omitzero"`
	// CreatedBy is "user", or the node whose agent made it; Depth counts
	// the agents that made it, one after the other.
	CreatedBy string `json:"created_by,omitzero"`
	Depth     int    `json:"depth,omitzero"`
	// Proposed is set for a node an agent made that waits for the user's
	// approval: nothing runs for it until then.
	Proposed bool `json:"proposed,omitzero"`
	// Runtime is what runs for it; only the engine sets it.
	Runtime Runtime        `json:"runtime,omitzero"`
	Unknown jsontext.Value `json:",embed"`
}

// Runtime is what runs for a node.
type Runtime struct {
	// Terminal is the shell of a terminal node.
	Terminal string `json:"terminal,omitzero"`
	// Session is the kou session of an agent node.
	Session string `json:"session,omitzero"`
	// AgentSession is the session of the program a terminal runs: Claude
	// Code's or Codex's, to resume it.
	AgentSession string `json:"agent_session,omitzero"`
	// Worktree and Branch are where the node works, apart from the
	// checkout.
	Worktree string `json:"worktree,omitzero"`
	Branch   string `json:"branch,omitzero"`
	// Epoch counts the node's tokens: a new one makes the old ones void.
	Epoch int `json:"epoch,omitzero"`
	// Briefed is set once the node's program was told of the canvas.
	Briefed bool `json:"briefed,omitzero"`
}

// Edge carries messages from a node's output port to another's input.
type Edge struct {
	ID   string `json:"id"`
	From Port   `json:"from"`
	To   Port   `json:"to"`
	// Template makes the message's text from the output; {{text}} when
	// empty.
	Template string `json:"template,omitzero"`
	// Mode is auto (the default), approve (each message waits for the
	// user) or off.
	Mode string `json:"mode,omitzero"`
	// Deliver is queue (the default: the message waits for the node to be
	// idle) or now.
	Deliver string `json:"deliver,omitzero"`
	// Header puts where the message comes from before its text; by
	// default for agents, not for shells.
	Header  *bool          `json:"header,omitzero"`
	Unknown jsontext.Value `json:",embed"`
}

// Port is a node's port.
type Port struct {
	Node string `json:"node"`
	Port string `json:"port"`
}

func (p Port) String() string { return p.Node + ":" + p.Port }

// Edge modes and deliveries.
const (
	ModeAuto    = "auto"
	ModeApprove = "approve"
	ModeOff     = "off"

	DeliverQueue = "queue"
	DeliverNow   = "now"
)

func (e *Edge) mode() string {
	if e.Mode == "" {
		return ModeAuto
	}
	return e.Mode
}

func (e *Edge) deliver() string {
	if e.Deliver == "" {
		return DeliverQueue
	}
	return e.Deliver
}

// Rect is a node's place.
type Rect struct {
	X int `json:"x"`
	Y int `json:"y"`
	W int `json:"w"`
	H int `json:"h"`
}

func (n *Node) rect() Rect { return Rect{n.X, n.Y, n.W, n.H} }

// Sizes bound a node's.
const (
	minWidth, minHeight = 160, 90
	maxSize             = 6000
	maxCoordinate       = 1_000_000
)

// defaultSize is the size a node of a kind and preset starts with.
func defaultSize(kind, preset string) (int, int) {
	switch kind {
	case KindTerminal:
		if preset != "" && preset != "shell" && preset != "command" {
			return 760, 520
		}
		return 760, 460
	case KindAgent:
		return 440, 560
	case KindSource:
		return 320, 220
	}
	return 280, 180
}

var (
	canvasIDPattern = regexp.MustCompile(`^[A-Za-z0-9-]{1,128}$`)
	nodeIDPattern   = regexp.MustCompile(`^n_[a-z0-9]{4,16}$`)
	edgeIDPattern   = regexp.MustCompile(`^e_[a-z0-9]{4,16}$`)
	presetPattern   = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)
	pluginPattern   = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,63}$`)
	portPattern     = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)
)

// ValidID reports whether id can name a canvas: the UUID of the session it
// was made from, usually.
func ValidID(id string) bool { return canvasIDPattern.MatchString(id) }

const idAlphabet = "abcdefghijklmnopqrstuvwxyz0123456789"

// newID makes an ID: prefix and n random letters and digits.
func newID(prefix string, n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	for i := range b {
		b[i] = idAlphabet[int(b[i])%len(idAlphabet)]
	}
	return prefix + string(b)
}

// cleanTitle is a title as it is kept: one line, without control
// characters, at most 120 characters.
func cleanTitle(title string) string {
	title = strings.Map(func(r rune) rune {
		switch {
		case r == '\n' || r == '\r' || r == '\t':
			return ' '
		case r < 0x20 || r == 0x7f || r >= 0x80 && r <= 0x9f:
			return -1
		}
		return r
	}, title)
	title = strings.Join(strings.Fields(title), " ")
	if runes := []rune(title); len(runes) > 120 {
		title = string(runes[:119]) + "…"
	}
	return title
}

// clone copies a document, deeply.
func (d *Doc) clone() *Doc {
	data, err := json.Marshal(d)
	if err != nil {
		panic(fmt.Sprintf("canvas: encode a document: %v", err))
	}
	var c Doc
	if err := json.Unmarshal(data, &c); err != nil {
		panic(fmt.Sprintf("canvas: decode a document: %v", err))
	}
	if c.Nodes == nil {
		c.Nodes = []*Node{}
	}
	if c.Edges == nil {
		c.Edges = []*Edge{}
	}
	return &c
}

func (d *Doc) node(id string) *Node {
	for _, n := range d.Nodes {
		if n.ID == id {
			return n
		}
	}
	return nil
}

func (d *Doc) edge(id string) *Edge {
	for _, e := range d.Edges {
		if e.ID == id {
			return e
		}
	}
	return nil
}

// normalize makes a document read from disk whole: settings filled, nodes
// of known kinds with IDs of their own, edges between nodes that are
// there.
func (d *Doc) normalize() {
	d.Settings.fill()
	seen := map[string]bool{}
	nodes := d.Nodes[:0]
	for _, n := range d.Nodes {
		if n == nil || !nodeIDPattern.MatchString(n.ID) || seen[n.ID] {
			continue
		}
		switch n.Kind {
		case KindTerminal, KindAgent, KindSource, KindNote:
		default:
			continue
		}
		seen[n.ID] = true
		w, h := defaultSize(n.Kind, n.Preset)
		if n.W < minWidth || n.W > maxSize {
			n.W = w
		}
		if n.H < minHeight || n.H > maxSize {
			n.H = h
		}
		nodes = append(nodes, n)
	}
	d.Nodes = nodes
	edgeSeen := map[string]bool{}
	edges := d.Edges[:0]
	for _, e := range d.Edges {
		if e == nil || !edgeIDPattern.MatchString(e.ID) || edgeSeen[e.ID] || !seen[e.From.Node] || !seen[e.To.Node] {
			continue
		}
		edgeSeen[e.ID] = true
		edges = append(edges, e)
	}
	d.Edges = edges
	if d.Nodes == nil {
		d.Nodes = []*Node{}
	}
	if d.Edges == nil {
		d.Edges = []*Edge{}
	}
}

// configString reads a string of a node's configuration.
func (n *Node) configString(key string) string {
	value, _ := n.configValue(key).(string)
	return value
}

// configValue reads a value of a node's configuration.
func (n *Node) configValue(key string) any {
	if len(n.Config) == 0 {
		return nil
	}
	var config map[string]any
	if json.Unmarshal(n.Config, &config) != nil {
		return nil
	}
	return config[key]
}

// configMap is the node's configuration as a map.
func (n *Node) configMap() map[string]any {
	config := map[string]any{}
	if len(n.Config) != 0 {
		_ = json.Unmarshal(n.Config, &config)
	}
	return config
}

// configBool reads a boolean of a node's configuration, fallback when it
// is not set.
func (n *Node) configBool(key string, fallback bool) bool {
	if value, ok := n.configValue(key).(bool); ok {
		return value
	}
	return fallback
}

// harness reports whether a terminal node runs an agent's harness rather
// than a shell or a command.
func (n *Node) harness() bool {
	return n.Kind == KindTerminal && n.Preset != "" && n.Preset != "shell" && n.Preset != "command"
}

// agentish reports whether the node's program is an agent: messages to it
// say where they come from, and two of them answering each other is a
// loop to watch.
func (n *Node) agentish() bool { return n.Kind == KindAgent || n.harness() }
