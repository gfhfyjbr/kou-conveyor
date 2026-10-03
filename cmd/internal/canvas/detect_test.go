package canvas

import (
	"slices"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// agentsCatalog has a preset that runs Claude Code, and a plugin's that
// runs Codex from its package.
var agentsCatalog = presetCache{harnesses: []harnessDef{
	{Harness: plugin.CanvasHarness{ID: "claude-code", Title: "Claude Code", Command: []string{"claude"}}},
	{Plugin: "extra", Harness: plugin.CanvasHarness{ID: "codex", Title: "Codex", Command: []string{"npx", "-y", "@openai/codex"}, IdleMS: 8000}},
	{Harness: plugin.CanvasHarness{ID: "shell", Title: "Shell"}},
}}

func TestAgentsAreFoundInTheCommandsShellsRun(t *testing.T) {
	for _, tc := range []struct {
		line, agent string
	}{
		{"claude", "claude-code"},
		{"FOO=1 caffeinate -i claude", "claude-code"},
		{"cd x && claude", "claude-code"},
		{"claude --model opus", "claude-code"},
		{"claude -p 'hi'", ""},
		{"claude mcp list", ""},
		{"npx -y @openai/codex@latest", "extra/codex"},
		{"codex", "extra/codex"},
		{"codex exec x", ""},
		{"gemini", "gemini"},
		{"python -m aider", "aider"},
		{"uvx --from aider-chat aider", "aider"},
		{"node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js", "claude-code"},
		{"vim", ""},
		{"bash -c 'claude'", ""},
		{"echo claude", ""},
		{"", ""},
	} {
		f, ok := agentInCommand(agentsCatalog, tc.line)
		got := ""
		if ok {
			got = f.agent.ID
			if !f.agent.Detected {
				t.Errorf("%q: the agent is not said to be found", tc.line)
			}
		}
		if got != tc.agent {
			t.Errorf("%q runs %q, want %q", tc.line, got, tc.agent)
		}
	}

	// What a preset says of its agent goes with it.
	f, ok := agentInCommand(agentsCatalog, "npx -y @openai/codex@latest")
	if !ok || f.idle != 8*time.Second || f.agent.Title != "Codex" || f.agent.Program != "codex" {
		t.Fatalf("codex found as %+v, %+v", f, f.agent)
	}
	// A known agent no preset runs is found all the same.
	if f, ok := agentInCommand(presetCache{}, "claude"); !ok || f.agent.ID != "claude" || f.agent.Title != "Claude Code" || f.input != nil {
		t.Fatalf("claude without its preset: %+v", f)
	}
}

func TestAgentsAreFoundInTheForegroundOfTerminals(t *testing.T) {
	for _, tc := range []struct {
		program terminal.Program
		agent   string
	}{
		{terminal.Program{PID: 7, Name: "claude"}, "claude-code"},
		{terminal.Program{PID: 7, Name: "node", Args: []string{"node", "/opt/homebrew/bin/codex"}}, "extra/codex"},
		{terminal.Program{PID: 7, Name: "node", Args: []string{"node", "/opt/homebrew/bin/codex", "exec", "go on"}}, ""},
		{terminal.Program{PID: 7, Name: "sh", Args: []string{"/bin/sh", "./claude"}}, "claude-code"},
		// A program whose arguments the system does not say has its name.
		{terminal.Program{PID: 7, Name: "gemini", Args: []string{"node"}}, "gemini"},
		{terminal.Program{PID: 7, Name: "vim", Args: []string{"vim", "main.go"}}, ""},
		{terminal.Program{PID: 7, Name: "zsh", Args: []string{"-zsh"}}, ""},
	} {
		f, ok := agentInProgram(agentsCatalog, tc.program)
		got := ""
		if ok {
			got = f.agent.ID
			if f.agent.PID != tc.program.PID {
				t.Errorf("%+v: the agent's process is %d", tc.program, f.agent.PID)
			}
		}
		if got != tc.agent {
			t.Errorf("%+v runs %q, want %q", tc.program, got, tc.agent)
		}
	}
}

func TestCommandLinesSplitAsShellsSplitThem(t *testing.T) {
	for _, tc := range []struct {
		line string
		want [][]string
	}{
		{`claude`, [][]string{{"claude"}}},
		{`a "b c" 'd e' f\ g`, [][]string{{"a", "b c", "d e", "f g"}}},
		{`a; b && c | d`, [][]string{{"a"}, {"b"}, {"c"}, {"d"}}},
		{`(cd x; claude)`, [][]string{{"cd", "x"}, {"claude"}}},
		{`say "a \"quoted\" word" ''`, [][]string{{"say", `a "quoted" word`, ""}}},
		{`unclosed 'quote`, [][]string{{"unclosed", "quote"}}},
		{`  `, nil},
	} {
		got := commandWords(tc.line)
		if !slices.EqualFunc(got, tc.want, slices.Equal) {
			t.Errorf("%q splits into %q, want %q", tc.line, got, tc.want)
		}
	}
}
