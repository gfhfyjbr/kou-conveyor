package canvascli

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/tool/toolarg"
)

// Tool is a tool of the canvas: one model for the agents' plugin
// (canvas-agent), the MCP server and the command line.
type Tool struct {
	Name        string
	Description string
	// Schema is the JSON schema of its arguments, an object.
	Schema string
	// Waits says that its calls may wait for other nodes: while all that
	// runs of an agent is such calls, the harness does not ask the agent
	// for progress reports (plugin.Tool's waits).
	Waits bool
	// Refetch says that its results show what the canvas keeps — the board,
	// a node's screen, messages and transcript —, which a call fetches
	// again: a long conversation of an agent prunes them before the other
	// tools' results (plugin.Tool's refetch).
	Refetch bool
	run     func(ctx context.Context, c *client, args jsontext.Value) (result, error)
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
			"A busy kou agent that has shown nothing for 10 minutes or more is marked quiet: it waits on a long command, or it is stuck — its transcript tells. " +
			"To message a node you know the title of, CanvasSend needs no look first.",
		Schema: `{"type":"object","properties":{` +
			`"detail":{"type":"string","enum":["summary","full"],"description":"full adds the connections' templates and the notes' texts."}}}`,
		Refetch: true,
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
		Description: "Create a node on the canvas — a kou agent (the default), a terminal running a command or another coding agent, a note — beside your node unless you say where, " +
			"and give it its first prompt. A kou agent is an agent like you with a session of its own, which the canvas talks to directly: " +
			"make coders, reviewers and researchers kou agents, and give each one that writes code a worktree of its own. " +
			"An agent you spawn (a kou agent, claude-code, codex, opencode) is wired to you: its output to your input, so what it answers comes back to you as a message; " +
			"connect wires it otherwise, connect {} not at all. It answers the node's ID and place.",
		Schema: `{"type":"object","properties":{` +
			`"preset":{"type":"string","description":"What the node runs: agent (the default: a kou agent like you), shell, command (runs command), note, claude-code, codex, opencode, or a plugin's preset as plugin/preset."},` +
			`"title":{"type":"string","description":"Its title, which the other nodes see: short and telling."},` +
			`"command":{"type":"string","description":"For a command node: the command line it runs."},` +
			`"cwd":{"type":"string","description":"The directory it works in, relative to the workspace or absolute: where a terminal starts; for a kou agent without a worktree, the folder its tools and shell work in (its session stays this workspace's). A worktree goes before it."},` +
			`"worktree":{"type":"string","description":"Work in a git worktree of its own, made when missing: its name, its branch kou/<name>. One writer per worktree. A kou agent's worktree is named after its session, whatever the name."},` +
			`"base":{"type":"string","description":"The branch or commit a new worktree starts from; HEAD by default."},` +
			`"include":{"type":"array","items":{"type":"string"},"description":"Paths of the workspace git does not carry (vendor/, a tool's folder, fonts) to copy into its worktree, which gets the submodules of the user's checkout by itself."},` +
			`"link":{"type":"array","items":{"type":"string"},"description":"Such paths to link into its worktree instead of copying: shared with the user's checkout, for big things nobody writes."},` +
			`"prompt":{"type":"string","description":"Its first prompt, sent once it runs; for a note, its text."},` +
			`"config":{"type":"object","description":"More of its configuration, as its preset defines it: for a kou agent model, effort (low, medium, high, xhigh, max), instructions (its role), output, and reply (auto: what it answers at a turn's end goes back to the node that sent it a message; off: it answers nodes with CanvasSend only — a foreman's default); permission_mode for claude-code."},` +
			`"access":{"type":"string","enum":["none","observe","talk","build"],"description":"What its program may do on the canvas: talk by default for what an agent makes; build lets it make nodes too."},` +
			`"near":{"type":"string","description":"The node to place it beside: yours by default."},` +
			`"side":{"type":"string","enum":["right","below","left","above"],"description":"Which side of near: the first free one by default."},` +
			`"x":{"type":"integer","description":"Where it goes, instead of beside near: its left edge."},` +
			`"y":{"type":"integer","description":"Its top edge."},` +
			`"w":{"type":"integer","description":"Its width; its preset's by default."},` +
			`"h":{"type":"integer","description":"Its height; its preset's by default."},` +
			`"connect":{"type":"object","description":"Wire it at once: from's output to its input, its output to to's input. A node is its ID or self, with :port when not out or in. Left out, an agent is wired to answer you, as {\"to\":\"self\"}, and nothing else is wired; {} wires nothing.",` +
			`"properties":{"from":{"type":"string"},"to":{"type":"string"}}}` +
			`}}`,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a spawnArgs
			if err := decodeArgs("CanvasSpawn", args, &a); err != nil {
				return result{}, err
			}
			// A tool is called by an agent: a kou agent, or a coding
			// agent through kou-canvas mcp.
			a.byAgent = true
			return c.spawn(ctx, a)
		},
	},
	{
		Name:        "CanvasRemove",
		Description: "Remove a node from the canvas: one you made, or any with admin access. Its program ends — a kou agent's run is stopped, and its session deleted; its worktree stays, with its work.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node's ID."},` +
			`"keep_session":{"type":"boolean","description":"Only when the user asks to keep it: keeps a kou agent's session, which then shows in the user's session list as a session of its own. By default it is deleted with the node."},` +
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
			`"deliver":{"type":"string","enum":["queue","now"],"description":"By default a kou agent gets each message at once — at work, it reads it between its tool calls — and a terminal once it is idle. queue makes a kou agent wait until its turn ends too; now types into a busy terminal."}` +
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
			"Without wait, an agent's answer comes to you later as a prompt that starts with [canvas] reply from — unless it is a foreman, or its config's reply is off: those answer with CanvasSend. " +
			"A kou agent reads the text at once, between its tool calls; a terminal is given it once idle, and Enter is pressed after it; keys are pressed as tmux send-keys names them. " +
			"With replace, the text takes the place of your last prompt to a kou agent — one its model refused, say — instead of coming after it: no need to remove the agent and start over.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node: its ID (n_…) or its title."},` +
			`"text":{"type":"string","description":"The message: the prompt, or what to type."},` +
			`"wait":{"type":"boolean","description":"Wait for the node's answer to this message, and return it. A node that waits for your answer cannot be waited for in turn."},` +
			`"timeout_s":{"type":"integer","description":"With wait: how long to wait at most, in seconds; 600 by default. An answer that comes later comes to you as a reply."},` +
			`"submit":{"type":"boolean","description":"Press Enter after the text: true by default."},` +
			`"keys":{"type":"array","items":{"type":"string"},"description":"Keys to press after the text, as tmux names them: Enter, Tab, Escape, BSpace, Up, Down, Left, Right, Home, End, C-c, M-x, F1…"},` +
			`"when":{"type":"string","enum":["idle","now"],"description":"By default a kou agent reads it at once (between its tool calls) and a terminal once it is idle. idle makes a kou agent wait until its turn ends; now types into a busy terminal, and starts an idle kou agent even when its queue is paused."},` +
			`"replace":{"type":"boolean","description":"For a kou agent: the text replaces your last prompt to it, as an edited prompt does. Its session goes back to before that prompt — the agent forgets the prompt and all its run did of it, files aside — and runs this text from there. Only once its run is over, and only while no other prompt came after yours."}` +
			`},"required":["node"]}`,
		Waits: true,
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
			`"wait":{"type":"string","enum":["idle","output","exit"],"description":"Wait until the node is idle, gives its next output (or its run ends without one: refused, stopped, failed), or ends, then read."},` +
			`"after":{"type":"integer","description":"With wait output: the count of outputs CanvasSend answered, so that an output given meanwhile counts."},` +
			`"timeout_s":{"type":"integer","description":"How long to wait at most, in seconds: 600 by default."}` +
			`},"required":["node"]}`,
		Waits:   true,
		Refetch: true,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a readArgs
			if err := decodeArgs("CanvasRead", args, &a); err != nil {
				return result{}, err
			}
			return c.read(ctx, a)
		},
	},
	{
		Name: "CanvasTranscript",
		Description: "Read a node's raw transcript: its prompts, answers, reasoning, tool calls with their outcomes, notices, and what the canvas told it (kind peer). " +
			"tail=N gives the last N entries, wherever you stopped; after=last reads on from where you last stopped reading the node, a cursor of yours for each filter of kinds and tools. " +
			"An answer holds about 30 KB: when it says more, read on with after set to its next — what did not fit is not lost. " +
			"Filter by kinds, tools and time; ask for notes or summaries when you look into a stall.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node whose raw transcript to read: its ID, title, or self."},` +
			`"tail":{"type":"integer","description":"The last N entries that match, 500 at most: what the node did lately. It moves no cursor, and does not go with after."},` +
			`"after":{"type":"string","description":"Read on after a cursor: the next of an answer, or last for where you stopped reading the node with the same kinds and tools. Neither after nor tail: from the session's start."},` +
			`"limit":{"type":"integer","description":"Maximum entries: 50 by default, 500 at most; fewer when they would not fit the answer."},` +
			`"kinds":{"type":"array","items":{"type":"string","enum":["user","assistant","reasoning","tool","notice","error","compaction","heartbeat","peer"]},"description":"Only these entry kinds: peer is a message of the canvas, from another node."},` +
			`"tools":{"type":"array","items":{"type":"string"},"description":"Only tool entries with these names."},` +
			`"since":{"type":"string","description":"Only entries at or after this RFC3339 time. A read with since or until moves no cursor."},` +
			`"until":{"type":"string","description":"Only entries at or before this RFC3339 time."},` +
			`"message_limit":{"type":"integer","description":"Maximum bytes of message, reasoning and detail fields: 8 KiB by default, less for an entry too big for the answer."},` +
			`"output_limit":{"type":"integer","description":"Maximum bytes of tool output, stderr and error fields: 4 KiB by default, less for an entry too big for the answer. An output the harness kept whole in a file names it."},` +
			`"include":{"type":"array","items":{"type":"string","enum":["notes","summaries"]},"description":"Include the durable notes or recent summaries beside the entries."},` +
			`"format":{"type":"string","enum":["text","json"],"description":"text (the default): an entry a line or a block, at local times; json: the entries as the server gives them, with their IDs and cursors."}` +
			`},"required":["node"]}`,
		Refetch: true,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a transcriptArgs
			if err := decodeArgs("CanvasTranscript", args, &a); err != nil {
				return result{}, err
			}
			return c.transcript(ctx, a)
		},
	},
	{
		Name: "CanvasSearch",
		Description: "Search a node's persisted raw transcript with a case-insensitive regular expression, showing matching lines with context. " +
			"Use nodes=children to search all nodes you look after; this is for locating evidence, while CanvasTranscript reads the structured incremental feed.",
		Schema: `{"type":"object","properties":{` +
			`"node":{"type":"string","description":"The node to search: its ID, title, or self; with nodes=children it is the parent context."},` +
			`"nodes":{"type":"string","enum":["children"],"description":"Search all of your children instead of only node."},` +
			`"q":{"type":"string","description":"A case-insensitive regular expression to find."},` +
			`"limit":{"type":"integer","description":"Maximum matches: 20 by default, 200 at most."}` +
			`},"required":["q"]}`,
		Refetch: true,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a searchArgs
			if err := decodeArgs("CanvasSearch", args, &a); err != nil {
				return result{}, err
			}
			return c.search(ctx, a)
		},
	},
	{
		Name: "CanvasWait",
		Description: "Wait for any of several nodes — your children by default: the nodes you made and those you are wired to — instead of polling or sleeping. " +
			"until answer (the default): one of them gives an output, an agent's answer — or a run of it ends without one, as when its model refuses, which the result says; idle: all of them are idle; exit: one of them ends; any: one of them changes at all. " +
			"It answers which node it was, with the state of each; pass its next as after to the next wait, so that nothing given meanwhile is missed. " +
			"No need to wait at all for an answer to come back: end your turn, and the answer wakes you as a message.",
		Schema: `{"type":"object","properties":{` +
			`"nodes":{"type":"array","items":{"type":"string"},"description":"The nodes: their IDs (n_…) or titles. Your children when not given."},` +
			`"until":{"type":"string","enum":["answer","idle","exit","any"],"description":"What ends the wait: answer (the default), idle, exit or any."},` +
			`"after":{"type":"string","description":"The next the last wait answered: outputs given since count, those before do not."},` +
			`"timeout_s":{"type":"integer","description":"How long to wait at most, in seconds: 600 by default, 1800 at most."}` +
			`}}`,
		Waits: true,
		run: func(ctx context.Context, c *client, args jsontext.Value) (result, error) {
			var a waitArgs
			if err := decodeArgs("CanvasWait", args, &a); err != nil {
				return result{}, err
			}
			return c.waitAny(ctx, a)
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

// number is a number of a tool's arguments, which models write as 100,
// 100.0 or "100" alike; nil when it is not given.
type number = *toolarg.Number

func intOf(n number) *int {
	if n == nil {
		return nil
	}
	v := n.Rounded()
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
	Include  nodeList       `json:"include"`
	Link     nodeList       `json:"link"`
	Prompt   string         `json:"prompt"`
	Config   jsontext.Value `json:"config"`
	Access   string         `json:"access"`
	Near     string         `json:"near"`
	Side     string         `json:"side"`
	X        number         `json:"x"`
	Y        number         `json:"y"`
	W        number         `json:"w"`
	H        number         `json:"h"`
	Connect  *spawnConnect  `json:"connect"`
	// byAgent: the caller is an agent, as a tool's is; a command line
	// may be a shell's.
	byAgent bool
}

// spawnConnect wires a new node at once: from's output to its input, its
// output to to's input. Both left empty, it wires nothing.
type spawnConnect struct {
	From string `json:"from"`
	To   string `json:"to"`
}

// codingAgents are the presets of terminals that run a coding agent, which
// answers what it is sent as a kou agent does.
var codingAgents = map[string]bool{"claude-code": true, "codex": true, "opencode": true}

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
	Replace  loose    `json:"replace"`
}

type readArgs struct {
	Node     string `json:"node"`
	What     string `json:"what"`
	Lines    number `json:"lines"`
	Wait     string `json:"wait"`
	After    number `json:"after"`
	TimeoutS number `json:"timeout_s"`
}

type waitArgs struct {
	Nodes    nodeList `json:"nodes"`
	Until    string   `json:"until"`
	After    string   `json:"after"`
	TimeoutS number   `json:"timeout_s"`
}

// nodeList is the nodes of a tool's arguments, which models write as a
// list or as one string, "a, b".
type nodeList []string

func (l *nodeList) UnmarshalJSON(data []byte) error {
	var list []string
	if err := json.Unmarshal(data, &list); err == nil {
		*l = list
		return nil
	}
	var one string
	if err := json.Unmarshal(data, &one); err != nil {
		return errors.New("it is a list of nodes")
	}
	*l = strings.Split(one, ",")
	return nil
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
	QuietS    int      `json:"quiet_s"`
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
	ID      string `json:"id"`
	State   string `json:"state"`
	Reason  string `json:"reason"`
	Deliver string `json:"deliver"`
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
		// A node of no preset is a kou agent: the canvas's own, which it
		// talks to and reads directly.
		preset = "agent"
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
	include, link := paths(a.Include), paths(a.Link)
	if name, wanted, err := worktreeOf(a.Worktree); err != nil {
		return result{}, err
	} else if wanted || len(include) > 0 || len(link) > 0 {
		// What config says of the worktree stays, but for what the
		// arguments say.
		spec, _ := config["worktree"].(map[string]any)
		if spec == nil {
			spec = map[string]any{}
		}
		if agent {
			config["sandbox"] = "worktree"
		} else {
			if name != "" {
				spec["name"] = name
			}
			if base := strings.TrimSpace(a.Base); base != "" {
				spec["base"] = base
			}
		}
		if len(include) > 0 {
			spec["include"] = include
		}
		if len(link) > 0 {
			spec["link"] = link
		}
		if !agent || len(spec) > 0 {
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
	// An agent an agent spawns answers it: its output is wired to the
	// spawner's input, so that what it ends a turn with comes back as a
	// message, unless connect says otherwise. A command line wires so for
	// an agent's program only: a shell would take the answers for commands.
	wire := a.Connect
	answers := agent || codingAgents[preset] && (kind == "" || kind == "terminal")
	if wire == nil && answers && c.node != "" && (a.byAgent || c.runsAnAgent(ctx)) {
		wire = &spawnConnect{To: "self"}
	}
	if wire != nil && (wire.From != "" || wire.To != "") {
		connect := map[string]any{}
		if wire.From != "" {
			connect["from"] = wire.From
		}
		if wire.To != "" {
			connect["to"] = wire.To
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

// runsAnAgent says whether the node this program runs for is an agent's,
// as the canvas has it: a kou agent, or a terminal of a coding agent.
func (c *client) runsAnAgent(ctx context.Context) bool {
	var s self
	if _, err := c.call(ctx, "GET", "/api/canvas/self", nil, nil, &s); err != nil {
		return false
	}
	switch n := s.Node; n.Kind {
	case "agent":
		return true
	case "terminal":
		return n.Preset != "" && n.Preset != "shell" && n.Preset != "command"
	}
	return false
}

// paths are the paths of a list, trimmed, without the empty ones.
func paths(list nodeList) []string {
	var out []string
	for _, p := range list {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
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
	if a.Replace {
		switch {
		case len(a.Keys) != 0:
			return result{}, errors.New("replace takes text, not keys")
		case a.Text == "":
			return result{}, errors.New("replace needs the text that takes your last prompt's place")
		}
		body["replace"] = true
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
	if a.Replace && (answer.Message == nil || answer.Message.State != "dropped") {
		fmt.Fprintf(&b, "your last prompt to %s is replaced: its session went back to before it, and runs this text from there\n", target)
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
			if strings.HasSuffix(reason, "refused to answer") {
				b.WriteString("\nto word it otherwise, send the new text with replace: it takes this prompt's place, and the refused one is forgotten")
			}
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
		if answer.Message != nil && answer.Message.Deliver == "now" {
			fmt.Fprintf(&b, "sent to %s: it goes in at once — an agent at work reads it between its tool calls", target)
		} else {
			fmt.Fprintf(&b, "sent to %s: it is delivered once the node is idle", target)
		}
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
		NoAnswer string `json:"no_answer"`
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
	if answer.NoAnswer != "" {
		b.WriteString(" · " + noAnswer(answer.NoAnswer))
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
		NoAnswer string `json:"no_answer"`
	}
	raw, err := c.call(ctx, "GET", "/api/canvas/nodes/"+url.PathEscape(node)+"/wait", q, nil, &answer)
	if err != nil {
		return result{}, err
	}
	text := fmt.Sprintf("%s · %s · outputs %d", answer.Node, describeStatus(answer.Status), answer.Outputs)
	if answer.NoAnswer != "" {
		text += " · " + noAnswer(answer.NoAnswer)
	}
	if answer.TimedOut {
		text = "timed out: " + text
	}
	return result{text: text, raw: raw}, nil
}

// noAnswer says that a node's last run ended without an answer, and why.
func noAnswer(why string) string {
	return "its last run ended without an answer: " + why
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

// waitAny waits for any of several nodes (CanvasWait).
func (c *client) waitAny(ctx context.Context, a waitArgs) (result, error) {
	q := url.Values{}
	var nodes []string
	for _, n := range a.Nodes {
		if n = strings.TrimSpace(n); n != "" {
			nodes = append(nodes, n)
		}
	}
	if len(nodes) > 0 {
		q.Set("nodes", strings.Join(nodes, ","))
	}
	if a.Until != "" {
		q.Set("until", a.Until)
	}
	if a.After != "" {
		q.Set("after", a.After)
	}
	timeout := 600 * time.Second
	if v := intOf(a.TimeoutS); v != nil && *v > 0 {
		timeout = time.Duration(*v) * time.Second
	}
	q.Set("timeout", strconv.Itoa(int(timeout/time.Second)))
	ctx, cancel := timeoutOf(ctx, timeout)
	defer cancel()
	var answer struct {
		Fired string `json:"fired"`
		Until string `json:"until"`
		Nodes []struct {
			Node     string `json:"node"`
			Title    string `json:"title"`
			Status   status `json:"status"`
			Outputs  int    `json:"outputs"`
			Gone     bool   `json:"gone"`
			NoAnswer string `json:"no_answer"`
		} `json:"nodes"`
		Next     string `json:"next"`
		TimedOut bool   `json:"timed_out"`
	}
	raw, err := c.call(ctx, "GET", "/api/canvas/wait", q, nil, &answer)
	if err != nil {
		return result{}, err
	}
	var b strings.Builder
	switch {
	case answer.TimedOut:
		fmt.Fprintf(&b, "Timed out after %s waiting for %s.", timeout, answer.Until)
	case answer.Fired != "":
		title, why := answer.Fired, ""
		for _, n := range answer.Nodes {
			if n.Node == answer.Fired {
				if n.Title != "" {
					title = n.Node + " «" + n.Title + "»"
				}
				why = n.NoAnswer
			}
		}
		if why != "" && (answer.Until == "answer" || answer.Until == "output") {
			// No answer comes of the run: waiting on would wait for nothing.
			fmt.Fprintf(&b, "%s: %s ended its run without an answer — %s. "+
				"Send it the task again, or, to word it otherwise, CanvasSend with replace, which edits your last message to it and runs it anew.", answer.Until, title, why)
		} else {
			fmt.Fprintf(&b, "%s: %s.", answer.Until, title)
		}
	default:
		fmt.Fprintf(&b, "%s: already so, nothing to wait for.", answer.Until)
	}
	for _, n := range answer.Nodes {
		if n.Gone {
			fmt.Fprintf(&b, "\n%s · removed", n.Node)
			continue
		}
		fmt.Fprintf(&b, "\n%s «%s» · %s · %d outputs", n.Node, n.Title, describeStatus(n.Status), n.Outputs)
		if n.NoAnswer != "" {
			b.WriteString(" · " + noAnswer(n.NoAnswer))
		}
	}
	if answer.Next != "" {
		fmt.Fprintf(&b, "\nnext: %s", answer.Next)
	}
	return result{text: b.String(), raw: raw}, nil
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
