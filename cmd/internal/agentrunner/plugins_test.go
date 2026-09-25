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
	code := RunMain(ctx, []string{"-workspace", workspace, "-session-directory", t.TempDir()}, func(name string) string {
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
	if got := toolNames(first); got != "Bash,ViewImage,SkillUse,Echo" {
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
		{"untrusted", nil, `{"prompt":"hi"}`, `"Text":"Bash,ViewImage"`},
		{"disallowed", map[string]string{plugin.TrustEnvironment: "1"}, `{"prompt":"hi","disallowed_tools":["Echo"]}`, `"Text":"Bash,ViewImage,SkillUse"`},
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
	if code != 0 || len(lines) != 3 ||
		!strings.Contains(lines[0], `"name":"core"`) || !strings.Contains(lines[0], `"source":"builtin","active":true,"tools":["Bash","ViewImage","SkillUse"]`) ||
		!strings.Contains(lines[1], `"name":"echo"`) || !strings.Contains(lines[1], `"active":false,"reason":"the workspace is not trusted","tools":["Echo"],"commands":["/shout"]`) ||
		!strings.Contains(lines[2], `"error":`) || !strings.Contains(lines[2], "lowercase") {
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
	stdout, stderr, code := runWithPlugins(t, workspace, map[string]string{plugin.TrustEnvironment: "1"}, `{"prompt":"work"}`,
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
	if tools[0] != "Bash,ViewImage" || tools[1] != "Bash,ViewImage,SkillUse,Late" || tools[3] != "Bash,ViewImage,SkillUse,Late" || tools[4] != "Bash,ViewImage" {
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
