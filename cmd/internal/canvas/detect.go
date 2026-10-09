package canvas

import (
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Agents in terminals. A terminal node may run an agent's program: the one
// its preset launches — Claude Code, Codex —, or one the user runs in its
// shell, which the canvas finds in the command the shell says it started,
// and in the program in the foreground of its terminal. A node that runs
// one is an agent's: what is sent to it is a prompt, under a line that says
// who sends it; whether it works or waits is told from what it shows —
// output that goes on, and is not the echo of what was typed —; and once
// it falls quiet after a message from the canvas, its screen is its answer.

// Agent is the agent a terminal node runs.
type Agent struct {
	// ID names it: the preset's it is, else the known agent's (claude,
	// codex…).
	ID    string `json:"id"`
	Title string `json:"title"`
	// Program is what runs it, PID its process, when known.
	Program string `json:"program,omitzero"`
	PID     int    `json:"pid,omitzero"`
	// Detected: the user ran it in the node's shell; else the node's preset
	// launched it.
	Detected bool `json:"detected,omitzero"`
}

func sameAgent(a, b *Agent) bool { return a == b || a != nil && b != nil && *a == *b }

// knownAgent is an agent's program the canvas knows without a preset.
type knownAgent struct {
	id, title string
	// names are what it runs as; packages, what a runner (npx, uvx…) runs
	// it from.
	names, packages []string
	// once are what has it run one prompt and exit — a command, not an
	// agent at the terminal: flags anywhere, a subcommand first.
	once []string
}

var knownAgents = []knownAgent{
	{id: "claude", title: "Claude Code", names: []string{"claude", "claude-code"}, packages: []string{"@anthropic-ai/claude-code"}, once: []string{"-p", "--print", "mcp", "config", "update", "doctor"}},
	{id: "codex", title: "Codex", names: []string{"codex"}, packages: []string{"@openai/codex"}, once: []string{"exec", "e", "login", "logout", "mcp", "apply", "completion"}},
	{id: "gemini", title: "Gemini CLI", names: []string{"gemini", "gemini-cli"}, packages: []string{"@google/gemini-cli"}, once: []string{"-p", "--prompt", "mcp", "extensions"}},
	{id: "opencode", title: "OpenCode", names: []string{"opencode"}, packages: []string{"opencode-ai"}, once: []string{"run", "auth", "serve", "upgrade"}},
	{id: "aider", title: "Aider", names: []string{"aider"}, packages: []string{"aider-chat"}, once: []string{"-m", "--message", "--message-file"}},
	{id: "cursor-agent", title: "Cursor Agent", names: []string{"cursor-agent"}, once: []string{"-p", "--print"}},
	{id: "amp", title: "Amp", names: []string{"amp"}, packages: []string{"@sourcegraph/amp"}, once: []string{"-x", "--execute"}},
	{id: "copilot", title: "Copilot CLI", names: []string{"copilot"}, packages: []string{"@github/copilot"}, once: []string{"-p", "--prompt"}},
	{id: "qwen", title: "Qwen Code", names: []string{"qwen"}, packages: []string{"@qwen-code/qwen-code"}, once: []string{"-p", "--prompt"}},
	{id: "goose", title: "Goose", names: []string{"goose"}, once: []string{"run"}},
	{id: "crush", title: "Crush", names: []string{"crush"}, packages: []string{"@charmland/crush"}, once: []string{"run"}},
	{id: "droid", title: "Droid", names: []string{"droid"}, once: []string{"exec"}},
	{id: "kou", title: "kou-conveyor", names: []string{"kou-conveyor-tui"}},
}

// found is an agent found running in a shell, and what its preset says of
// it, when it is one's: how it takes text, how long it is quiet before it
// waits.
type found struct {
	agent *Agent
	input *plugin.CanvasInput
	idle  time.Duration
}

// agentInCommand finds the agent a shell's command line runs, if it runs
// one.
func agentInCommand(catalog presetCache, line string) (found, bool) {
	for _, words := range commandWords(line) {
		if f, ok := agentIn(catalog, words); ok {
			return f, true
		}
	}
	return found{}, false
}

// agentInProgram finds the agent a program in the foreground of a
// terminal is, if it is one.
func agentInProgram(catalog presetCache, p terminal.Program) (found, bool) {
	words := p.Args
	if len(words) == 0 {
		words = []string{p.Name}
	}
	f, ok := agentIn(catalog, words)
	if !ok && p.Name != "" && len(p.Args) > 0 {
		f, ok = agentIn(catalog, []string{p.Name})
	}
	if ok {
		agent := *f.agent
		agent.PID = p.PID
		f.agent = &agent
	}
	return f, ok
}

// agentIn finds the agent a command's words run: a preset of the
// catalog's that runs it, else a known agent.
func agentIn(catalog presetCache, words []string) (found, bool) {
	l := launchedBy(words)
	if l.name == "" {
		return found{}, false
	}
	known, isKnown := knownAgentOf(l)
	if isKnown && runsOnce(known.once, l.args) {
		return found{}, false
	}
	for _, h := range catalog.harnesses {
		if len(h.Harness.Command) == 0 {
			continue
		}
		own := launchedBy(h.Harness.Command)
		if own.name == l.name || own.pkg != "" && own.pkg == l.pkg || isKnown && slices.Contains(known.names, own.name) {
			f := found{
				agent: &Agent{ID: presetName(h.Plugin, h.Harness.ID), Title: h.Harness.Title, Program: l.name, Detected: true},
				input: h.Harness.Input,
				idle:  time.Duration(h.Harness.IdleMS) * time.Millisecond,
			}
			return f, true
		}
	}
	if isKnown {
		return found{agent: &Agent{ID: known.id, Title: known.title, Program: l.name, Detected: true}}, true
	}
	return found{}, false
}

// knownAgentOf finds the known agent a program is.
func knownAgentOf(l launched) (knownAgent, bool) {
	for _, k := range knownAgents {
		if slices.Contains(k.names, l.name) || l.pkg != "" && slices.Contains(k.packages, l.pkg) {
			return k, true
		}
		for _, p := range k.packages {
			if strings.Contains(l.path, "/node_modules/"+p+"/") {
				return k, true
			}
		}
	}
	return knownAgent{}, false
}

// runsOnce reports whether arguments have a program run one prompt and
// exit: one of once's flags anywhere, or its subcommand first.
func runsOnce(once, args []string) bool {
	first := true
	for _, arg := range args {
		if arg == "--" {
			break
		}
		if strings.HasPrefix(arg, "-") {
			flag, _, _ := strings.Cut(arg, "=")
			if slices.Contains(once, flag) {
				return true
			}
			continue
		}
		if first && slices.Contains(once, arg) {
			return true
		}
		first = false
	}
	return false
}

// launched is what a command's words run: the program — its name, the path
// it was run by, the package a runner runs it from — and its arguments.
type launched struct {
	name, path, pkg string
	args            []string
}

// wrappers run the command that follows their options; the letters are
// their options that take a value.
var wrappers = map[string]string{
	"env": "uCPS", "nohup": "", "time": "", "caffeinate": "tw", "nice": "n", "command": "",
	"builtin": "", "exec": "a", "sudo": "ugpUCDhrT", "doas": "uC", "stdbuf": "ioe",
	"unbuffer": "", "noglob": "", "nocorrect": "", "rlwrap": "", "op": "",
}

// interpreters run the script that follows their options — a script run by
// its #! line has its interpreter's name.
var interpreters = map[string]bool{
	"node": true, "nodejs": true, "bun": true, "deno": true, "tsx": true, "ts-node": true,
	"python": true, "python3": true, "ruby": true, "perl": true,
	"sh": true, "bash": true, "zsh": true, "dash": true,
}

// runners run a package's program: the subcommand that does, if they need
// one.
var runners = map[string][]string{
	"npx": nil, "bunx": nil, "pnpx": nil, "uvx": nil,
	"pipx": {"run"}, "npm": {"exec", "x"}, "pnpm": {"dlx", "exec"}, "yarn": {"dlx", "exec"}, "uv": {"tool"},
}

// launchedBy finds what a command's words run: past the variables they set
// and the programs that run another (env, nohup…), through interpreters
// (node script) and the runners of packages (npx package).
func launchedBy(words []string) launched {
	for len(words) > 0 {
		word := words[0]
		base := filepath.Base(word)
		if isAssignment(word) {
			words = words[1:]
			continue
		}
		if values, ok := wrappers[base]; ok {
			words = skipOptions(words[1:], values)
			continue
		}
		if base == "bun" && len(words) > 1 && words[1] == "x" {
			base, words = "bunx", words[1:]
		}
		if subcommands, ok := runners[base]; ok {
			rest := words[1:]
			if subcommands != nil {
				if len(rest) == 0 || !slices.Contains(subcommands, rest[0]) {
					return launched{name: programName(base), path: word, args: rest}
				}
				rest = rest[1:]
				if base == "uv" && len(rest) > 0 && rest[0] == "run" {
					rest = rest[1:] // uv tool run
				}
			}
			rest = skipOptions(rest, "pcw")
			if len(rest) == 0 {
				return launched{name: programName(base), path: word}
			}
			pkg := packageName(rest[0])
			return launched{name: programName(pkg), path: rest[0], pkg: pkg, args: rest[1:]}
		}
		if interpreters[base] || strings.HasPrefix(base, "python3.") {
			rest := words[1:]
			if len(rest) > 0 && (rest[0] == "run" && (base == "deno" || base == "bun")) {
				rest = rest[1:]
			}
			// python -m module runs the module; -c and -e, the code that
			// follows them, which is no program's.
			for i := 0; i < len(rest) && strings.HasPrefix(rest[i], "-"); i++ {
				switch {
				case rest[i] == "-m" && i+1 < len(rest):
					return launched{name: programName(rest[i+1]), path: rest[i+1], args: rest[i+2:]}
				case rest[i] == "-c" || rest[i] == "-e" || rest[i] == "--eval":
					return launched{name: programName(base), path: word}
				}
			}
			rest = skipOptions(rest, "rC")
			if len(rest) == 0 {
				return launched{name: programName(base), path: word}
			}
			words = rest
			continue
		}
		return launched{name: programName(base), path: word, args: words[1:]}
	}
	return launched{}
}

// isAssignment reports whether a word sets a variable for the command.
func isAssignment(word string) bool {
	name, _, ok := strings.Cut(word, "=")
	return ok && name != "" && !strings.HasPrefix(name, "-") && strings.IndexFunc(name, func(r rune) bool {
		return !(r == '_' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9')
	}) < 0
}

// skipOptions skips the options words start with; values are the letters
// of the short options that take a value, which follows them.
func skipOptions(words []string, values string) []string {
	for len(words) > 0 && strings.HasPrefix(words[0], "-") && words[0] != "-" {
		option := words[0]
		words = words[1:]
		if option == "--" {
			break
		}
		if len(option) == 2 && strings.ContainsRune(values, rune(option[1])) && len(words) > 0 {
			words = words[1:]
		}
	}
	return words
}

// packageName is a package without its version: @openai/codex@latest is
// @openai/codex, aider-chat==0.80 aider-chat.
func packageName(spec string) string {
	if at := strings.LastIndexByte(spec, '@'); at > 0 {
		spec = spec[:at]
	}
	if at := strings.IndexAny(spec, "=<>[~!"); at > 0 {
		spec = spec[:at]
	}
	return spec
}

// programName is the name a program, script or package is known by: its
// last element, without the extension a script has.
func programName(path string) string {
	name := path[strings.LastIndexByte(path, '/')+1:]
	for _, ext := range []string{".js", ".mjs", ".cjs", ".ts", ".py", ".exe"} {
		name = strings.TrimSuffix(name, ext)
	}
	return strings.ToLower(name)
}

// commandWords splits a shell's command line into the words of its simple
// commands: quotes and escapes as the shells read them, ; & | between
// commands. Nothing is expanded.
func commandWords(line string) [][]string {
	var commands [][]string
	var words []string
	var word strings.Builder
	inWord := false
	end := func() {
		if inWord {
			words = append(words, word.String())
			word.Reset()
			inWord = false
		}
	}
	next := func() {
		end()
		if len(words) > 0 {
			commands = append(commands, words)
			words = nil
		}
	}
	for i := 0; i < len(line); i++ {
		switch ch := line[i]; ch {
		case ' ', '\t', '\n':
			end()
		case ';', '&', '|', '(', ')':
			next()
		case '\'':
			inWord = true
			closing := strings.IndexByte(line[i+1:], '\'')
			if closing < 0 {
				word.WriteString(line[i+1:])
				i = len(line)
				break
			}
			word.WriteString(line[i+1 : i+1+closing])
			i += closing + 1
		case '"':
			inWord = true
			for i++; i < len(line) && line[i] != '"'; i++ {
				if line[i] == '\\' && i+1 < len(line) && strings.IndexByte("\"\\$`", line[i+1]) >= 0 {
					i++
				}
				word.WriteByte(line[i])
			}
		case '\\':
			inWord = true
			if i+1 < len(line) {
				i++
				word.WriteByte(line[i])
			}
		default:
			inWord = true
			word.WriteByte(ch)
		}
	}
	next()
	return commands
}
