// Command kou-conveyor-tui is a terminal front-end for kou-conveyor-runner.
// The runner owns execution and durable sessions; this process owns presentation.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/muesli/termenv"
	"golang.org/x/term"
)

type options struct {
	cockpit.Options
	session, historyFile, model, thinking, prompt string
	// preferences holds the effort, shared with the web cockpit, and the
	// layout the cockpit starts in.
	preferences string
	noColor     bool
	// compact lays the cockpit out below the command instead of in a screen
	// of its own; see compact.go.
	compact bool
	// latest resumes the most recent session; pick opens the session list.
	latest, pick bool
}

func parseOptions(args []string, output io.Writer) (options, error) {
	var o options
	f := flag.NewFlagSet("kou-conveyor-tui", flag.ContinueOnError)
	f.SetOutput(output)
	f.StringVar(&o.Workspace, "workspace", ".", "agent workspace")
	f.StringVar(&o.SessionDir, "session-directory", "", "runner sessions (default: <workspace>/.harness/sessions)")
	f.StringVar(&o.session, "session", "", "resume a session by ID or unique ID prefix")
	f.BoolVar(&o.latest, "continue", false, "resume the most recently updated session")
	f.BoolVar(&o.pick, "resume", false, "choose a saved session to resume")
	f.StringVar(&o.SettingsFile, "config", "", "connection settings file (default: KOU_CONVEYOR_CONFIG or the user config directory)")
	f.StringVar(&o.historyFile, "history-file", "", "prompt history (default: <workspace>/.harness/ui-history.json)")
	f.StringVar(&o.Runner, "runner", "", "runner executable (also KOU_CONVEYOR_RUNNER)")
	f.StringVar(&o.model, "model", "", "model ID; otherwise use runner environment/default")
	f.StringVar(&o.Provider, "provider", "", "provider; otherwise use runner environment/default")
	f.StringVar(&o.thinking, "thinking-level", "", "effort: low, medium, high, xhigh, max (default: the last chosen in either cockpit, or high)")
	f.StringVar(&o.prompt, "p", "", "submit an initial prompt; one-shot mode when not in a terminal")
	f.StringVar(&o.LogDir, "log-directory", "", "runner JSONL log directory")
	f.BoolVar(&o.noColor, "no-color", false, "disable colors (also NO_COLOR)")
	inline := f.Bool("inline", false, "stay below the command, printing the transcript into the terminal's scrollback (default: the layout used last)")
	compact := f.Bool("compact", false, "the former name of -inline")
	fullscreen := f.Bool("fullscreen", false, "take a screen of your own, with the mouse")
	f.DurationVar(&o.Heartbeat, "tool-heartbeat-interval", 10*time.Minute, "runner tool-wait heartbeat (0 disables)")
	f.Usage = func() {
		fmt.Fprint(output, "Usage: kou-conveyor-tui [options]\n\nInteractive agent cockpit. Ctrl-K: commands · Ctrl-S: sessions · Ctrl-R: history · /settings: connection · /help: keys\n\n")
		f.PrintDefaults()
	}
	if err := f.Parse(args); err != nil {
		return o, err
	}
	if f.NArg() != 0 {
		return o, errors.New("unexpected arguments; use -p for an initial prompt")
	}
	if o.Heartbeat < 0 {
		return o, errors.New("tool heartbeat interval must not be negative")
	}
	if o.thinking != "" && !cockpit.ValidThinkingLevel(o.thinking) {
		return o, errors.New("thinking-level must be low, medium, high, xhigh or max")
	}
	if o.session != "" && !cockpit.ValidSessionID(o.session) {
		return o, errors.New("session must contain only letters, digits and dashes")
	}
	if n := boolCount(o.session != "", o.latest, o.pick); n > 1 {
		return o, errors.New("use only one of -session, -continue and -resume")
	}
	below := *inline || *compact
	if below && *fullscreen {
		return o, errors.New("use only one of -inline and -fullscreen")
	}
	if o.pick && o.prompt != "" {
		return o, errors.New("-resume chooses a session interactively; use -session or -continue with -p")
	}
	if o.Provider != "" {
		switch o.Provider {
		case "openai", "anthropic", "openai-codex", "openrouter", "fireworks", "ollama":
		default:
			return o, fmt.Errorf("unsupported provider %q", o.Provider)
		}
	}
	var err error
	if strings.TrimSpace(o.Workspace) == "" {
		return o, errors.New("workspace must not be empty")
	}
	o.Workspace, err = filepath.Abs(o.Workspace)
	if err != nil {
		return o, err
	}
	info, err := os.Stat(o.Workspace)
	if err != nil {
		return o, err
	}
	if !info.IsDir() {
		return o, errors.New("workspace is not a directory")
	}
	// Explicit relative paths have exactly the same meaning as runner CLI paths:
	// relative to the invoking directory, not to -workspace.
	for _, field := range []struct {
		p   *string
		def string
	}{
		{&o.SessionDir, filepath.Join(o.Workspace, ".harness", "sessions")},
		{&o.historyFile, filepath.Join(o.Workspace, ".harness", "ui-history.json")},
	} {
		if *field.p == "" {
			*field.p = field.def
		}
		*field.p, err = filepath.Abs(*field.p)
		if err != nil {
			return o, err
		}
	}
	if o.LogDir != "" {
		o.LogDir, err = filepath.Abs(o.LogDir)
		if err != nil {
			return o, err
		}
	}
	if o.SettingsFile == "" {
		// Without a home directory there is nowhere to keep settings; the
		// environment still configures runs.
		o.SettingsFile, _ = cockpit.SettingsPath()
	} else if o.SettingsFile, err = filepath.Abs(o.SettingsFile); err != nil {
		return o, err
	}
	o.preferences = cockpit.PreferencesPath(o.SettingsFile)
	if o.thinking == "" {
		o.thinking = cockpit.LoadPreferences(o.preferences).Effort
	}
	if o.thinking == "" {
		o.thinking = "high"
	}
	o.compact = below || !*fullscreen && cockpit.LoadPreferences(o.preferences).Layout == cockpit.LayoutCompact
	switch {
	case o.latest:
		list, err := cockpit.ListSessions(o.SessionDir)
		if err != nil {
			return o, err
		}
		// The most recent, whether pinned or not.
		var newest time.Time
		for _, s := range list {
			if o.session == "" || s.UpdatedAt.After(newest) {
				o.session, newest = s.ID, s.UpdatedAt
			}
		}
	case o.session != "":
		// A prefix names an existing session; anything else is a new one.
		if id, err := cockpit.FindSession(o.SessionDir, o.session); err == nil {
			o.session = id
		} else if !errors.Is(err, os.ErrNotExist) {
			return o, err
		}
	}
	return o, nil
}

func boolCount(values ...bool) int {
	n := 0
	for _, v := range values {
		if v {
			n++
		}
	}
	return n
}

func main() { os.Exit(runCLI(os.Args[1:])) }

func runCLI(args []string) int {
	o, err := parseOptions(args, os.Stderr)
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-tui:", err)
		return 2
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	o.Runner, err = cockpit.LocateRunner(o.Runner)
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-tui:", err)
		return 1
	}
	interactive := term.IsTerminal(int(os.Stdin.Fd())) && term.IsTerminal(int(os.Stdout.Fd())) && os.Getenv("TERM") != "dumb"
	if !interactive {
		if strings.TrimSpace(o.prompt) == "" {
			fmt.Fprintln(os.Stderr, "kou-conveyor-tui needs a terminal; use -p for one-shot mode")
			return 2
		}
		if err := runHeadless(ctx, o, os.Stdout, os.Stderr); err != nil {
			fmt.Fprintln(os.Stderr, "kou-conveyor-tui:", err)
			if ctx.Err() != nil {
				return 130
			}
			return 1
		}
		return 0
	}
	if o.noColor || os.Getenv("NO_COLOR") != "" {
		o.noColor = true
		lipgloss.SetColorProfile(termenv.Ascii)
	}
	m := newModel(ctx, o)
	// Pictures of pasted images: the kitty graphics protocol where the
	// terminal speaks it, half blocks or descriptions elsewhere.
	mode, tmux := detectGraphics(os.Getenv, o.noColor)
	m.graphics = newGraphics(mode, tmux, os.Stdout)
	if mode == graphicsKitty {
		m.graphics.start()
	}
	if mode == graphicsBlocks && !lipgloss.HasDarkBackground() {
		blockBackground = 0xf0f0
	}
	programOptions := []tea.ProgramOption{tea.WithReportFocus(), tea.WithContext(ctx)}
	if !o.compact {
		// All motion, not only drags: the pointer's place decides what lights up.
		programOptions = append(programOptions, tea.WithAltScreen(), tea.WithMouseAllMotion())
	}
	program := tea.NewProgram(m, programOptions...)
	_, err = program.Run()
	m.releasePictures()
	// The pointer shape outlives the program in the terminals that set it.
	fmt.Fprint(os.Stdout, ansi.SetPointerShape("default"))
	if m.compact && !m.fresh && err == nil {
		// The transcript stays in the scrollback; say how to come back to it.
		fmt.Fprintf(os.Stdout, "%s\n", m.styles.faint.Render("kou-conveyor-tui -session "+m.sessionID+" resumes this session"))
	}
	// Never leave a runner behind, including on SIGTERM, EOF or a render failure.
	m.shutdown()
	if errors.Is(err, tea.ErrProgramPanic) {
		fmt.Fprintln(os.Stderr, "kou-conveyor-tui: internal error; any run in progress was stopped")
		return 1
	}
	if err != nil && !errors.Is(err, tea.ErrProgramKilled) {
		fmt.Fprintln(os.Stderr, "kou-conveyor-tui:", err)
		return 1
	}
	if ctx.Err() != nil {
		return 130
	}
	return 0
}
