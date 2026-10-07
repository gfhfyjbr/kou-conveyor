package plugin

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writePlugin(t *testing.T, directory, manifest string, files ...string) string {
	t.Helper()
	if err := os.MkdirAll(directory, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(directory, ManifestName), []byte(manifest), 0o644); err != nil {
		t.Fatal(err)
	}
	for index := 0; index+1 < len(files); index += 2 {
		path := filepath.Join(directory, files[index])
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(files[index+1]), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	return directory
}

const fullManifest = `{
	"name": "git-glance",
	"version": "1.0.0",
	"description": "Git at a glance",
	"tools": [{"name": "GitGlance", "description": "Show the branch", "parameters": {"type": "object"}, "run": ["python3", "./glance.py", "--json"]}],
	"skills": "skills",
	"prompt": "prompt.md",
	"commands": [{"name": "review", "args": "[focus]", "description": "Review the changes", "prompt": "Review the changes. Focus: {{args}}"}],
	"web": {"script": "web/plugin.js", "style": "web/plugin.css"}
}`

func TestReadLoadsAManifest(t *testing.T) {
	directory := writePlugin(t, filepath.Join(t.TempDir(), "glance"), fullManifest,
		"glance.py", "print(1)", "prompt.md", "  Prefer small commits.\n", "skills/x/SKILL.md", "---\nname: x\ndescription: y\n---\n",
		"web/plugin.js", "export default () => {}", "web/plugin.css", "")
	loaded, err := Read(directory, SourceUser)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.Name != "git-glance" || loaded.Instructions != "Prefer small commits." || loaded.Directory != directory || loaded.Source != SourceUser {
		t.Fatalf("loaded = %+v", loaded)
	}
	command, err := loaded.ToolCommand(loaded.Tools[0])
	if err != nil || strings.Join(command, " ") != "python3 "+filepath.Join(directory, "glance.py")+" --json" {
		t.Fatalf("command = %q, %v", command, err)
	}
	if _, err := loaded.Resolve("../escape"); err == nil {
		t.Fatal("resolved a path outside the plugin")
	}
	review := loaded.Commands[0]
	if got := review.Expand(" tests "); got != "Review the changes. Focus: tests" {
		t.Fatalf("expand = %q", got)
	}
	if got := (Command{Prompt: "Summarize the diff."}).Expand("briefly"); got != "Summarize the diff.\n\nbriefly" {
		t.Fatalf("expand without placeholder = %q", got)
	}
}

func TestReadRejectsBadManifests(t *testing.T) {
	for name, test := range map[string]struct{ manifest, want string }{
		"unknown field":  {`{"name": "x", "tool": []}`, "unknown"},
		"bad name":       {`{"name": "Bad Name"}`, "lowercase"},
		"bad tool name":  {`{"name": "x", "tools": [{"name": "1st", "description": "d", "run": ["true"]}]}`, "must start with a letter"},
		"no command":     {`{"name": "x", "tools": [{"name": "T", "description": "d"}]}`, "needs the command"},
		"no description": {`{"name": "x", "tools": [{"name": "T", "run": ["true"]}]}`, "needs a description"},
		"bad schema":     {`{"name": "x", "tools": [{"name": "T", "description": "d", "run": ["true"], "parameters": {"type": "string"}}]}`, "type"},
		"duplicate tool": {`{"name": "x", "tools": [{"name": "T", "description": "d", "run": ["true"]}, {"name": "T", "description": "d", "run": ["true"]}]}`, "twice"},
		"escaping run":   {`{"name": "x", "tools": [{"name": "T", "description": "d", "run": ["../../bin/sh"]}]}`, "not inside"},
		"bad command":    {`{"name": "x", "commands": [{"name": "Review", "description": "d", "prompt": "p"}]}`, "lowercase"},
		"empty prompt":   {`{"name": "x", "commands": [{"name": "review", "description": "d", "prompt": " "}]}`, "needs a description and a prompt"},
		"missing file":   {`{"name": "x", "prompt": "missing.md"}`, "no such file"},
		"script type":    {`{"name": "x", "web": {"script": "plugin.ts"}}`, ".js or .mjs"},
		"absolute path":  {`{"name": "x", "skills": "/etc"}`, "not inside"},
	} {
		t.Run(name, func(t *testing.T) {
			directory := writePlugin(t, filepath.Join(t.TempDir(), "p"), test.manifest)
			if _, err := Read(directory, SourceUser); err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
}

func TestDiscoverFollowsSourcesTrustAndSettings(t *testing.T) {
	config, workspace := t.TempDir(), t.TempDir()
	tool := func(name string) string {
		return `{"name": "` + name + `", "description": "d", "run": ["true"]}`
	}
	writePlugin(t, filepath.Join(config, "plugins", "a"), `{"name": "shared", "tools": [`+tool("UserTool")+`]}`)
	writePlugin(t, filepath.Join(config, "plugins", "b"), `{"name": "other", "tools": [`+tool("Bash")+`,`+tool("OtherTool")+`], "commands": [{"name": "go", "description": "d", "prompt": "p"}]}`)
	writePlugin(t, filepath.Join(config, "plugins", "broken"), `{"name": "BROKEN"}`)
	os.MkdirAll(filepath.Join(config, "plugins", "not-a-plugin"), 0o755)
	writePlugin(t, filepath.Join(WorkspaceDirectory(workspace), "a"), `{"name": "shared", "tools": [`+tool("WorkspaceTool")+`], "commands": [{"name": "go", "description": "d", "prompt": "p"}]}`)

	names := func(found Found) string {
		var parts []string
		for _, current := range found.Plugins {
			state := "on"
			if !current.Active {
				state = "off"
			}
			var tools []string
			for _, definition := range current.Tools {
				tools = append(tools, definition.Name)
			}
			parts = append(parts, string(current.Source)+":"+current.Name+"="+state+"["+strings.Join(tools, ",")+"]")
		}
		return strings.Join(parts, " ")
	}

	// Off the canvas, the canvas's tools are not there.
	noCanvas := func(string) string { return "" }
	const canvasAgent = "builtin:canvas-agent=off[CanvasView,CanvasSpawn,CanvasRemove,CanvasConnect,CanvasDisconnect,CanvasMove,CanvasSend,CanvasRead,CanvasEmit] "
	// Nor, outside the browser cockpit, the skill that sets up its tunnel.
	const builtins = "builtin:core=on[Bash,ViewImage,SkillUse] " + canvasAgent + "builtin:guide=on[] builtin:tunnel-agent=off[] "
	found := Discover(Options{ConfigDirectory: config, Workspace: workspace, Getenv: noCanvas})
	if got := names(found); got != builtins+"user:shared=on[UserTool] user:other=on[OtherTool] workspace:shared=off[WorkspaceTool]" || found.Trusted {
		t.Fatalf("untrusted: %s", got)
	}
	if len(found.Errors) != 2 || !strings.Contains(found.Errors[0].Error(), "BROKEN") || !strings.Contains(found.Errors[1].Error(), `tool "Bash" is "core"'s already`) {
		t.Fatalf("errors = %v", found.Errors)
	}
	if found.Plugins[1].Reason != "only agents on a canvas have it" || found.Plugins[6].Reason != "the workspace is not trusted" {
		t.Fatalf("reasons = %q, %q", found.Plugins[1].Reason, found.Plugins[6].Reason)
	}

	var settings Settings
	settings.Trust(workspace, true)
	settings.Disable("other", true)
	if err := SaveSettings(config, settings); err != nil {
		t.Fatal(err)
	}
	found = Discover(Options{ConfigDirectory: config, Workspace: workspace, Getenv: noCanvas})
	if got := names(found); got != builtins+"user:shared=off[UserTool] user:other=off[Bash,OtherTool] workspace:shared=on[WorkspaceTool]" || !found.Trusted {
		t.Fatalf("trusted: %s", got)
	}
	if found.Plugins[4].Reason != "replaced by the workspace plugin of the same name" || found.Plugins[5].Reason != "turned off" {
		t.Fatalf("reasons = %q, %q", found.Plugins[4].Reason, found.Plugins[5].Reason)
	}
	if active := found.Active(); len(active) != 3 || len(active[2].Commands) != 1 {
		t.Fatalf("active = %+v", active)
	}
	// On a canvas, they are.
	onCanvas := func(name string) string {
		if name == "KOU_CANVAS_TOKEN" {
			return "kc1.token"
		}
		return ""
	}
	if found := Discover(Options{ConfigDirectory: config, Workspace: workspace, Getenv: onCanvas}); !found.Plugins[1].Active || found.Plugins[1].Instructions == "" {
		t.Fatalf("canvas-agent = %+v", found.Plugins[1])
	}
	// Run by the browser cockpit, which keeps the tunnel's config, the agent
	// has the skill that sets the tunnel up.
	inCockpit := func(name string) string {
		if name == "KOU_CONVEYOR_TUNNEL_CONFIG" {
			return "/tmp/tunnel.json"
		}
		return ""
	}
	if found := Discover(Options{ConfigDirectory: config, Workspace: workspace, Getenv: inCockpit}); found.Plugins[3].Name != "tunnel-agent" || !found.Plugins[3].Active || found.Plugins[3].Skills == "" {
		t.Fatalf("tunnel-agent = %+v", found.Plugins[3])
	}

	found = Discover(Options{ConfigDirectory: config, Workspace: workspace, Disabled: []string{"core"}})
	if found.Plugins[0].Active {
		t.Fatal("core runs although it is turned off")
	}
	reloaded, err := LoadSettings(config)
	if err != nil || !reloaded.Trusts(workspace+string(filepath.Separator)) || len(reloaded.Disabled) != 1 {
		t.Fatalf("settings = %+v, %v", reloaded, err)
	}
}

func TestConfigDirectory(t *testing.T) {
	directory, err := ConfigDirectory(func(name string) string {
		if name == ConfigEnvironment {
			return "/tmp/cockpit/settings.json"
		}
		return ""
	})
	if err != nil || directory != "/tmp/cockpit" {
		t.Fatalf("directory = %q, %v", directory, err)
	}
}

// The repository's example plugins load as they are.
func TestTheExamplesAreValidPlugins(t *testing.T) {
	plugins, problems := ReadDirectory(filepath.Join("..", "..", "examples", "plugins"), SourceUser)
	if len(problems) != 0 {
		t.Fatal(problems)
	}
	var sources []string
	for _, p := range plugins {
		if p.Manifest.Canvas != nil {
			for _, source := range p.Manifest.Canvas.Sources {
				sources = append(sources, p.Manifest.Name+"/"+source.ID)
			}
		}
	}
	if len(plugins) < 3 || strings.Join(sources, " ") != "github-events/github-issues" {
		t.Fatalf("%d plugins, canvas sources %v", len(plugins), sources)
	}
}
