package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Presets are data: a plugin's canvas section declares the harnesses its
// terminals run and the sources of events it has (plugin.CanvasHarness,
// plugin.CanvasSource) — the canvas's own plugin, canvas, among them. The
// engine reads them from the active plugins of the canvas's workspace.

// canvasPlugin is the canvas's own plugin: its presets name no plugin.
const canvasPlugin = "canvas"

// catalogFor is how long the presets read are used before they are read
// again.
const catalogFor = 3 * time.Second

type harnessDef struct {
	Plugin  string // "" for the canvas's own
	Harness plugin.CanvasHarness
	Owner   plugin.Plugin
}

type sourceDef struct {
	Plugin string
	Source plugin.CanvasSource
	Owner  plugin.Plugin
}

type presetCache struct {
	at        time.Time
	harnesses []harnessDef
	sources   []sourceDef
}

// The presets the engine has without its plugin: a shell, a command, and
// the sources it implements.
var (
	builtinHarnesses = []plugin.CanvasHarness{
		{ID: "shell", Title: "Shell", Icon: "▣", Launch: "shell", Status: []string{"osc133"}, Output: "osc133"},
		{ID: "command", Title: "Command…", Icon: "▣", Launch: "type", Status: []string{"osc133"}, Output: "osc133"},
	}
	builtinSources = []plugin.CanvasSource{
		{ID: "manual", Title: "Manual", Builtin: true, Outputs: []plugin.CanvasPort{{ID: "out", Title: "Fired"}}},
		{ID: "timer", Title: "Timer", Builtin: true, Outputs: []plugin.CanvasPort{{ID: "out", Title: "Tick"}}},
		{ID: "files", Title: "Files", Builtin: true, Outputs: []plugin.CanvasPort{{ID: "out", Title: "Changed"}}},
		{ID: "webhook", Title: "Webhook", Builtin: true, Outputs: []plugin.CanvasPort{{ID: "out", Title: "Received"}}},
	}
)

func presetName(pluginName, id string) string {
	if pluginName == "" {
		return id
	}
	return pluginName + "/" + id
}

// catalog is what the active plugins of a workspace give the canvas.
func (e *Engine) catalog(ws Workspace) presetCache {
	e.mu.Lock()
	cached, ok := e.presets[ws.ID]
	e.mu.Unlock()
	if ok && time.Since(cached.at) < catalogFor {
		return cached
	}
	p := presetCache{at: time.Now()}
	for _, current := range e.o.Host.Plugins(ws).Plugins {
		if !current.Active || current.Canvas == nil {
			continue
		}
		name := current.Name
		if name == canvasPlugin {
			name = ""
		}
		for _, h := range current.Canvas.Harnesses {
			p.harnesses = append(p.harnesses, harnessDef{Plugin: name, Harness: h, Owner: current})
		}
		for _, s := range current.Canvas.Sources {
			if s.Builtin && name != "" {
				continue // only the canvas's own sources are the engine's
			}
			p.sources = append(p.sources, sourceDef{Plugin: name, Source: s, Owner: current})
		}
	}
	for _, h := range builtinHarnesses {
		if _, ok := p.harness("", h.ID); !ok {
			p.harnesses = append(p.harnesses, harnessDef{Harness: h})
		}
	}
	for _, s := range builtinSources {
		if _, ok := p.source("", s.ID); !ok {
			p.sources = append(p.sources, sourceDef{Source: s})
		}
	}
	e.mu.Lock()
	e.presets[ws.ID] = p
	e.mu.Unlock()
	return p
}

func (p presetCache) harness(pluginName, id string) (harnessDef, bool) {
	for _, h := range p.harnesses {
		if h.Plugin == pluginName && h.Harness.ID == id {
			return h, true
		}
	}
	return harnessDef{}, false
}

func (p presetCache) source(pluginName, id string) (sourceDef, bool) {
	for _, s := range p.sources {
		if s.Plugin == pluginName && s.Source.ID == id {
			return s, true
		}
	}
	return sourceDef{}, false
}

// Launch is how kou-canvas launch starts a terminal node's harness: the
// program's arguments, what it adds to its environment, and where it runs.
type Launch struct {
	Argv  []string `json:"argv"`
	Env   []string `json:"env"`
	Dir   string   `json:"dir"`
	Title string   `json:"title"`
}

// launchSpec makes a harness node's launch: its files written, its
// arguments filled in. resume takes the harness's own session up again.
func (c *Canvas) launchSpec(id string, resume bool) (Launch, error) {
	n := c.node(id)
	if n == nil {
		return Launch{}, errNotFound("no such node")
	}
	if !n.harness() {
		return Launch{}, errInvalid("«" + n.Title + "» runs no harness")
	}
	def, ok := c.e.catalog(c.ws).harness(n.Plugin, n.Preset)
	if !ok {
		return Launch{}, errInvalid(fmt.Sprintf("no preset %q: is its plugin on?", presetName(n.Plugin, n.Preset)))
	}
	h := def.Harness
	if len(h.Command) == 0 {
		return Launch{}, errInvalid("the preset " + h.ID + " runs no program")
	}
	agentSession := n.Runtime.AgentSession
	resuming := resume && agentSession != "" && len(h.Resume) > 0
	if !resuming && slices.ContainsFunc(h.SessionArg, func(arg string) bool { return strings.Contains(arg, "runtime.agent_session") }) {
		// The harness takes the session it is told to start: a new one,
		// whose ID resumes it later.
		agentSession = uuid.New().String()
		c.setRuntime(id, func(r *Runtime) { r.AgentSession = agentSession })
		n.Runtime.AgentSession = agentSession
	}
	dir := c.e.launchDir(c, id)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return Launch{}, err
	}
	config := n.configMap()
	if properties, ok := h.Config["properties"].(map[string]any); ok {
		for key, value := range properties {
			if property, ok := value.(map[string]any); ok {
				if fallback, ok := property["default"]; ok {
					if _, set := config[key]; !set {
						config[key] = fallback
					}
				}
			}
		}
	}
	brief := c.brief(id)
	files := map[string]string{}
	lookup := func(name string) (string, bool) {
		switch name {
		case "brief":
			return brief, true
		case "node.title":
			return n.Title, true
		case "node.id":
			return n.ID, true
		case "runtime.agent_session":
			return agentSession, agentSession != ""
		case "canvas.id":
			return c.id, true
		}
		if file, ok := strings.CutPrefix(name, "files."); ok {
			path, found := files[file]
			return path, found
		}
		if key, ok := strings.CutPrefix(name, "config."); ok {
			value, found := config[key]
			if !found || value == nil {
				return "", false
			}
			return textOf(value), true
		}
		return "", false
	}
	for _, name := range sortedKeys(h.Files) {
		value := h.Files[name]
		var path string
		var data []byte
		if value.Kind() == '"' {
			var text string
			if err := json.Unmarshal(value, &text); err != nil {
				return Launch{}, err
			}
			path, data = filepath.Join(dir, name+".txt"), []byte(expand(text, lookup))
		} else {
			path = filepath.Join(dir, name+".json")
			formatted := jsontext.Value(append([]byte(nil), value...))
			_ = formatted.Indent()
			data = formatted
		}
		if err := writeAtomic(path, data, 0o600); err != nil {
			return Launch{}, err
		}
		files[name] = path
	}
	argv := append([]string(nil), h.Command...)
	if resuming {
		argv = append(argv, expandArgs(h.Resume, lookup)...)
	} else {
		argv = append(argv, expandArgs(h.SessionArg, lookup)...)
	}
	argv = append(argv, expandArgs(h.Args, lookup)...)
	env := c.e.env(c, id, terminalScope(n), n.Runtime.Epoch)
	for _, name := range sortedKeys(h.Env) {
		env = append(env, name+"="+expand(h.Env[name], lookup))
	}
	workdir, err := c.nodeDir(n)
	if err != nil {
		return Launch{}, err
	}
	if !n.Runtime.Briefed {
		c.setRuntime(id, func(r *Runtime) { r.Briefed = true })
	}
	return Launch{Argv: argv, Env: env, Dir: workdir, Title: h.Title}, nil
}

// expandArgs fills arguments in; an argument whose values are all missing
// goes, and with it the flag before it.
func expandArgs(args []string, lookup func(string) (string, bool)) []string {
	var out []string
	for i, arg := range args {
		value := expand(arg, lookup)
		if value == "" && placeholders(arg) {
			if i > 0 && len(out) > 0 && strings.HasPrefix(args[i-1], "-") && !placeholders(args[i-1]) && out[len(out)-1] == args[i-1] {
				out = out[:len(out)-1]
			}
			continue
		}
		out = append(out, value)
	}
	return out
}

// nodeDir is where a node works: its worktree, its cwd, or the workspace.
func (c *Canvas) nodeDir(n *Node) (string, error) {
	if n.Runtime.Worktree != "" {
		return n.Runtime.Worktree, nil
	}
	if cwd := strings.TrimSpace(n.configString("cwd")); cwd != "" {
		if !filepath.IsAbs(cwd) {
			cwd = filepath.Join(c.ws.Path, cwd)
		}
		return filepath.Clean(cwd), nil
	}
	return c.ws.Path, nil
}

// maxBrief bounds what a node is told of the canvas; briefNodes, what of
// it names the other nodes, which are named until it is spent.
const (
	maxBrief   = 1536
	briefNodes = 480
)

// brief is what a node's agent is told of the canvas it is on: who it is,
// the nodes it can message, where its answers go, and how it messages a
// node — in one step, as it knows their IDs and titles.
func (c *Canvas) brief(id string) string {
	catalog := c.e.catalog(c.ws)
	c.mu.Lock()
	defer c.mu.Unlock()
	n := c.doc.node(id)
	if n == nil {
		return ""
	}
	kind := "kou agent"
	if n.Kind == KindTerminal {
		kind = n.Preset
		if def, ok := catalog.harness(n.Plugin, n.Preset); ok {
			kind = def.Harness.Title
		}
	}
	var b strings.Builder
	fmt.Fprintf(&b, briefStart+"%s» (%s, %s) on the kou-conveyor canvas «%s».\n", n.Title, n.ID, kind, c.doc.Title)
	talks := allows(n.Access, ScopeTalk)
	if others := c.briefNodesLocked(catalog, id); others != "" {
		if talks {
			b.WriteString("Nodes you can message: " + others + ".\n")
		} else {
			b.WriteString("The other nodes: " + others + ".\n")
		}
	}
	b.WriteString("Messages from nodes arrive as prompts that start with \"[canvas] from «Title» (id):\"; the answers to what you sent, with \"[canvas] reply from «Title» (id):\".\n")
	var targets []string
	for _, e := range c.doc.Edges {
		if e.From.Node == id && e.From.Port == "out" && e.mode() != ModeOff {
			if target := c.doc.node(e.To.Node); target != nil {
				targets = append(targets, fmt.Sprintf("«%s» (%s)", target.Title, target.ID))
			}
		}
	}
	along := "nothing is wired to your output yet"
	if len(targets) > 0 {
		along = "your edges take it to " + strings.Join(targets, ", ")
	}
	switch outputOf(n) {
	case OutputAll:
		b.WriteString("Every answer you end a turn with goes out — " + along + " —, the user's too; a node that messaged you gets its answer back.\n")
	case OutputExplicit:
		b.WriteString("Your answers go back to the nodes that message you, and no further: what you put out with CanvasEmit goes along your edges (" + along + ").\n")
	default:
		b.WriteString("The answer you end a turn with goes where the turn's message came from: a node's message is answered back to that node, and on along your edges (" + along + "); the user's prompts (those without a [canvas] line) and replies are answered to the user alone — that answer goes to no node.\n")
	}
	if talks {
		b.WriteString("To message a node, call the kou-canvas tool CanvasSend once — {\"node\":\"<id or title>\",\"text\":\"…\",\"wait\":true} —: with wait its answer is the call's result; without, it comes to you later as a reply. In a shell: kou-canvas send <node> \"text\" --wait.\n")
	}
	if n.Runtime.Worktree != "" {
		fmt.Fprintf(&b, "You work in the git worktree %s on branch %s; commit there.\n", n.Runtime.Worktree, n.Runtime.Branch)
	}
	if allows(n.Access, ScopeObserve) {
		b.WriteString("The canvas changes while you work: CanvasView (kou-canvas view) shows its nodes and edges as they are now — look before you create or connect anything.\n")
	}
	switch n.Access {
	case ScopeAdmin:
		b.WriteString("Your access: admin — you can create, connect, message and remove any node, and change the canvas.\n")
	case ScopeBuild:
		b.WriteString("Your access: build — you can create nodes, connect and message them, and remove the ones you made.\n")
	case ScopeTalk:
		b.WriteString("Your access: talk — you can read and message nodes, but not create or delete them.\n")
	case ScopeObserve:
		b.WriteString("Your access: observe — you can look at the canvas and read nodes.\n")
	}
	if n.Preset == "foreman" {
		b.WriteString("You are the canvas's foreman: you build and run the canvas at the user's request — make the nodes the work needs, wire them, start them, and report back briefly.\n")
	}
	if role := strings.TrimSpace(n.configString("instructions")); role != "" {
		b.WriteString("Your role: " + role + "\n")
	}
	brief := strings.TrimRight(b.String(), "\n")
	// No blank line inside: the first of a prompt ends its brief.
	for strings.Contains(brief, "\n\n") {
		brief = strings.ReplaceAll(brief, "\n\n", "\n")
	}
	if len(brief) > maxBrief {
		brief = cut(brief, maxBrief-len("…")) + "…"
	}
	return brief
}

// briefNodesLocked names the nodes a node can message — the others that
// take input —, as long as what names them is under briefNodes; a count
// stands for the rest.
func (c *Canvas) briefNodesLocked(catalog presetCache, id string) string {
	var named []string
	size, more := 0, 0
	for _, other := range c.doc.Nodes {
		if other.ID == id || other.Proposed || other.Kind != KindTerminal && other.Kind != KindAgent {
			continue
		}
		entry := fmt.Sprintf("«%s» (%s, %s)", other.Title, other.ID, c.briefKindLocked(catalog, other))
		if more > 0 || len(named) > 0 && size+len(entry) > briefNodes {
			more++
			continue
		}
		named = append(named, entry)
		size += len(entry) + 2
	}
	out := strings.Join(named, ", ")
	if more > 0 {
		out += fmt.Sprintf(", and %d more, which CanvasView lists", more)
	}
	return out
}

// briefKindLocked says what a node is, as the others are told: a
// terminal says the agent it runs.
func (c *Canvas) briefKindLocked(catalog presetCache, n *Node) string {
	if n.Kind != KindTerminal {
		return kindName(n)
	}
	if st := c.states[n.ID]; st != nil && st.status.Agent != nil && st.status.Agent.Detected {
		return st.status.Agent.Title + " in a shell"
	}
	if n.harness() {
		if def, ok := catalog.harness(n.Plugin, n.Preset); ok && def.Harness.Title != "" {
			return def.Harness.Title
		}
	}
	return kindName(n)
}

// kindName names what a node is, for people.
func kindName(n *Node) string {
	switch n.Kind {
	case KindAgent:
		if n.Preset == "foreman" {
			return "foreman"
		}
		return "kou agent"
	case KindTerminal:
		return n.Preset
	case KindSource:
		return "source " + presetName(n.Plugin, n.Preset)
	}
	return n.Kind
}

// jsonOf encodes a value, its maps' keys sorted; nil when it cannot.
func jsonOf(v any) jsontext.Value {
	data, err := json.Marshal(v, json.Deterministic(true))
	if err != nil {
		return nil
	}
	return data
}

// setSessionTitle titles an agent's session as its node.
func setSessionTitle(dir, session, title, canvas, node string) error {
	return cockpit.SetCanvasMeta(dir, session, title, canvas, node)
}
