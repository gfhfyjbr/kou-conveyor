package canvas

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/worktree"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// A terminal node is a shell of the server's (cmd/internal/terminal), which
// the page shows as the sidebar's terminals are shown, and which the node's
// preset may have run a program: a command, or an agent's harness — Claude
// Code, Codex — that kou-canvas launch starts in the shell's place. The
// engine follows what the shell prints: its prompts and commands, which the
// shell integration marks (OSC 133), say when it is idle and what a command
// printed, its output; a harness says it through its hooks
// (kou-canvas hook), or by falling quiet. An agent the user runs in the
// shell is found and followed as a harness is (detect.go).

// instance is what runs for a node.
type instance interface {
	// deliver gives the node a message.
	deliver(m *Message) error
	// read reads what the node shows: screen, tail, output or answer.
	read(what string, lines int) (string, error)
	// stop stops following the node; what runs for it goes on.
	stop()
}

const (
	// launchFallback is how long a shell that marks no prompt is waited
	// for before its program is launched all the same.
	launchFallback = 1500 * time.Millisecond
	// maxCapture bounds a command's output kept.
	maxCapture = 256 << 10
	// unanswered is how long a message typed into a shell may start no
	// command before the shell counts as idle again.
	unanswered = 2 * time.Second
	// hooksSilent is how long a harness that should say it started, but
	// does not, starts before it counts as idle.
	hooksSilent = 20 * time.Second
	// startedBy is how soon a harness given a message starts its turn: a
	// turn that starts later is the user's.
	startedBy = 10 * time.Second
	// echoWindow is how long after something is typed what the terminal
	// shows is taken for its echo, not for the program at work.
	echoWindow = 500 * time.Millisecond
	// activityGap is the longest pause of output that goes on; sustained,
	// how long it goes on before an agent counts as at work.
	activityGap = 2 * time.Second
	sustained   = 1500 * time.Millisecond
	// agentIdle is how long an agent shows nothing before it counts as
	// waiting for its next prompt, when its preset does not say.
	agentIdle = 4 * time.Second
	// hooksQuiet is how long a harness whose hooks said it is at work may
	// show nothing before it counts as idle: a turn interrupted says
	// nothing more.
	hooksQuiet = 12 * time.Second
	// quietAfter is how long a command shows nothing before it is quiet.
	quietAfter = 5 * time.Second
	// probeEvery is how often the program in the foreground of a shell is
	// looked at.
	probeEvery = 2 * time.Second
)

// agentInput is how an agent found in a shell without a preset of its own
// takes text: as a paste, then Enter.
var agentInput = plugin.CanvasInput{Paste: "bracketed", Submit: "\r", SubmitDelayMS: 100}

type terminalInstance struct {
	c       *Canvas
	id      string
	session *terminal.Session
	def     harnessDef
	// shell: the node is a shell or a command, whose commands' outputs are
	// its own; else it runs a harness.
	shell bool
	ready *regexp.Regexp

	signal   chan struct{}
	quit     chan struct{}
	stopOnce sync.Once

	mu       sync.Mutex
	queue    []terminal.Event
	cancel   func()
	answer   string
	launchAs string // what to type at the first prompt
	// input is how the agent found in the shell takes text.
	input *plugin.CanvasInput

	lastOutput atomic.Int64
	delivered  atomic.Int64
	commandAt  atomic.Int64
	startedAt  time.Time

	// launching is set once the launch was typed: the next command is the
	// harness.
	launching atomic.Bool

	// The loop's own.
	prompted  bool
	harnessUp bool
	inLine    bool
	line      []byte
	capturing bool
	capture   []byte
	command   string
	// marksStart: the shell marked where a command starts (OSC 133 C) at
	// least once; else its outputs are told from what follows its prompt.
	marksStart bool
	// activeSince is when the output that goes on started, lastActive
	// when it last came: output that is not the echo of what was typed.
	activeSince, lastActive time.Time
	// agent is the agent found running in the shell, nil for none;
	// interactive says the command that runs is an agent's, whose screens
	// are no command's output; probed is when the foreground was last
	// looked at.
	agent       *found
	interactive bool
	probed      time.Time
}

// startTerminal binds a terminal node to its shell, or starts one when
// start is set.
func (c *Canvas) startTerminal(n *Node, start bool, resume bool) {
	def, ok := c.e.catalog(c.ws).harness(n.Plugin, n.Preset)
	if !ok {
		c.setStatus(n.ID, StateError, fmt.Sprintf("no preset %q: is its plugin on?", presetName(n.Plugin, n.Preset)))
		return
	}
	terminals := c.e.o.Host.Terminals()
	var session *terminal.Session
	if n.Runtime.Terminal != "" {
		if s, err := terminals.Get(n.Runtime.Terminal); err == nil {
			if exited, _ := s.Exited(); !exited {
				session = s
			}
		}
	}
	fresh := session == nil
	if fresh {
		if !start {
			detail := "not started"
			if n.Runtime.Terminal != "" {
				detail = "the shell ended"
			}
			c.setStatus(n.ID, StateStopped, detail)
			return
		}
		c.setStatus(n.ID, StateStarting, "")
		dir, err := c.terminalDir(n)
		if err != nil {
			c.setStatus(n.ID, StateError, err.Error())
			return
		}
		cols, rows := gridOf(n.W, n.H)
		session, err = terminals.Start(terminal.Spec{
			Workspace: c.ws.ID, Dir: dir, Theme: true, Cols: cols, Rows: rows,
			Owner: ownerOf(c.ws.ID, c.id, n.ID), Env: c.e.env(c, n.ID, terminalScope(n), n.Runtime.Epoch),
		})
		switch {
		case errors.Is(err, terminal.ErrTooMany):
			c.setStatus(n.ID, StateError, "too many terminals: close some first")
			return
		case err != nil:
			c.setStatus(n.ID, StateError, "cannot start a shell: "+err.Error())
			return
		}
		id := session.ID()
		c.setRuntime(n.ID, func(r *Runtime) { r.Terminal = id })
	}
	at := time.Now()
	t := &terminalInstance{
		c: c, id: n.ID, session: session, def: def, shell: !n.harness(),
		signal: make(chan struct{}, 1), quit: make(chan struct{}), startedAt: at,
		activeSince: at, lastActive: at,
	}
	if def.Harness.Ready != "" {
		t.ready, _ = regexp.Compile(def.Harness.Ready)
	}
	t.lastOutput.Store(at.UnixNano())
	if fresh {
		t.launchAs = c.launchLine(n, def, resume)
	}
	c.mu.Lock()
	st := c.states[n.ID]
	if st == nil || c.closed {
		c.mu.Unlock()
		if fresh {
			_ = terminals.Kill(session.ID())
		}
		return
	}
	if old := st.inst; old != nil {
		defer old.stop()
	}
	st.inst, st.terminal = t, session
	state, detail := StateStarting, ""
	var agent *Agent
	if !fresh {
		state = StateIdle
		if info := session.Info(); info.Running != "" && !t.shell {
			// A harness that ran when the server restarted is taken to wait
			// for its next message; its end ends its command.
			detail = info.Running
			t.harnessUp, t.capturing = true, true
			agent = t.presetAgent()
		}
	}
	c.setStatusLocked(n.ID, state, detail)
	c.setAgentLocked(n.ID, agent)
	c.hub.publish(map[string]any{"type": "terminal", "node": n.ID, "terminal": session.Info()})
	c.mu.Unlock()
	t.cancel = session.Observe(t.observe)
	go t.loop(fresh)
}

// terminalScope is the scope of a terminal's token: its access, or none —
// a token all the same, which launches its harness and reports what it
// does.
func terminalScope(n *Node) string {
	if _, ok := scopeRank[n.Access]; ok {
		return n.Access
	}
	return ScopeNone
}

// terminalDir is where a terminal node's shell starts: its worktree, made
// when it has none yet, its cwd, or the workspace.
func (c *Canvas) terminalDir(n *Node) (string, error) {
	if spec, ok := n.configValue("worktree").(map[string]any); ok || n.configBool("worktree", false) {
		name, _ := spec["name"].(string)
		branch, _ := spec["branch"].(string)
		base, _ := spec["base"].(string)
		if name == "" {
			name = worktree.Slug(n.Title)
		}
		if name == "" || !worktree.ValidName(name) {
			name = strings.ReplaceAll(n.ID, "_", "-")
		}
		ctx, cancel := context.WithTimeout(c.e.ctx, 2*time.Minute)
		defer cancel()
		path, made, err := worktree.Ensure(ctx, c.ws.Path, name, branch, base)
		if err != nil {
			return "", fmt.Errorf("the worktree: %w", err)
		}
		c.setRuntime(n.ID, func(r *Runtime) { r.Worktree, r.Branch = path, made })
		return path, nil
	}
	if cwd := strings.TrimSpace(n.configString("cwd")); cwd != "" {
		if !filepath.IsAbs(cwd) {
			cwd = filepath.Join(c.ws.Path, cwd)
		}
		if info, err := os.Stat(cwd); err != nil || !info.IsDir() {
			return "", fmt.Errorf("no such folder: %s", cwd)
		}
		return filepath.Clean(cwd), nil
	}
	if info, err := os.Stat(c.ws.Path); err != nil || !info.IsDir() {
		return "", fmt.Errorf("the workspace's folder is missing")
	}
	return c.ws.Path, nil
}

// gridOf is how many columns and rows a node of a size shows, at the
// page's terminal font.
func gridOf(w, h int) (int, int) {
	cols := (w - 16) * 10 / 78
	rows := (h - 64) / 17
	return min(max(cols, 20), 400), min(max(rows, 5), 200)
}

// launchLine is what is typed at a fresh shell's first prompt: the
// command, or what launches the harness.
func (c *Canvas) launchLine(n *Node, def harnessDef, resume bool) string {
	switch {
	case n.Preset == "command" && n.Plugin == "":
		return strings.TrimSpace(n.configString("command"))
	case !n.harness():
		return ""
	}
	switch def.Harness.Launch {
	case "shell":
		return ""
	case "type":
		spec, err := c.launchSpec(n.ID, resume)
		if err != nil {
			return ""
		}
		// The shell has the canvas's variables, its token among them: the
		// line sets the preset's own alone, and shows no token.
		var env []string
		for _, entry := range spec.Env {
			name, _, _ := strings.Cut(entry, "=")
			if _, own := def.Harness.Env[name]; own {
				env = append(env, shellQuote(entry))
			}
		}
		words := make([]string, 0, len(spec.Argv)+len(env)+1)
		if len(env) > 0 {
			words = append(append(words, "env"), env...)
		}
		for _, arg := range spec.Argv {
			words = append(words, shellQuote(arg))
		}
		return strings.Join(words, " ")
	}
	// The leading space keeps the line out of the shell's history.
	if resume {
		return " kou-canvas launch --resume"
	}
	return " kou-canvas launch"
}

// shellQuote quotes a word for any of the shells.
func shellQuote(word string) string {
	if word != "" && strings.IndexFunc(word, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || strings.ContainsRune("-_./=:,@%+", r))
	}) < 0 {
		return word
	}
	return "'" + strings.ReplaceAll(word, "'", `'"'"'`) + "'"
}

// observe hears the shell; it runs under the shell's lock and must not
// block.
func (t *terminalInstance) observe(ev terminal.Event) {
	t.mu.Lock()
	if len(t.queue) < 8192 || ev.Exit != nil || len(ev.Marks) > 0 {
		t.queue = append(t.queue, ev)
	}
	t.mu.Unlock()
	select {
	case t.signal <- struct{}{}:
	default:
	}
}

func (t *terminalInstance) take() []terminal.Event {
	t.mu.Lock()
	defer t.mu.Unlock()
	events := t.queue
	t.queue = nil
	return events
}

// loop follows the shell until it ends or the node goes.
func (t *terminalInstance) loop(fresh bool) {
	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()
	var fallback <-chan time.Time
	if fresh {
		fallback = time.After(launchFallback)
	} else {
		t.prompted = true
	}
	for {
		select {
		case <-t.quit:
			return
		case <-t.signal:
			for _, ev := range t.take() {
				if t.handle(ev) {
					return
				}
			}
		case <-fallback:
			fallback = nil
			if !t.prompted {
				t.prompted = true
				t.prompt()
			}
		case <-ticker.C:
			t.heuristics()
		}
	}
}

// handle takes an event of the shell in; true when the shell ended.
func (t *terminalInstance) handle(ev terminal.Event) bool {
	if ev.Output != nil {
		at := time.Now()
		t.lastOutput.Store(at.UnixNano())
		t.noteOutput(at)
		prev := 0
		for _, mark := range ev.Marks {
			at := min(max(mark.At, prev), len(ev.Output))
			t.segment(ev.Output[prev:at])
			t.mark(mark)
			prev = at
		}
		t.segment(ev.Output[prev:])
	}
	if ev.Meta != nil {
		t.c.terminalMeta(t.id, t.session)
	}
	if ev.Exit != nil {
		t.c.shellEnded(t.id, *ev.Exit)
		t.c.terminalMeta(t.id, t.session)
		return true
	}
	return false
}

// noteOutput takes in output that came at at: unless it is the echo of
// what was typed, it shows the program at work.
func (t *terminalInstance) noteOutput(at time.Time) {
	if at.Sub(t.session.LastInput()) < echoWindow {
		return
	}
	if at.Sub(t.lastActive) > activityGap {
		t.activeSince = at
	}
	t.lastActive = at
}

// working reports whether the program has shown output a while, and goes
// on showing it.
func (t *terminalInstance) working(at time.Time) bool {
	return at.Sub(t.lastActive) < activityGap && t.lastActive.Sub(t.activeSince) >= sustained
}

// quietFor is how long the program has shown nothing, nor been given a
// message.
func (t *terminalInstance) quietFor(at time.Time) time.Duration {
	return at.Sub(latest(t.lastActive, time.Unix(0, t.delivered.Load())))
}

func latest(a, b time.Time) time.Time {
	if a.After(b) {
		return a
	}
	return b
}

func (t *terminalInstance) segment(p []byte) {
	if len(p) == 0 {
		return
	}
	if t.capturing {
		t.capture = append(t.capture, p...)
		if len(t.capture) > 2*maxCapture {
			t.capture = append(t.capture[:0:0], t.capture[len(t.capture)-maxCapture:]...)
		}
	}
	if t.inLine && len(t.line) < 16<<10 {
		t.line = append(t.line, p...)
	}
}

// mark takes a semantic prompt mark in.
func (t *terminalInstance) mark(mark terminal.Mark) {
	switch mark.Kind {
	case 'A':
		if !t.prompted {
			t.prompted = true
			t.prompt()
		}
	case 'B':
		t.inLine, t.line = true, t.line[:0]
	case 'C':
		t.inLine = false
		t.marksStart = true
		command := strings.TrimSpace(terminal.LastLines(terminal.Render(t.line), 1))
		t.capturing, t.capture, t.command = true, t.capture[:0], command
		t.commandAt.Store(time.Now().UnixNano())
		t.interactive = false
		switch {
		case t.launching.Swap(false):
			t.harnessUp = true
			t.c.setAgent(t.id, t.presetAgent())
			t.c.setStatus(t.id, StateStarting, "starting "+t.def.Harness.Title)
		case t.harnessUp:
		default:
			if f, ok := agentInCommand(t.c.e.catalog(t.c.ws), command); ok {
				t.foundAgent(f, true)
				return
			}
			t.c.setStatus(t.id, StateBusy, command)
		}
	case 'D':
		if t.interactive {
			// An agent's session ended: what it showed is no command's
			// output.
			t.interactive, t.capturing = false, false
			title := "the agent"
			if t.agent != nil {
				title = t.agent.agent.Title
			}
			t.dropAgent()
			t.c.unanswered(t.id, title+" exited")
			t.c.setStatus(t.id, StateIdle, "")
			return
		}
		if !t.capturing {
			// A shell that never marks where its commands start (bash
			// before 4.4 has no PS0): after its prompt come the command's
			// line, then what the command printed.
			if t.shell && t.inLine && !t.marksStart {
				t.inLine = false
				if command, text, ok := unmarkedCommand(t.line); ok {
					t.command = command
					t.output(text, mark.Code)
				}
			}
			return
		}
		t.capturing = false
		code := mark.Code
		if t.harnessUp {
			t.harnessUp = false
			detail := t.def.Harness.Title + " exited"
			if code > 0 {
				detail += " (" + strconv.Itoa(code) + ")"
			}
			t.c.unanswered(t.id, detail)
			t.c.setAgent(t.id, nil)
			t.c.setStatus(t.id, StateExited, detail)
			return
		}
		if !t.shell {
			t.c.unanswered(t.id, "the command it started ended")
			t.c.setStatus(t.id, StateIdle, "")
			return
		}
		t.output(strings.Trim(terminal.Render(t.capture), "\n"), code)
	}
}

// presetAgent is the agent a harness's node runs, once launched.
func (t *terminalInstance) presetAgent() *Agent {
	agent := &Agent{ID: presetName(t.def.Plugin, t.def.Harness.ID), Title: t.def.Harness.Title}
	if len(t.def.Harness.Command) > 0 {
		agent.Program = launchedBy(t.def.Harness.Command).name
	}
	return agent
}

// foundAgent takes in an agent found running in the shell: the node is
// its, and waits for its prompts; the message that started it, if one did,
// is done with. started: it starts now.
func (t *terminalInstance) foundAgent(f found, started bool) {
	if f.input == nil {
		f.input = &agentInput
	}
	if f.idle <= 0 {
		f.idle = agentIdle
	}
	fresh := t.agent == nil
	t.agent, t.interactive = &f, true
	t.mu.Lock()
	t.input = f.input
	t.mu.Unlock()
	t.c.setAgent(t.id, f.agent)
	if !fresh {
		return
	}
	t.c.unanswered(t.id, "it started "+f.agent.Title+" there, which waits for its prompt")
	if started {
		t.activeSince, t.lastActive = time.Now(), time.Now()
		t.c.setStatus(t.id, StateStarting, "starting "+f.agent.Title)
	} else if !t.working(time.Now()) {
		t.c.setStatus(t.id, StateIdle, "")
	}
}

// dropAgent lets go of the agent found in the shell: it ended.
func (t *terminalInstance) dropAgent() {
	t.agent = nil
	t.mu.Lock()
	t.input = nil
	t.mu.Unlock()
	t.c.setAgent(t.id, nil)
}

// output puts what the shell's last command printed out on the port out.
func (t *terminalInstance) output(text string, code int) {
	if len(text) > maxCapture {
		text = "…" + text[len(text)-maxCapture:]
	}
	data := map[string]any{"command": t.command}
	if code >= 0 {
		data["exit_code"] = code
	}
	t.c.commandOutput(t.id, Output{Port: "out", Title: t.command, Text: text, Data: jsonOf(data)})
	t.c.setStatus(t.id, StateIdle, "")
}

// unmarkedCommand splits what a shell that does not mark where commands
// start showed between its prompt and the command's end: the command's
// line, then what it printed. An empty line, or one Ctrl-C ended, ran
// nothing.
func unmarkedCommand(line []byte) (command, text string, ok bool) {
	command, text, _ = strings.Cut(terminal.Render(line), "\n")
	command = strings.TrimSpace(command)
	if command == "" || strings.HasSuffix(command, "^C") {
		return "", "", false
	}
	return command, strings.Trim(text, "\n"), true
}

// prompt is the shell's first prompt: its program is launched.
func (t *terminalInstance) prompt() {
	t.mu.Lock()
	line := t.launchAs
	t.mu.Unlock()
	if line == "" {
		t.c.setStatus(t.id, StateIdle, "")
		return
	}
	t.launching.Store(!t.shell)
	go func() {
		err := t.session.Paste(line, terminal.PasteOptions{Submit: "\r", SubmitDelay: 30 * time.Millisecond})
		if err != nil {
			t.c.setStatus(t.id, StateError, "cannot type the command: "+err.Error())
		}
	}()
}

// heuristics tell what the shell does where nothing says it.
func (t *terminalInstance) heuristics() {
	at := time.Now()
	if at.Sub(t.probed) >= probeEvery {
		t.probed = at
		t.probe()
	}
	st := t.c.status(t.id)
	switch {
	case t.agent != nil:
		t.followAgent(at, st)
	case t.shell:
		t.followShell(at, st)
	default:
		t.followHarness(at, st)
	}
}

// probe looks at the program in the foreground of the shell: an agent
// that runs there is found, one that ended is let go.
func (t *terminalInstance) probe() {
	if t.harnessUp {
		return // the preset says what runs
	}
	program, running := t.session.Foreground()
	if !running {
		if t.agent != nil {
			// It ended; the shell's next marks may say so after.
			title := t.agent.agent.Title
			t.dropAgent()
			t.c.unanswered(t.id, title+" exited")
			t.c.setStatus(t.id, StateIdle, "")
		}
		return
	}
	if t.agent != nil && t.agent.agent.PID == program.PID {
		return
	}
	if len(program.Args) == 1 && strings.Contains(program.Args[0], " ") {
		// A program that retitled itself (npm exec …) shows its title.
		program.Args = strings.Fields(program.Args[0])
	}
	f, ok := agentInProgram(t.c.e.catalog(t.c.ws), program)
	switch {
	case ok:
		t.foundAgent(f, false)
	case t.agent != nil && t.agent.agent.PID != 0:
		// The agent gave its place to another program.
		t.dropAgent()
		t.c.setStatus(t.id, StateBusy, program.Name)
	}
}

// followShell tells what a shell does where its marks do not.
func (t *terminalInstance) followShell(at time.Time, st Status) {
	if st.State != StateBusy {
		return
	}
	delivered := t.delivered.Load()
	if st.Detail == "delivered" {
		// A message that started no command went to whatever reads the
		// shell's input: no telling when that is done.
		if t.commandAt.Load() < delivered && at.Sub(time.Unix(0, delivered)) > unanswered {
			t.c.unanswered(t.id, "it started no command: what runs in the shell read it")
			t.c.setStatus(t.id, StateIdle, "")
		}
		return
	}
	since := latest(t.lastActive, time.Unix(0, max(t.commandAt.Load(), delivered)))
	t.c.setQuiet(t.id, at.Sub(since) > quietAfter)
}

// followHarness tells what a harness does where its hooks do not.
func (t *terminalInstance) followHarness(at time.Time, st Status) {
	h := t.def.Harness
	quiet := t.quietFor(at)
	switch st.State {
	case StateBusy, StateStarting:
		idle := time.Duration(h.IdleMS) * time.Millisecond
		if idle <= 0 {
			idle = 4 * time.Second
		}
		switch {
		case slices.Contains(h.Status, "idle") && quiet > idle,
			t.ready != nil && quiet > 800*time.Millisecond && t.ready.MatchString(t.session.Tail(10)):
			switch {
			case st.State == StateBusy && h.Output == "screen":
				// What its screen shows once it falls idle is its answer.
				screen := strings.Trim(t.session.Tail(t.session.Info().Rows), "\n")
				t.c.quietAgent(t.id, &Output{Port: "out", Title: h.Title, Text: screen}, "")
			case st.State == StateBusy && h.Output == "none":
				t.c.quietAgent(t.id, nil, h.Title+"'s answer is on its screen alone: read it")
			default:
				t.c.quietAgent(t.id, nil, "")
			}
		case st.State == StateBusy && slices.Contains(h.Status, "hooks") && quiet > hooksQuiet && at.Sub(st.Since) > hooksQuiet:
			// A turn interrupted says nothing more: the harness waits again.
			t.c.quietAgent(t.id, nil, h.Title+" stopped before it answered")
		case st.State == StateStarting && at.Sub(st.Since) > hooksSilent && quiet > 5*time.Second:
			// A harness whose hooks never said it started is taken to have.
			t.c.setStatus(t.id, StateIdle, "no word from its hooks")
		}
	case StateIdle, StateWaiting:
		if t.resumed(at, st) {
			t.c.setStatus(t.id, StateBusy, "")
		}
	}
}

// resumed reports whether a harness that waits is at work again where
// nothing said it: the user typed to it, or answered its question. What it
// shows must have begun after it fell to waiting — the end of the turn it
// just finished is no new work; and a harness whose hooks say when its
// turns start is at work again by them alone, save after a question.
func (t *terminalInstance) resumed(at time.Time, st Status) bool {
	if !t.harnessUp || !t.working(at) || !t.activeSince.After(st.Since) {
		return false
	}
	return st.State == StateWaiting || !slices.Contains(t.def.Harness.Status, "hooks")
}

// followAgent tells what an agent found in the shell does: at work while
// what it shows goes on, waiting once it falls quiet — when a message came
// to it, its screen is its answer.
func (t *terminalInstance) followAgent(at time.Time, st Status) {
	quiet := t.quietFor(at)
	switch st.State {
	case StateStarting:
		if quiet > sustained && at.Sub(st.Since) > time.Second {
			t.c.setStatus(t.id, StateIdle, "")
		}
	case StateIdle, StateWaiting:
		if t.working(at) {
			t.c.setStatus(t.id, StateBusy, "")
		}
	case StateBusy:
		if quiet > t.agent.idle {
			screen := strings.Trim(t.session.Tail(t.session.Info().Rows), "\n")
			t.c.quietAgent(t.id, &Output{Port: "out", Title: t.agent.agent.Title, Text: screen}, "")
		}
	}
}

func (t *terminalInstance) markDelivered() { t.delivered.Store(time.Now().UnixNano()) }

func (t *terminalInstance) deliver(m *Message) error {
	options := terminal.PasteOptions{Bracketed: true, Submit: "\r", SubmitDelay: 60 * time.Millisecond}
	t.mu.Lock()
	input := t.input
	t.mu.Unlock()
	if input == nil {
		input = t.def.Harness.Input
	}
	if input != nil {
		if input.Paste == "type" {
			options.Bracketed = false
		}
		if input.Newline == "lf" {
			options.Newline = "\n"
		}
		if input.Submit != "" {
			options.Submit = input.Submit
		}
		if input.SubmitDelayMS > 0 {
			options.SubmitDelay = time.Duration(input.SubmitDelayMS) * time.Millisecond
		}
	}
	if m.NoSubmit {
		options.Submit = ""
	}
	t.markDelivered()
	return t.session.Paste(m.Text, options)
}

func (t *terminalInstance) keys(names []string) error { return t.session.Keys(names...) }

func (t *terminalInstance) read(what string, lines int) (string, error) {
	switch what {
	case "output", "answer":
		t.mu.Lock()
		answer := t.answer
		t.mu.Unlock()
		if answer != "" && !t.shell {
			return answer, nil
		}
		return t.c.lastOutput(t.id), nil
	case "screen":
		return t.session.Tail(t.session.Info().Rows), nil
	}
	if lines <= 0 {
		lines = 80
	}
	return t.session.Tail(lines), nil
}

// setAnswer keeps what a harness's hooks said it answered.
func (t *terminalInstance) setAnswer(text string) {
	t.mu.Lock()
	t.answer = text
	t.mu.Unlock()
}

func (t *terminalInstance) stop() {
	t.stopOnce.Do(func() {
		if t.cancel != nil {
			t.cancel()
		}
		close(t.quit)
	})
}

// relaunch launches a terminal's program again in its shell, if the shell
// is there and the program is not running.
func (t *terminalInstance) relaunch(n *Node, resume bool) bool {
	if exited, _ := t.session.Exited(); exited {
		return false
	}
	line := t.c.launchLine(n, t.def, resume)
	if line == "" {
		return true
	}
	t.mu.Lock()
	t.launchAs = line
	t.mu.Unlock()
	t.launching.Store(!t.shell)
	go func() {
		_ = t.session.Paste(line, terminal.PasteOptions{Submit: "\r", SubmitDelay: 30 * time.Millisecond})
	}()
	return true
}

// quietAgent is a terminal's agent quiet after a turn: out, when there is
// one, answers the message delivered to it last — else reason says why
// there is no answer —, and it takes the next message.
func (c *Canvas) quietAgent(id string, out *Output, reason string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[id]
	if st == nil {
		return
	}
	// The answer first: a state of idle has the next message delivered,
	// whose the node's next answer is.
	switch {
	case out != nil:
		c.answerLocked(id, *out, []*Message{st.cause}, "")
	case reason != "":
		c.unansweredLocked(id, reason)
	}
	c.setStatusLocked(id, StateIdle, "")
}

// shellEnded is a node's shell that ended: what waits for its answer is
// told, and the exit goes out on its port exit.
func (c *Canvas) shellEnded(id string, code int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	exit := "exit " + strconv.Itoa(code)
	if st := c.states[id]; st != nil && direct(st.cause) {
		c.settleLocked(st.cause.ID, "", "its shell ended ("+exit+")")
	}
	c.setAgentLocked(id, nil)
	c.setStatusLocked(id, StateExited, exit)
	c.emitLocked(id, Output{Port: "exit", Title: "Exited", Text: exit, Data: jsonOf(map[string]any{"code": code})})
}

// terminalMeta tells the pages what a node's shell says of itself.
func (c *Canvas) terminalMeta(id string, session *terminal.Session) {
	info := session.Info()
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.states[id] != nil {
		c.hub.publish(map[string]any{"type": "terminal", "node": id, "terminal": info})
	}
}

// lastOutput is a node's last output.
func (c *Canvas) lastOutput(id string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	if st := c.states[id]; st != nil {
		return st.output
	}
	return ""
}
