// Package canvascli is kou-canvas: how the programs of a canvas's nodes —
// agents, harnesses, shells, sources — see the canvas and act on it. It
// calls the server's canvas API with the token of its node
// (KOU_CANVAS_TOKEN): as a command line, as the tools of the canvas-agent
// plugin (tool), as an MCP server for Claude Code and Codex (mcp), and as
// their hooks (hook), which tell the canvas what the harness does. It
// starts the harness of a terminal node too (launch).
//
// The tools are one model for every way in (tools.go): the command line's
// commands are the tools' calls, and print what the tools answer.
package canvascli

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

// What the server gives the programs of a canvas's nodes.
const (
	envURL   = "KOU_CANVAS_URL"
	envToken = "KOU_CANVAS_TOKEN"
	envID    = "KOU_CANVAS_ID"
	envNode  = "KOU_CANVAS_NODE"
)

// errNoCanvas is what kou-canvas says outside a canvas.
var errNoCanvas = errors.New("not on a canvas: kou-canvas works in the programs of a canvas's nodes, which are given KOU_CANVAS_TOKEN")

// app is kou-canvas's world: its streams and environment.
type app struct {
	in       io.Reader
	out, err io.Writer
	getenv   func(string) string
	// client makes the requests; http.DefaultClient's transport when nil.
	client *http.Client
}

// Main runs kou-canvas and returns its exit code.
func Main(args []string) int {
	a := &app{in: os.Stdin, out: os.Stdout, err: os.Stderr, getenv: os.Getenv}
	return a.run(context.Background(), args)
}

const usage = `kou-canvas: see the canvas this program runs on, and act on it.

  kou-canvas self                                   who you are: node, canvas, access, brief
  kou-canvas view [--full] [--json]                 the nodes, where they are, how they are wired
  kou-canvas spawn <preset> [--title T] [--cmd C] [--cwd D] [--worktree [name]] [--base B]
                   [--prompt P] [--right-of|--below|--left-of|--above <node|self>]
                   [--x X --y Y --w W --h H] [--connect-to N] [--connect-from N] [--access A]
                                                    presets: shell, command, claude-code, codex,
                                                    opencode, agent, note, or plugin/preset
  kou-canvas rm <node> [--keep-session]
  kou-canvas connect <node>[:port] <node>[:port] [--template T] [--approve] [--now]
  kou-canvas disconnect <edge> | <node>[:port] <node>[:port]
  kou-canvas move <node> [--x X] [--y Y] [--w W] [--h H]
  kou-canvas send <node> "text" [--wait [--timeout 600]] [--no-enter] [--now]
                                                    --wait prints the node's answer
  kou-canvas keys <node> C-c Escape Up Enter …
  kou-canvas read <node> [--screen|--tail N|--output|--answer] [--wait idle|output|exit]
                  [--after N] [--timeout 600]
  kou-canvas wait <node> [--until idle|output|exit] [--after N] [--timeout 600]
  kou-canvas emit [--port out] [--title T] "text" [--data JSON]
  kou-canvas launch [--resume]                      start the harness of this terminal node
  kou-canvas hook claude|codex                      the hooks of Claude Code and Codex
  kou-canvas mcp                                    the tools as an MCP server, on stdio
  kou-canvas tool <Name>                            call a tool with JSON arguments on stdin

Nodes are named by their IDs (n_…), or self; send, read and wait take their titles too.
--json prints what the server answered.
`

func (a *app) run(ctx context.Context, args []string) int {
	if len(args) == 0 {
		fmt.Fprint(a.err, usage)
		return 2
	}
	name, rest := args[0], args[1:]
	switch name {
	case "help", "-h", "-help", "--help":
		fmt.Fprint(a.out, usage)
		return 0
	case "hook":
		// A hook never fails the harness, nor holds it up.
		a.hook(ctx, rest)
		return 0
	case "mcp":
		return a.mcp(ctx)
	case "tool":
		return a.tool(ctx, rest)
	case "plugin-json":
		data, err := PluginManifest()
		if err != nil {
			fmt.Fprintln(a.err, "kou-canvas:", err)
			return 1
		}
		a.out.Write(data)
		return 0
	case "launch":
		return a.launch(ctx, rest)
	}
	command, ok := commands[name]
	if !ok {
		fmt.Fprintf(a.err, "kou-canvas: no command %q\n\n%s", name, usage)
		return 2
	}
	parsed, err := parseFlags(rest, command.flags)
	if err != nil {
		fmt.Fprintf(a.err, "kou-canvas %s: %v\n", name, err)
		return 2
	}
	c, err := a.canvasClient()
	if err != nil {
		fmt.Fprintln(a.err, "kou-canvas:", err)
		return 1
	}
	res, err := command.run(ctx, c, parsed)
	if err != nil {
		fmt.Fprintf(a.err, "kou-canvas %s: %v\n", name, err)
		return 1
	}
	if parsed.bool("json") && res.raw != nil {
		value := jsontext.Value(res.raw).Clone()
		if err := value.Indent(jsontext.WithIndentPrefix(""), jsontext.WithIndent("  ")); err != nil {
			a.out.Write(res.raw)
		} else {
			a.out.Write(value)
		}
		fmt.Fprintln(a.out)
		return 0
	}
	fmt.Fprintln(a.out, strings.TrimRight(res.text, "\n"))
	return 0
}

// client calls the canvas API with a node's token.
type client struct {
	base, token string
	node        string // the node's own ID
	http        *http.Client
}

// canvasClient is the client of the node this program runs for.
func (a *app) canvasClient() (*client, error) {
	token := strings.TrimSpace(a.getenv(envToken))
	base := strings.TrimRight(strings.TrimSpace(a.getenv(envURL)), "/")
	if token == "" || base == "" {
		return nil, errNoCanvas
	}
	httpClient := a.client
	if httpClient == nil {
		httpClient = &http.Client{}
	}
	return &client{base: base, token: token, node: strings.TrimSpace(a.getenv(envNode)), http: httpClient}, nil
}

// apiError is what the server said went wrong.
type apiError struct {
	status  int
	message string
}

func (e *apiError) Error() string { return e.message }

// call makes a request, and decodes the answer into out unless it is nil.
// It returns the answer as it came.
func (c *client) call(ctx context.Context, method, path string, query url.Values, body any, out any) ([]byte, error) {
	address := c.base + path
	if len(query) != 0 {
		address += "?" + query.Encode()
	}
	var reader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		reader = bytes.NewReader(data)
	}
	req, err := http.NewRequestWithContext(ctx, method, address, reader)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	res, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("the canvas cannot be reached: %w", err)
	}
	defer res.Body.Close()
	data, err := io.ReadAll(io.LimitReader(res.Body, 64<<20))
	if err != nil {
		return nil, err
	}
	if res.StatusCode >= 300 {
		var problem struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(data, &problem) == nil && problem.Error != "" {
			return nil, &apiError{res.StatusCode, problem.Error}
		}
		return nil, &apiError{res.StatusCode, fmt.Sprintf("the canvas answered %s", res.Status)}
	}
	if out != nil && len(bytes.TrimSpace(data)) != 0 {
		if err := json.Unmarshal(data, out); err != nil {
			return nil, fmt.Errorf("the canvas's answer is not what kou-canvas expects: %w", err)
		}
	}
	return data, nil
}

// timeoutOf bounds a request that waits for as long as the wait it asks.
func timeoutOf(ctx context.Context, wait time.Duration) (context.Context, context.CancelFunc) {
	return context.WithTimeout(ctx, wait+30*time.Second)
}

// tool runs a tool with its arguments on stdin, as the canvas-agent
// plugin's tools do, and prints what it answers.
func (a *app) tool(ctx context.Context, args []string) int {
	if len(args) != 1 {
		fmt.Fprintln(a.err, "kou-canvas tool: name one tool: kou-canvas tool CanvasView < arguments.json")
		return 2
	}
	t, ok := toolNamed(args[0])
	if !ok {
		fmt.Fprintf(a.out, "Error: no tool %q\n", args[0])
		return 2
	}
	input, err := io.ReadAll(io.LimitReader(a.in, 8<<20))
	if err != nil {
		fmt.Fprintln(a.out, "Error:", err)
		return 1
	}
	c, err := a.canvasClient()
	if err != nil {
		fmt.Fprintln(a.out, "Error:", err)
		return 1
	}
	res, err := t.call(ctx, c, input)
	if err != nil {
		fmt.Fprintln(a.out, "Error:", err)
		return 1
	}
	fmt.Fprintln(a.out, strings.TrimRight(res.text, "\n"))
	return 0
}
