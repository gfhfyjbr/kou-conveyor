package canvascli

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"math"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// Tool is a tool of the canvas: one model for the agents' plugin
// (canvas-agent), the MCP server and the command line.
type Tool struct {
	Name        string
	Description string
	// Schema is the JSON schema of its arguments, an object.
	Schema string
	run    func(ctx context.Context, c *client, args jsontext.Value) (result, error)
}

// result is what a tool answers: text for the agent, and what the server
// answered, which --json prints.
type result struct {
	text string
	raw  []byte
}

// Tools are the canvas's tools, in the order they are listed.
func Tools() []Tool { return tools }

func toolNamed(name string) (Tool, bool) {
	for _, t := range tools {
		if t.Name == name {
			return t, true
		}
	}
	return Tool{}, false
}

// call runs a tool with its arguments, a JSON object.
func (t Tool) call(ctx context.Context, c *client, input []byte) (result, error) {
	args := jsontext.Value(bytes.TrimSpace(input))
	if len(args) == 0 {
		args = jsontext.Value("{}")
	}
	if !args.IsValid() || args.Kind() != '{' {
		return result{}, fmt.Errorf("%s takes its arguments as a JSON object", t.Name)
	}
	return t.run(ctx, c, args)
}

// decodeArgs reads a tool's arguments.
func decodeArgs(name string, args jsontext.Value, v any) error {
	if err := json.Unmarshal(args, v); err != nil {
		return fmt.Errorf("the arguments of %s: %v", name, err)
	}
	return nil
}

var tools = []Tool{
	{
		Name: "CanvasView",
		Description: "Show the canvas you are on: every node (its ID, kind, title, status, place and size, ports, worktree), every connection between them, " +
			"and free places beside your node. Look before you create, connect or remove anything: the canvas changes while you work. " +
			"To message a node you know the title of, CanvasSend needs no look first.",
		Schema: `{"type":"object","properties":{` +
			`"detail":{"type":"string","enum":["summary","full"],"description":"full adds the connections' templates and the notes' texts."}}}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a viewArgs
			if err := decodeArgs("CanvasView", args, &a); err != nil {
				return result{}, err
			}
			return c.view(ctx, a)
		},
	},
	{
		Name: "CanvasSpawn",
		Description: "Create a node on the canvas — a terminal running a coding agent or a command, a kou agent, a note — beside your node unless you say where; " +
			"optionally wire it to other nodes and give it its first prompt. Give each coding agent that writes code a worktree of its own. " +
			"It answers the node's ID and place.",
		Schema: `{"type":"object","properties":{` +
			`"preset":{"type":"string","description":"What the node runs: shell, command (runs command), claude-code, codex, opencode, agent (a kou agent like you), note, or a plugin's preset as plugin/preset."},` +
			`"title":{"type":"string","description":"Its title, which the other nodes see: short and telling."},` +
			`"command":{"type":"string","description":"For a command node: the command line it runs."},` +
			`"cwd":{"type":"string","description":"The directory a terminal starts in, relative to the workspace or to its worktree."},` +
			`"worktree":{"type":"string","description":"Work in a git worktree of its own, made when missing: its name, its branch kou/<name>. One writer per worktree. A kou agent's worktree is named after its session, whatever the name."},` +
			`"base":{"type":"string","description":"The branch or commit a new worktree starts from; HEAD by default."},` +
			`"prompt":{"type":"string","description":"Its first prompt, sent once it runs; for a note, its text."},` +
			`"config":{"type":"object","description":"More of its configuration, as its preset defines it: model, effort, instructions for an agent; permission_mode for claude-code."},` +
			`"access":{"type":"string","enum":["none","observe","talk","build"],"description":"What its program may do on the canvas: talk by default for what an agent makes; build lets it make nodes too."},` +
			`"near":{"type":"string","description":"The node to place it beside: yours by default."},` +
			`"side":{"type":"string","enum":["right","below","left","above"],"description":"Which side of near: the first free one by default."},` +
			`"x":{"type":"integer","description":"Where it goes, instead of beside near: its left edge."},` +
			`"y":{"type":"integer","description":"Its top edge."},` +
			`"w":{"type":"integer","description":"Its width; its preset's by default."},` +
			`"h":{"type":"integer","description":"Its height; its preset's by default."},` +
			`"connect":{"type":"object","description":"Wire it at once: from's output to its input, its output to to's input. A node is its ID or self, with :port when not out or in.",` +
			`"properties":{"from":{"type":"string"},"to":{"type":"string"}}}` +
			`},"required":["preset"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a spawnArgs
			if err := decodeArgs("CanvasSpawn", args, &a); err != nil {
				return result{}, err
			}
			return c.spawn(ctx, a)
		},
	},
	{
		Name:        "CanvasRemove",
		Description: "Remove a node from the canvas: one you made, or any with admin access. Its program ends; its worktree stays, with its work.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node's ID."},` +
			`"keep_session":{"type":"boolean","description":"Keep a kou agent's session in the session list; it is deleted with the node otherwise."},` +
			`"keep_worktree":{"type":"boolean","description":"Worktrees are always kept: only the user removes them."}` +
			`},"required":["node"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a removeArgs
			if err := decodeArgs("CanvasRemove", args, &a); err != nil {
				return result{}, err
			}
			return c.remove(ctx, a)
		},
	},
	{
		Name: "CanvasConnect",
		Description: "Connect a node's output to another's input: what the first puts out — a command's output, an agent's answer at the end of its turn — " +
			"goes to the second as a message.",
		Schema: `{"type":"object","properties":{` +
			`"from":{"type":"string","description":"The output: a node's ID or self, with :port when not out."},` +
			`"to":{"type":"string","description":"The input: a node's ID or self, with :port when not in."},` +
			`"template":{"type":"string","description":"What the message says: {{text}}, {{title}}, {{data.<path>}}, {{from.title}}, {{from.id}}; the output's text by default."},` +
			`"mode":{"type":"string","enum":["auto","approve","off"],"description":"approve holds each message until the user lets it go; a connection that closes a loop of agents starts so."},` +
			`"deliver":{"type":"string","enum":["queue","now"],"description":"queue (the default) waits until the target is idle; now gives it at once."}` +
			`},"required":["from","to"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a connectArgs
			if err := decodeArgs("CanvasConnect", args, &a); err != nil {
				return result{}, err
			}
			return c.connect(ctx, a)
		},
	},
	{
		Name:        "CanvasDisconnect",
		Description: "Remove a connection: by its ID, or by the output and input it joins.",
		Schema: `{"type":"object","properties":{` +
			`"edge":{"type":"string","description":"The connection's ID (e_…)."},` +
			`"from":{"type":"string","description":"Or the output it leaves: node or node:port."},` +
			`"to":{"type":"string","description":"And the input it reaches: node or node:port."}` +
			`}}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a disconnectArgs
			if err := decodeArgs("CanvasDisconnect", args, &a); err != nil {
				return result{}, err
			}
			return c.disconnect(ctx, a)
		},
	},
	{
		Name:        "CanvasMove",
		Description: "Move or resize a node, to keep the canvas tidy.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node's ID, or self."},` +
			`"x":{"type":"integer"},"y":{"type":"integer"},"w":{"type":"integer"},"h":{"type":"integer"}` +
			`},"required":["node"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a moveArgs
			if err := decodeArgs("CanvasMove", args, &a); err != nil {
				return result{}, err
			}
			return c.move(ctx, a)
		},
	},
	{
		Name: "CanvasSend",
		Description: "Message a node — a prompt for an agent, a command for a shell — named by its ID or its title: one call, no need to look it up first. " +
			"With wait, the call waits for the node's answer and returns it: an agent's answer at the end of its turn, a shell's command output. " +
			"Without wait, an agent's answer comes to you later as a prompt that starts with [canvas] reply from. " +
			"The text goes in once the node is idle, and Enter is pressed after it; keys are pressed as tmux send-keys names them.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node: its ID (n_…) or its title."},` +
			`"text":{"type":"string","description":"The message: the prompt, or what to type."},` +
			`"wait":{"type":"boolean","description":"Wait for the node's answer to this message, and return it. A node that waits for your answer cannot be waited for in turn."},` +
			`"timeout_s":{"type":"integer","description":"With wait: how long to wait at most, in seconds; 600 by default. An answer that comes later comes to you as a reply."},` +
			`"submit":{"type":"boolean","description":"Press Enter after the text: true by default."},` +
			`"keys":{"type":"array","items":{"type":"string"},"description":"Keys to press after the text, as tmux names them: Enter, Tab, Escape, BSpace, Up, Down, Left, Right, Home, End, C-c, M-x, F1…"},` +
			`"when":{"type":"string","enum":["idle","now"],"description":"idle (the default) waits until the node is idle; now gives it at once: an agent at work reads it after its current step."}` +
			`},"required":["node"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a sendArgs
			if err := decodeArgs("CanvasSend", args, &a); err != nil {
				return result{}, err
			}
			return c.send(ctx, a)
		},
	},
	{
		Name: "CanvasRead",
		Description: "Read a node: a terminal's latest lines or screen, the latest output of a node (a command's output, an agent's answer). " +
			"With wait, wait first — until it is idle, gives its next output, or ends — instead of polling. " +
			"To ask a node something and have its answer, CanvasSend with wait does it in one call.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node: its ID (n_…), its title, or self."},` +
			`"what":{"type":"string","enum":["screen","tail","output","answer"],"description":"tail (the default): a terminal's latest lines; screen: what it shows; output or answer: its latest output."},` +
			`"lines":{"type":"integer","description":"How many lines of tail: 80 by default."},` +
			`"wait":{"type":"string","enum":["idle","output","exit"],"description":"Wait until the node is idle, gives its next output, or ends, then read."},` +
			`"after":{"type":"integer","description":"With wait output: the count of outputs CanvasSend answered, so that an output given meanwhile counts."},` +
			`"timeout_s":{"type":"integer","description":"How long to wait at most, in seconds: 600 by default."}` +
			`},"required":["node"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a readArgs
			if err := decodeArgs("CanvasRead", args, &a); err != nil {
				return result{}, err
			}
			return c.read(ctx, a)
		},
	},
	{
		Name: "CanvasEmit",
		Description: "Put something out of your node now, as its output: it goes along your edges to the nodes your output is connected to. " +
			"No need to for your answer to a node's message: the answer you end that turn with goes back to it, and along your edges, by itself. " +
			"What you answer the user stays with the user, unless you emit it.",
		Schema: `{"type":"object","properties":{` +
			`"text":{"type":"string","description":"What to put out."},` +
			`"port":{"type":"string","description":"The output: out by default."},` +
			`"title":{"type":"string","description":"A title for it, which templates read as {{title}}."},` +
			`"data":{"type":"object","description":"Structured data besides the text, which templates read as {{data.<path>}}."}` +
			`},"required":["text"]}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a emitArgs
			if err := decodeArgs("CanvasEmit", args, &a); err != nil {
				return result{}, err
			}
			return c.emit(ctx, a)
		},
	},
}

// number is a number of a tool's arguments, which models write as 100 or
// 100.0 alike.
type number = *float64

func intOf(n number) *int {
	if n == nil {
		return nil
	}
	v := int(math.Round(*n))
	return &v
}

type viewArgs struct {
	Detail string `json:"detail"`
}

type spawnArgs struct {
	Preset   string         `json:"preset"`
	Kind     string         `json:"kind"`
	Title    string         `json:"title"`
	Command  string         `json:"command"`
	Cwd      string         `json:"cwd"`
	Worktree jsontext.Value `json:"worktree"`
	Base     string         `json:"base"`
	Prompt   string         `json:"prompt"`
	Config   jsontext.Value `json:"config"`
	Access   string         `json:"access"`
	Near     string         `json:"near"`
	Side     string         `json:"side"`
	X        number         `json:"x"`
	Y        number         `json:"y"`
	W        number         `json:"w"`
	H        number         `json:"h"`
	Connect  *struct {
		From string `json:"from"`
		To   string `json:"to"`
	} `json:"connect"`
}

type removeArgs struct {
	Node         string `json:"node"`
	KeepSession  bool   `json:"keep_session"`
	KeepWorktree bool   `json:"keep_worktree"`
}

type connectArgs struct {
	From     string `json:"from"`
	To       string `json:"to"`
	Template string `json:"template"`
	Mode     string `json:"mode"`
	Deliver  string `json:"deliver"`
}

type disconnectArgs struct {
	Edge string `json:"edge"`
	From string `json:"from"`
	To   string `json:"to"`
}

type moveArgs struct {
	Node string `json:"node"`
	X    number `json:"x"`
	Y    number `json:"y"`
	W    number `json:"w"`
	H    number `json:"h"`
}

// loose is a yes or no of a tool's arguments, which models write as true
// or "true" alike — and CanvasSend's wait as CanvasRead's, "idle".
type loose bool

func (b *loose) UnmarshalJSON(data []byte) error {
	switch strings.ToLower(strings.Trim(strings.TrimSpace(string(data)), `"`)) {
	case "true", "yes", "1", "idle", "output", "answer":
		*b = true
	case "false", "no", "0", "", "null":
		*b = false
	default:
		return errors.New("it is true or false")
	}
	return nil
}

type sendArgs struct {
	Node     string   `json:"node"`
	Text     string   `json:"text"`
	Wait     loose    `json:"wait"`
	TimeoutS number   `json:"timeout_s"`
	Submit   *bool    `json:"submit"`
	Keys     []string `json:"keys"`
	When     string   `json:"when"`
}

type readArgs struct {
	Node     string `json:"node"`
	What     string `json:"what"`
	Lines    number `json:"lines"`
	Wait     string `json:"wait"`
	After    number `json:"after"`
	TimeoutS number `json:"timeout_s"`
}

type emitArgs struct {
	Text  string         `json:"text"`
	Port  string         `json:"port"`
	Title string         `json:"title"`
	Data  jsontext.Value `json:"data"`
}

// What the server answers.

type viewCanvas struct {
	ID    string `json:"id"`
	Title string `json:"title"`
	Live  bool   `json:"live"`
	Rev   int64  `json:"rev"`
}

type viewNode struct {
	ID        string   `json:"id"`
	Kind      string   `json:"kind"`
	Preset    string   `json:"preset"`
	Plugin    string   `json:"plugin"`
	Title     string   `json:"title"`
	Status    string   `json:"status"`
	Detail    string   `json:"detail"`
	X         int      `json:"x"`
	Y         int      `json:"y"`
	W         int      `json:"w"`
	H         int      `json:"h"`
	Inputs    []string `json:"inputs"`
	Outputs   []string `json:"outputs"`
	Worktree  string   `json:"worktree"`
	Branch    string   `json:"branch"`
	Program   string   `json:"program"`
	Access    string   `json:"access"`
	CreatedBy string   `json:"created_by"`
	Proposed  bool     `json:"proposed"`
	Pending   int      `json:"pending"`
	Session   string   `json:"session"`
	Text      string   `json:"text"`
}

type viewEdge struct {
	ID       string `json:"id"`
	From     string `json:"from"`
	To       string `json:"to"`
	Mode     string `json:"mode"`
	Deliver  string `json:"deliver"`
	Template string `json:"template"`
}

type freePlace struct {
	Side string `json:"side"`
	X    int    `json:"x"`
	Y    int    `json:"y"`
}

type view struct {
	Canvas viewCanvas  `json:"canvas"`
	Me     *viewNode   `json:"me"`
	Scope  string      `json:"scope"`
	Nodes  []viewNode  `json:"nodes"`
	Edges  []viewEdge  `json:"edges"`
	Free   []freePlace `json:"free"`
}

type self struct {
	Canvas    viewCanvas `json:"canvas"`
	Node      viewNode   `json:"node"`
	Scope     string     `json:"scope"`
	Brief     string     `json:"brief"`
	Workspace string     `json:"workspace"`
	Page      string     `json:"page"`
}

type rect struct {
	X int `json:"x"`
	Y int `json:"y"`
	W int `json:"w"`
	H int `json:"h"`
}

type opsResult struct {
	Rev    int64             `json:"rev"`
	IDs    map[string]string `json:"ids"`
	Placed map[string]rect   `json:"placed"`
}

type status struct {
	State    string `json:"state"`
	Detail   string `json:"detail"`
	Activity string `json:"activity"`
}

type message struct {
	ID     string `json:"id"`
	State  string `json:"state"`
	Reason string `json:"reason"`
}

func (c *client) view(ctx context.Context, a viewArgs) (result, error) {
	q := url.Values{}
	switch a.Detail {
	case "", "summary":
	case "full":
		q.Set("detail", "full")
	default:
		return result{}, errors.New("detail is summary or full")
	}
	var v view
	raw, err := c.call(ctx, "GET", "/api/canvas/view", q, nil, &v)
	if err != nil {
		return result{}, err
	}
	return result{text: formatView(v, a.Detail == "full"), raw: raw}, nil
}

func (c *client) self(ctx context.Context) (result, error) {
	var s self
	raw, err := c.call(ctx, "GET", "/api/canvas/self", nil, nil, &s)
	if err != nil {
		return result{}, err
	}
	return result{text: formatSelf(s), raw: raw}, nil
}

// ops applies a batch of operations.
func (c *client) ops(ctx context.Context, ops ...map[string]any) (opsResult, []byte, error) {
	var res opsResult
	raw, err := c.call(ctx, "POST", "/api/canvas/ops", nil, map[string]any{"ops": ops}, &res)
	return res, raw, err
}

func (c *client) spawn(ctx context.Context, a spawnArgs) (result, error) {
	preset := strings.TrimSpace(a.Preset)
	kind := strings.TrimSpace(a.Kind)
	if preset == "" && kind == "" && strings.TrimSpace(a.Command) != "" {
		preset = "command"
	}
	if preset == "" && kind == "" {
		return result{}, errors.New("name the preset of the node: shell, command, claude-code, codex, opencode, agent, note, or plugin/preset")
	}
	switch preset {
	case "terminal":
		kind, preset = "terminal", "shell"
	case "agent", "kou":
		kind, preset = "agent", ""
	case "note":
		kind, preset = "note", ""
	}
	config := map[string]any{}
	if len(a.Config) != 0 && string(a.Config) != "null" {
		if err := json.Unmarshal(a.Config, &config); err != nil {
			return result{}, errors.New("config is a JSON object")
		}
	}
	if command := strings.TrimSpace(a.Command); command != "" {
		if preset == "" && kind == "" {
			preset = "command"
		}
		config["command"] = command
	}
	if cwd := strings.TrimSpace(a.Cwd); cwd != "" {
		config["cwd"] = cwd
	}
	agent := kind == "agent" || preset == "foreman"
	if name, wanted, err := worktreeOf(a.Worktree); err != nil {
		return result{}, err
	} else if wanted {
		if agent {
			config["sandbox"] = "worktree"
		} else {
			spec := map[string]any{}
			if name != "" {
				spec["name"] = name
			}
			if base := strings.TrimSpace(a.Base); base != "" {
				spec["base"] = base
			}
			config["worktree"] = spec
		}
	}
	node := map[string]any{}
	if kind != "" {
		node["kind"] = kind
	}
	if preset != "" {
		node["preset"] = preset
	}
	if title := strings.TrimSpace(a.Title); title != "" {
		node["title"] = title
	}
	prompt := strings.TrimSpace(a.Prompt)
	if kind == "note" {
		if prompt != "" {
			config["text"] = prompt
		}
		prompt = ""
	}
	if prompt != "" {
		node["prompt"] = prompt
	}
	if len(config) != 0 {
		node["config"] = config
	}
	if access := strings.TrimSpace(a.Access); access != "" {
		node["access"] = access
	}
	if near := strings.TrimSpace(a.Near); near != "" {
		node["near"] = near
	}
	if side := strings.TrimSpace(a.Side); side != "" {
		node["side"] = side
	}
	for name, value := range map[string]number{"x": a.X, "y": a.Y, "w": a.W, "h": a.H} {
		if v := intOf(value); v != nil {
			node[name] = *v
		}
	}
	if a.Connect != nil && (a.Connect.From != "" || a.Connect.To != "") {
		connect := map[string]any{}
		if a.Connect.From != "" {
			connect["from"] = a.Connect.From
		}
		if a.Connect.To != "" {
			connect["to"] = a.Connect.To
		}
		node["connect"] = connect
	}
	res, raw, err := c.ops(ctx, map[string]any{"op": "node.add", "node": node})
	if err != nil {
		return result{}, err
	}
	id := res.IDs["0"]
	at := res.Placed[id]
	// The view says what the node became, and how it is wired.
	var v view
	_, viewErr := c.call(ctx, "GET", "/api/canvas/view", nil, nil, &v)
	var b strings.Builder
	label, title := "", ""
	if viewErr == nil {
		for _, n := range v.Nodes {
			if n.ID == id {
				label, title = " ("+kindLabel(n)+")", " «"+n.Title+"»"
			}
		}
	}
	fmt.Fprintf(&b, "created %s%s%s at (%d,%d %d×%d)", id, title, label, at.X, at.Y, at.W, at.H)
	if viewErr == nil {
		for _, e := range v.Edges {
			if nodeOf(e.From) == id || nodeOf(e.To) == id {
				fmt.Fprintf(&b, "\nconnected %s → %s (%s%s)", e.From, e.To, e.ID, modeNote(e))
			}
		}
	}
	if prompt != "" {
		b.WriteString("\nits first prompt is sent once it runs")
	}
	return result{text: b.String(), raw: raw}, nil
}

// worktreeOf reads CanvasSpawn's worktree: a name, true for one named
// after the node, or nothing.
func worktreeOf(value jsontext.Value) (string, bool, error) {
	value = jsontext.Value(bytes.TrimSpace(value))
	if len(value) == 0 {
		return "", false, nil
	}
	switch value.Kind() {
	case 'n', 'f':
		return "", false, nil
	case 't':
		return "", true, nil
	case '"':
		var name string
		if err := json.Unmarshal(value, &name); err != nil {
			return "", false, err
		}
		name = strings.TrimSpace(name)
		switch strings.ToLower(name) {
		case "", "false", "no", "none":
			return "", false, nil
		case "true", "yes", "new", "auto":
			return "", true, nil
		}
		return name, true, nil
	case '{':
		var spec struct {
			Name string `json:"name"`
		}
		if err := json.Unmarshal(value, &spec); err != nil {
			return "", false, err
		}
		return strings.TrimSpace(spec.Name), true, nil
	}
	return "", false, errors.New("worktree is the name of a worktree")
}

func nodeOf(port string) string {
	node, _, _ := strings.Cut(port, ":")
	return node
}

func modeNote(e viewEdge) string {
	note := ""
	if e.Mode != "" && e.Mode != "auto" {
		note += ", " + e.Mode
	}
	if e.Deliver == "now" {
		note += ", now"
	}
	return note
}

func (c *client) remove(ctx context.Context, a removeArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node to remove")
	}
	_, raw, err := c.ops(ctx, map[string]any{
		"op": "node.remove", "id": node,
		"keep": map[string]any{"session": a.KeepSession, "worktree": true},
	})
	if err != nil {
		return result{}, err
	}
	return result{text: "removed " + node, raw: raw}, nil
}

// splitPort reads node:port, port fallback when it is not given.
func splitPort(ref, fallback string) (string, string) {
	ref = strings.TrimSpace(ref)
	if node, port, ok := strings.Cut(ref, ":"); ok && port != "" {
		return node, port
	}
	return strings.TrimSuffix(ref, ":"), fallback
}

func (c *client) connect(ctx context.Context, a connectArgs) (result, error) {
	fromNode, fromPort := splitPort(a.From, "out")
	toNode, toPort := splitPort(a.To, "in")
	if fromNode == "" || toNode == "" {
		return result{}, errors.New("connect needs from and to: node or node:port")
	}
	edge := map[string]any{
		"from": map[string]string{"node": fromNode, "port": fromPort},
		"to":   map[string]string{"node": toNode, "port": toPort},
	}
	if a.Template != "" {
		edge["template"] = a.Template
	}
	if a.Mode != "" {
		edge["mode"] = a.Mode
	}
	if a.Deliver != "" {
		edge["deliver"] = a.Deliver
	}
	res, raw, err := c.ops(ctx, map[string]any{"op": "edge.add", "edge": edge})
	if err != nil {
		return result{}, err
	}
	id := res.IDs["0"]
	text := fmt.Sprintf("connected %s:%s → %s:%s (%s)", fromNode, fromPort, toNode, toPort, id)
	// A connection that closes a loop of agents starts held for approval.
	var v view
	if _, err := c.call(ctx, "GET", "/api/canvas/view", nil, nil, &v); err == nil {
		for _, e := range v.Edges {
			if e.ID == id && e.Mode == "approve" && a.Mode != "approve" {
				text += "\nit closes a loop of agents: its messages wait for the user's approval"
			}
		}
	}
	return result{text: text, raw: raw}, nil
}

func (c *client) disconnect(ctx context.Context, a disconnectArgs) (result, error) {
	var ids []string
	if edge := strings.TrimSpace(a.Edge); edge != "" {
		ids = append(ids, edge)
	} else {
		if strings.TrimSpace(a.From) == "" || strings.TrimSpace(a.To) == "" {
			return result{}, errors.New("name the connection: edge, or from and to")
		}
		var v view
		if _, err := c.call(ctx, "GET", "/api/canvas/view", nil, nil, &v); err != nil {
			return result{}, err
		}
		fromNode, fromPort := splitPort(a.From, "")
		toNode, toPort := splitPort(a.To, "")
		fromNode, toNode = c.resolve(fromNode), c.resolve(toNode)
		for _, e := range v.Edges {
			eFromNode, eFromPort := splitPort(e.From, "")
			eToNode, eToPort := splitPort(e.To, "")
			if eFromNode == fromNode && eToNode == toNode && (fromPort == "" || fromPort == eFromPort) && (toPort == "" || toPort == eToPort) {
				ids = append(ids, e.ID)
			}
		}
		if len(ids) == 0 {
			return result{}, fmt.Errorf("no connection from %s to %s", a.From, a.To)
		}
	}
	ops := make([]map[string]any, 0, len(ids))
	for _, id := range ids {
		ops = append(ops, map[string]any{"op": "edge.remove", "id": id})
	}
	_, raw, err := c.ops(ctx, ops...)
	if err != nil {
		return result{}, err
	}
	return result{text: "disconnected " + strings.Join(ids, ", "), raw: raw}, nil
}

// resolve names self by the node's ID.
func (c *client) resolve(node string) string {
	if node == "self" && c.node != "" {
		return c.node
	}
	return node
}

func (c *client) move(ctx context.Context, a moveArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node to move")
	}
	set := map[string]any{}
	for name, value := range map[string]number{"x": a.X, "y": a.Y, "w": a.W, "h": a.H} {
		if v := intOf(value); v != nil {
			set[name] = *v
		}
	}
	if len(set) == 0 {
		return result{}, errors.New("say where: x, y, w or h")
	}
	_, raw, err := c.ops(ctx, map[string]any{"op": "node.update", "id": c.resolve(node), "set": set})
	if err != nil {
		return result{}, err
	}
	return result{text: "moved " + node, raw: raw}, nil
}

func (c *client) send(ctx context.Context, a sendArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node to send to")
	}
	if a.Text == "" && len(a.Keys) == 0 {
		return result{}, errors.New("send text, keys, or both")
	}
	body := map[string]any{"text": a.Text}
	if a.Submit != nil {
		body["submit"] = *a.Submit
	}
	if len(a.Keys) != 0 {
		body["keys"] = a.Keys
	}
	if a.When != "" {
		body["when"] = a.When
	}
	wait := bool(a.Wait) && a.Text != ""
	timeout := 600 * time.Second
	if wait {
		if v := intOf(a.TimeoutS); v != nil && *v > 0 {
			timeout = time.Duration(*v) * time.Second
		}
		body["wait"] = true
		body["timeout_s"] = int(timeout / time.Second)
		var cancel context.CancelFunc
		ctx, cancel = timeoutOf(ctx, timeout)
		defer cancel()
	}
	var answer struct {
		Message  *message `json:"message"`
		Node     string   `json:"node"`
		Outputs  int      `json:"outputs"`
		Answered bool     `json:"answered"`
		Answer   string   `json:"answer"`
		Reason   string   `json:"reason"`
		TimedOut bool     `json:"timed_out"`
		Status   status   `json:"status"`
	}
	raw, err := c.call(ctx, "POST", "/api/canvas/nodes/"+url.PathEscape(node)+"/send", nil, body, &answer)
	if err != nil {
		return result{}, err
	}
	var b strings.Builder
	target := answer.Node
	if target == "" {
		target = node
	}
	if wait {
		switch {
		case answer.Answered:
			fmt.Fprintf(&b, "%s answered:\n%s", target, strings.TrimRight(answer.Answer, "\n"))
		case answer.TimedOut:
			fmt.Fprintf(&b, "%s has not answered within %s (%s): its answer comes to you as a reply once there is one", target, timeout, describeStatus(answer.Status))
		default:
			reason := answer.Reason
			if reason == "" {
				reason = "it answered nothing"
			}
			fmt.Fprintf(&b, "%s gave no answer: %s", target, reason)
		}
		return result{text: b.String(), raw: raw}, nil
	}
	state := ""
	if answer.Message != nil {
		state = answer.Message.State
	}
	switch state {
	case "delivered":
		fmt.Fprintf(&b, "delivered to %s", target)
	case "awaiting_approval":
		fmt.Fprintf(&b, "sent to %s: it waits for the user's approval", target)
	case "dropped":
		fmt.Fprintf(&b, "not delivered to %s: %s", target, answer.Message.Reason)
	default:
		fmt.Fprintf(&b, "sent to %s: it is delivered once the node is idle", target)
	}
	if a.Text != "" && state != "dropped" {
		b.WriteString("\nan agent's answer comes to you later as a reply; send with wait to have it as this call's result")
	}
	fmt.Fprintf(&b, "\noutputs so far: %d (read with wait output and after %d for its next one)", answer.Outputs, answer.Outputs)
	return result{text: b.String(), raw: raw}, nil
}

func (c *client) read(ctx context.Context, a readArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node to read")
	}
	q := url.Values{}
	if a.What != "" {
		q.Set("what", a.What)
	}
	lines := 80
	if v := intOf(a.Lines); v != nil && *v > 0 {
		lines = *v
	}
	q.Set("lines", strconv.Itoa(lines))
	timeout := 600 * time.Second
	if a.Wait != "" {
		q.Set("wait", a.Wait)
		if v := intOf(a.TimeoutS); v != nil && *v > 0 {
			timeout = time.Duration(*v) * time.Second
		}
		q.Set("timeout", strconv.Itoa(int(timeout/time.Second)))
		if v := intOf(a.After); v != nil && *v >= 0 {
			q.Set("after", strconv.Itoa(*v))
		}
	}
	ctx, cancel := timeoutOf(ctx, timeout)
	defer cancel()
	var answer struct {
		Node     string `json:"node"`
		Text     string `json:"text"`
		Status   status `json:"status"`
		TimedOut bool   `json:"timed_out"`
		Outputs  *int   `json:"outputs"`
	}
	raw, err := c.call(ctx, "GET", "/api/canvas/nodes/"+url.PathEscape(node)+"/read", q, nil, &answer)
	if err != nil {
		return result{}, err
	}
	var b strings.Builder
	b.WriteString(answer.Node)
	b.WriteString(" · " + describeStatus(answer.Status))
	if answer.TimedOut {
		fmt.Fprintf(&b, " · timed out after %s waiting for %s", timeout, a.Wait)
	}
	text := strings.TrimRight(answer.Text, "\n")
	if text == "" {
		b.WriteString(" · nothing to read")
	} else {
		fmt.Fprintf(&b, " · %d lines\n%s", strings.Count(text, "\n")+1, text)
	}
	return result{text: b.String(), raw: raw}, nil
}

// wait waits for a node, as kou-canvas wait does.
func (c *client) wait(ctx context.Context, node, until string, after *int, timeout time.Duration) (result, error) {
	q := url.Values{}
	if until != "" {
		q.Set("until", until)
	}
	q.Set("timeout", strconv.Itoa(int(timeout/time.Second)))
	if after != nil {
		q.Set("after", strconv.Itoa(*after))
	}
	ctx, cancel := timeoutOf(ctx, timeout)
	defer cancel()
	var answer struct {
		Node     string `json:"node"`
		Status   status `json:"status"`
		Outputs  int    `json:"outputs"`
		TimedOut bool   `json:"timed_out"`
	}
	raw, err := c.call(ctx, "GET", "/api/canvas/nodes/"+url.PathEscape(node)+"/wait", q, nil, &answer)
	if err != nil {
		return result{}, err
	}
	text := fmt.Sprintf("%s · %s · outputs %d", answer.Node, describeStatus(answer.Status), answer.Outputs)
	if answer.TimedOut {
		text = "timed out: " + text
	}
	return result{text: text, raw: raw}, nil
}

func (c *client) emit(ctx context.Context, a emitArgs) (result, error) {
	if a.Text == "" && len(a.Data) == 0 {
		return result{}, errors.New("emit needs text")
	}
	body := map[string]any{"text": a.Text}
	if a.Port != "" {
		body["port"] = a.Port
	}
	if a.Title != "" {
		body["title"] = a.Title
	}
	if data := jsontext.Value(bytes.TrimSpace(a.Data)); len(data) != 0 && string(data) != "null" {
		if !data.IsValid() {
			return result{}, errors.New("data is JSON")
		}
		body["data"] = data
	}
	raw, err := c.call(ctx, "POST", "/api/canvas/emit", nil, body, nil)
	if err != nil {
		return result{}, err
	}
	port := a.Port
	if port == "" {
		port = "out"
	}
	return result{text: "put out on " + port, raw: raw}, nil
}

// describeStatus says a node's status in a few words.
func describeStatus(s status) string {
	text := s.State
	if text == "" {
		text = "unknown"
	}
	if s.Activity != "" {
		text += " (" + s.Activity + ")"
	} else if s.Detail != "" {
		text += " (" + s.Detail + ")"
	}
	return text
}
