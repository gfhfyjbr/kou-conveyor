package agentrunner

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// writeWorkspacePlugin puts a plugin with a tool, a skill and instructions
// in a workspace.
func writeWorkspacePlugin(t *testing.T, workspace string) {
	t.Helper()
	directory := filepath.Join(plugin.WorkspaceDirectory(workspace), "echo")
	files := map[string]string{
		"plugin.json": `{
			"name": "echo", "version": "0.1.0",
			"tools": [{"name": "Echo", "description": "Repeat the text.", "parameters": {"type": "object", "properties": {"text": {"type": "string"}}}, "run": ["/bin/sh", "./echo.sh"]}],
			"skills": "skills", "prompt": "prompt.md",
			"commands": [{"name": "shout", "description": "Shout", "prompt": "Shout {{args}}"}]
		}`,
		"echo.sh":              "printf 'echo:%s:%s:%s' \"$(cat)\" \"$KOU_CONVEYOR_PLUGIN_NAME\" \"$(basename \"$PWD\")\"\n",
		"prompt.md":            "Use Echo to repeat things.\n",
		"skills/loud/SKILL.md": "---\nname: loud\ndescription: Be loud.\n---\nSHOUT.\n",
	}
	for name, content := range files {
		path := filepath.Join(directory, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o755); err != nil {
			t.Fatal(err)
		}
	}
}

func runWithPlugins(t *testing.T, workspace string, env map[string]string, request string, respond func(int, llm.Request) llm.Response) (string, string, int) {
	t.Helper()
	return runWithPluginsIn(t, workspace, t.TempDir(), env, request, respond)
}

// runWithPluginsIn is runWithPlugins with the sessions kept in sessions.
func runWithPluginsIn(t *testing.T, workspace, sessions string, env map[string]string, request string, respond func(int, llm.Request) llm.Response) (string, string, int) {
	t.Helper()
	var mu sync.Mutex
	calls := 0
	client := &fakeClient{respond: func(_ context.Context, request llm.Request) (llm.Response, error) {
		mu.Lock()
		defer mu.Unlock()
		calls++
		return respond(calls, request), nil
	}}
	ctx, cancel := context.WithTimeout(t.Context(), 20*time.Second)
	defer cancel()
	var stdout, stderr bytes.Buffer
	config := testConfig(client)
	config.PluginConfigDirectory = func(func(string) string) (string, error) { return filepath.Join(workspace, "..", "config"), nil }
	code := RunMain(ctx, []string{"-workspace", workspace, "-session-directory", sessions}, func(name string) string {
		switch name {
		case "OPENAI_API_KEY":
			return "secret"
		case "SHELL":
			return "/bin/sh"
		}
		return env[name]
	}, func() []string { return nil }, strings.NewReader(request), &stdout, &stderr, config)
	return stdout.String(), stderr.String(), code
}

func toolNames(request llm.Request) string {
	var names []string
	for _, definition := range request.Tools {
		names = append(names, definition.Name)
	}
	return strings.Join(names, ",")
}

func TestRunnerRunsWorkspacePluginTools(t *testing.T) {
	workspace := filepath.Join(t.TempDir(), "project")
	writeWorkspacePlugin(t, workspace)
	var first llm.Request
	var result string
	stdout, stderr, code := runWithPlugins(t, workspace, map[string]string{plugin.TrustEnvironment: "1"}, `{"prompt":"repeat hi"}`,
		func(call int, request llm.Request) llm.Response {
			if call == 1 {
				first = request
				return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Echo", Arguments: `{"text": "hi"}`}}}}
			}
			for _, item := range request.Input {
				if value, ok := item.Data.(llm.ToolResult); ok && value.CallID == "call-1" && value.Output[0].Value != contextbuilder.ToolCallRunningPayload {
					result = value.Output[0].Value
				}
			}
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}
		})
	if code != 0 {
		t.Fatalf("exit %d: %s\n%s", code, stderr, stdout)
	}
	if got := toolNames(first); got != "Bash,Read,apply_patch,ViewImage,TranscriptSearch,SkillUse,Echo" {
		t.Fatalf("tools = %s", got)
	}
	system := first.Input[0].Data.(llm.Message).Text
	if !strings.Contains(system, "## echo plugin\n\nUse Echo to repeat things.") || !strings.Contains(system, "<name>loud</name>") {
		t.Fatalf("system prompt lacks the plugin: %s", system)
	}
	if result != `echo:{"text":"hi"}:echo:project` {
		t.Fatalf("result = %q", result)
	}
}

// An agent on a canvas — its runner has the canvas's token — has the
// canvas's tools, which the built-in canvas-agent plugin gives, besides its
// instructions: it does not have to find kou-canvas on its own.
func TestRunnerOffersTheCanvasToolsOnACanvas(t *testing.T) {
	workspace := filepath.Join(t.TempDir(), "project")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatal(err)
	}
	var first llm.Request
	stdout, stderr, code := runWithPlugins(t, workspace, map[string]string{"KOU_CANVAS_TOKEN": "token"}, `{"prompt":"hi"}`,
		func(call int, request llm.Request) llm.Response {
			if call == 1 {
				first = request
			}
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}
		})
	if code != 0 {
		t.Fatalf("exit %d: %s\n%s", code, stderr, stdout)
	}
	if got := toolNames(first); !strings.HasPrefix(got, "Bash,Read,apply_patch,ViewImage,TranscriptSearch,SkillUse,CanvasView,CanvasSpawn,") ||
		!strings.Contains(got, ",CanvasSend,CanvasRead,") {
		t.Fatalf("tools = %s", got)
	}
	if system := first.Input[0].Data.(llm.Message).Text; !strings.Contains(system, "## canvas-agent plugin") {
		t.Fatalf("the system prompt lacks the canvas: %s", system)
	}
}

func TestRunnerLeavesUntrustedWorkspacePluginsOut(t *testing.T) {
	workspace := filepath.Join(t.TempDir(), "project")
	writeWorkspacePlugin(t, workspace)
	answer := func(_ int, request llm.Request) llm.Response {
		return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: toolNames(request)}}}}
	}
	for _, test := range []struct {
		name    string
		env     map[string]string
		request string
		want    string
	}{
		{"untrusted", map[string]string{plugin.DisabledEnvironment: "guide"}, `{"prompt":"hi"}`, `"Text":"Bash,Read,apply_patch,ViewImage,TranscriptSearch"`},
		{"untrusted, with the built-in skill", nil, `{"prompt":"hi"}`, `"Text":"Bash,Read,apply_patch,ViewImage,TranscriptSearch,SkillUse"`},
		{"disallowed", map[string]string{plugin.TrustEnvironment: "1"}, `{"prompt":"hi","disallowed_tools":["Echo"]}`, `"Text":"Bash,Read,apply_patch,ViewImage,TranscriptSearch,SkillUse"`},
		{"core off", map[string]string{plugin.TrustEnvironment: "1", plugin.DisabledEnvironment: "core, other"}, `{"prompt":"hi"}`, `"Text":"Echo"`},
	} {
		t.Run(test.name, func(t *testing.T) {
			stdout, stderr, code := runWithPlugins(t, workspace, test.env, test.request, answer)
			if code != 0 || !strings.Contains(stdout, test.want) {
				t.Fatalf("exit %d, stderr %q, output lacks %s:\n%s", code, stderr, test.want, stdout)
			}
		})
	}
}

func TestRunnerListsPlugins(t *testing.T) {
	workspace := filepath.Join(t.TempDir(), "project")
	writeWorkspacePlugin(t, workspace)
	broken := filepath.Join(plugin.WorkspaceDirectory(workspace), "broken")
	os.MkdirAll(broken, 0o755)
	os.WriteFile(filepath.Join(broken, plugin.ManifestName), []byte(`{"name": "Broken"}`), 0o644)
	var stdout bytes.Buffer
	code := RunMain(t.Context(), []string{"-workspace", workspace, "-list-plugins"}, func(string) string { return "" }, func() []string { return nil },
		strings.NewReader(""), &stdout, &bytes.Buffer{}, testConfig(&fakeClient{}))
	lines := strings.Split(strings.TrimSpace(stdout.String()), "\n")
	if code != 0 || len(lines) != 6 ||
		!strings.Contains(lines[0], `"name":"core"`) || !strings.Contains(lines[0], `"source":"builtin","active":true,"tools":["Bash","ViewImage","SkillUse"]`) ||
		// The canvas's tools are only for agents on a canvas.
		!strings.Contains(lines[1], `"name":"canvas-agent"`) || !strings.Contains(lines[1], `"source":"builtin","active":false,"reason":"only agents on a canvas have it","tools":["CanvasView",`) ||
		!strings.Contains(lines[2], `"name":"guide"`) || !strings.Contains(lines[2], `"source":"builtin","active":true,"skills":true`) ||
		// The tunnel's skill, for agents the browser cockpit runs.
		!strings.Contains(lines[3], `"name":"tunnel-agent"`) || !strings.Contains(lines[3], `"source":"builtin","active":false,"reason":"it needs KOU_CONVEYOR_TUNNEL_CONFIG set","skills":true`) ||
		!strings.Contains(lines[4], `"name":"echo"`) || !strings.Contains(lines[4], `"active":false,"reason":"the workspace is not trusted","tools":["Echo"],"commands":["/shout"],"skills":true,"instructions":true`) ||
		!strings.Contains(lines[5], `"error":`) || !strings.Contains(lines[5], "lowercase") {
		t.Fatalf("exit %d:\n%s", code, stdout.String())
	}
}

// A plugin that comes, changes and goes while the agent works reaches it at
// its next turn: the run goes on with the plugin's tools as they are then,
// and a call made before a tool went still gets its result.
func TestRunnerFollowsPluginsDuringARun(t *testing.T) {
	workspace := filepath.Join(t.TempDir(), "project")
	os.MkdirAll(workspace, 0o755)
	directory := filepath.Join(plugin.WorkspaceDirectory(workspace), "late")
	write := func(files map[string]string) {
		for name, content := range files {
			path := filepath.Join(directory, name)
			os.MkdirAll(filepath.Dir(path), 0o755)
			if err := os.WriteFile(path, []byte(content), 0o755); err != nil {
				t.Fatal(err)
			}
		}
		// A second's step in the file times, whatever the file system's
		// resolution.
		later := time.Now().Add(time.Duration(len(files)) * time.Second)
		filepath.Walk(directory, func(path string, _ os.FileInfo, _ error) error { return os.Chtimes(path, later, later) })
	}
	call := func(id, name, arguments string) llm.Response {
		return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: arguments}}}}
	}
	resultOf := func(request llm.Request, id string) string {
		for _, item := range request.Input {
			if value, ok := item.Data.(llm.ToolResult); ok && value.CallID == id && value.Output[0].Value != contextbuilder.ToolCallRunningPayload {
				return value.Output[0].Value
			}
		}
		return ""
	}
	var tools []string
	var systems []string
	var results []string
	// Without the built-in skill, SkillUse comes and goes with the plugin's.
	stdout, stderr, code := runWithPlugins(t, workspace, map[string]string{plugin.TrustEnvironment: "1", plugin.DisabledEnvironment: "guide"}, `{"prompt":"work"}`,
		func(n int, request llm.Request) llm.Response {
			tools = append(tools, toolNames(request))
			systems = append(systems, request.Input[0].Data.(llm.Message).Text)
			switch n {
			case 1:
				// A plugin comes while the agent works.
				write(map[string]string{
					"plugin.json":          `{"name": "late", "tools": [{"name": "Late", "description": "Say late.", "run": ["/bin/sh", "./late.sh"]}], "prompt": "prompt.md", "skills": "skills"}`,
					"late.sh":              "echo late-v1\n",
					"prompt.md":            "Late has come.\n",
					"skills/slow/SKILL.md": "---\nname: slow\ndescription: Go slowly.\n---\nSlowly.\n",
				})
				return call("call-1", "Bash", `{"command": "true"}`)
			case 2:
				return call("call-2", "Late", `{}`)
			case 3:
				results = append(results, resultOf(request, "call-2"))
				// Its tool changes.
				write(map[string]string{"late.sh": "echo late-v2\n"})
				return call("call-3", "Late", `{}`)
			case 4:
				results = append(results, resultOf(request, "call-3"))
				// It goes; the next turn is without it.
				os.RemoveAll(directory)
				return call("call-4", "Bash", `{"command": "true"}`)
			case 5:
				// A model that calls it anyway is told it has gone.
				return call("call-5", "Late", `{}`)
			case 6:
				results = append(results, resultOf(request, "call-5"))
			}
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}
		})
	if code != 0 {
		t.Fatalf("exit %d: %s\n%s", code, stderr, stdout)
	}
	if len(tools) != 6 {
		t.Fatalf("%d turns: %v", len(tools), tools)
	}
	const core = "Bash,Read,apply_patch,ViewImage,TranscriptSearch"
	if tools[0] != core || tools[1] != core+",SkillUse,Late" || tools[3] != core+",SkillUse,Late" || tools[4] != core {
		t.Fatalf("tools by turn = %q", tools)
	}
	if strings.Contains(systems[0], "Late has come") || !strings.Contains(systems[1], "## late plugin\n\nLate has come.") || !strings.Contains(systems[1], "<name>slow</name>") ||
		strings.Contains(systems[4], "Late has come") || strings.Contains(systems[4], "<name>slow</name>") {
		t.Fatalf("system prompts = %q", systems)
	}
	if len(results) != 3 || !strings.Contains(results[0], "late-v1") || !strings.Contains(results[1], "late-v2") || !strings.Contains(results[2], "not available any more") {
		t.Fatalf("results = %q", results)
	}
	if !strings.Contains(stderr, "plugin> the plugins changed") || !strings.Contains(stderr, "late (Late, instructions, skills)") {
		t.Fatalf("stderr = %s", stderr)
	}
}

// A session may hold calls of tools its run has not: a branch of a canvas
// agent's session runs off the canvas, without the canvas's tools, and holds
// the agent's calls of them; a workspace's plugin may not be trusted any
// more; a model of another family works with another profile of built-in
// tools. The session resumes all the same: the calls' results read as they
// did, the model is not offered the tools, and a call of one now is turned
// away, saying why, and does not run.
func TestRunnerResumesCallsOfToolsItHasNot(t *testing.T) {
	// What the canvas's tools run, with no canvas to talk to.
	programs := t.TempDir()
	if err := os.WriteFile(filepath.Join(programs, "kou-canvas"), []byte("#!/bin/sh\nprintf 'canvas:%s:%s' \"$2\" \"$(cat)\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	path := programs + string(os.PathListSeparator) + os.Getenv("PATH")
	t.Setenv("PATH", path)
	withPath := func(env map[string]string) map[string]string {
		result := map[string]string{"PATH": path}
		for name, value := range env {
			result[name] = value
		}
		return result
	}
	resultOf := func(request llm.Request, id string) string {
		for _, item := range request.Input {
			if value, ok := item.Data.(llm.ToolResult); ok && value.CallID == id && value.Output[0].Value != contextbuilder.ToolCallRunningPayload {
				return value.Output[0].Value
			}
		}
		return ""
	}
	done := llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}
	for _, test := range []struct {
		name          string
		before, after map[string]string // the environments of the runs
		tool          string
		input, again  string // the arguments of the call before, and of the call now
		result        string // of the call before; empty for any but an error
		refusal       string // of the call now; empty for the coordinator's (tool.Unavailable)
	}{
		{"off the canvas", map[string]string{"KOU_CANVAS_TOKEN": "token"}, nil, "CanvasSend",
			`{"node": "n_b", "text": "hi"}`, `{"node": "n_b", "text": "again"}`, `canvas:CanvasSend:{"node":"n_b","text":"hi"}`,
			`tool "CanvasSend" is not available in this run: its plugin, canvas-agent, is not active (only agents on a canvas have it)`},
		{"not trusted", map[string]string{plugin.TrustEnvironment: "1"}, nil, "Echo",
			`{"text": "hi"}`, `{"text": "again"}`, `echo:{"text":"hi"}:echo:project`,
			`tool "Echo" is not available in this run: its plugin, echo, is not active (the workspace is not trusted)`},
		// The test's model, gpt-default, works with the patch profile:
		// apply_patch, and neither Edit nor Write.
		{"another profile", map[string]string{runconfig.ToolProfileEnvironment: "edit"}, nil, "Write",
			`{"path": "note.txt", "content": "hi"}`, `{"path": "note.txt", "content": "again"}`, "", ""},
	} {
		t.Run(test.name, func(t *testing.T) {
			workspace := filepath.Join(t.TempDir(), "project")
			writeWorkspacePlugin(t, workspace)
			sessions := t.TempDir()
			call := func(id, arguments string) llm.Response {
				return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: test.tool, Arguments: arguments}}}}
			}
			var made string
			stdout, stderr, code := runWithPluginsIn(t, workspace, sessions, withPath(test.before), `{"session_id":"resumed","prompt":"work"}`,
				func(n int, request llm.Request) llm.Response {
					if n == 1 {
						return call("call-1", test.input)
					}
					made = resultOf(request, "call-1")
					return done
				})
			if code != 0 || made == "" || strings.HasPrefix(made, "Error") || test.result != "" && made != test.result {
				t.Fatalf("first run: exit %d, result %q: %s\n%s", code, made, stderr, stdout)
			}
			var first llm.Request
			var refused string
			stdout, stderr, code = runWithPluginsIn(t, workspace, sessions, withPath(test.after), `{"session_id":"resumed","prompt":"again"}`,
				func(n int, request llm.Request) llm.Response {
					switch n {
					case 1:
						first = request
						return call("call-2", test.again)
					case 2:
						refused = resultOf(request, "call-2")
					}
					return done
				})
			if code != 0 {
				t.Fatalf("resumed run: exit %d: %s\n%s", code, stderr, stdout)
			}
			if tools := toolNames(first); strings.Contains(tools, test.tool) {
				t.Fatalf("tools = %s", tools)
			}
			if got := resultOf(first, "call-1"); got != made {
				t.Fatalf("the result of the call made before = %q, want %q", got, made)
			}
			unavailable := `tool "` + test.tool + `" is not available; the tools are `
			if test.refusal != "" && refused != test.refusal ||
				test.refusal == "" && (!strings.HasPrefix(refused, unavailable) || strings.Contains(strings.TrimPrefix(refused, unavailable), test.tool)) {
				t.Fatalf("the result of a call now = %q", refused)
			}
			if content, err := os.ReadFile(filepath.Join(workspace, "note.txt")); test.tool == "Write" && (err != nil || string(content) != "hi") {
				t.Fatalf("note.txt = %q, %v: the call now ran", content, err)
			}
		})
	}
}
