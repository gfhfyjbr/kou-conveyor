package main

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"
	"uuid"

	"github.com/atotto/clipboard"
	"github.com/charmbracelet/bubbles/textarea"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type runState int

const (
	idle runState = iota
	running
	stopping
)

// Every asynchronous result carries the generation it was started for. A
// result from an older run, load or notice is dropped, so late messages can
// never leak into the session the user is looking at now.
type (
	linesMsg struct {
		gen   int
		lines []cockpit.Line
		done  bool
		err   error
	}
	tickMsg   struct{ gen int }
	loadedMsg struct {
		gen   int
		id    string
		tr    *cockpit.Transcript
		meta  cockpit.Meta
		busy  bool // another process is running the session
		quiet bool // a refresh of the session on screen
		err   error
	}
	watchMsg struct {
		gen  int
		size int64
		busy bool
	}
	sessionsMsg struct {
		gen  int
		list []cockpit.SessionInfo
		err  error
		keep string // session to keep the cursor on
	}
	noticeMsg struct{ gen int }
	prefsMsg  struct{}
	copiedMsg struct {
		text string
		what string
		err  error
	}
	submitMsg struct{}
	// modelsMsg carries the models the connection lists, for the models
	// picker.
	modelsMsg struct {
		gen     int
		catalog cockpit.Catalog
	}
)

type outcome struct {
	kind string // done, failed or stopped
	at   time.Time
}

type notice struct {
	text  string
	level string // info, warn, error
	gen   int
}

type uiModel struct {
	ctx    context.Context
	opt    options
	styles styles

	width, height int
	ready         bool
	input         textarea.Model
	view          viewport.Model

	sessionID string
	fresh     bool // not persisted by the runner yet
	external  bool // another process is running this session; it is watched
	meta      cockpit.Meta
	tr        *cockpit.Transcript
	loading   bool
	loadGen   int

	state   runState
	job     *cockpit.Job
	jobGen  int
	started time.Time
	frame   int
	last    outcome
	diag    string // last runner stderr line

	expanded  map[string]bool
	expandAll bool
	cache     map[string]block
	spans     []entrySpan
	starters  map[int]string // welcome-screen line → starter prompt
	follow    bool
	unseen    int
	shown     int // entries rendered at the last refresh

	picker     *picker
	pickerGen  int
	form       *settingsForm
	formGen    int
	conn       cockpit.Connection // what the next run connects to, for display
	note       notice
	noteGen    int
	osc52      string // clipboard escape to emit with the next frame
	history    []historyEntry
	historyPos int // -1 when not browsing history
	draft      string
	thinking   string
	quitArmed  time.Time
	autoSubmit bool

	// Esc Esc: what the last lone esc announced, and when.
	escArmed time.Time
	escKind  string
	// pluginCommands are the slash commands the workspace's plugins add;
	// pluginsSeen fingerprints where the plugins came from when read.
	pluginCommands []cockpit.PluginCommand
	pluginsSeen    string

	// compaction is set while a run compacts the conversation: it holds how
	// many entries the transcript had when the run started.
	compaction *int

	// edit is the prompt being edited; editAfterStop opens the last one once
	// the run that is stopping has stopped.
	edit          *editState
	editAfterStop bool

	// prefsSeen is the preferences file as last read or written here; a
	// different one holds an effort chosen elsewhere.
	prefsSeen fs.FileInfo

	// compact lays the cockpit out below the command that started it,
	// printing the transcript into the terminal's scrollback; otherwise it
	// takes a screen of its own. See compact.go.
	compact   bool
	scroll    scrollback
	switching bool // the terminal is changing screens; prints wait for it
	quitting  bool // the last frame: compact mode leaves only the scrollback

	debris debris // pieces of mouse reports Bubble Tea cut in two; see debris.go

	lines     []string   // the transcript as rendered, one entry per line
	px, py    int        // where the pointer is, or -1
	press     *cell      // where the left button went down, while it is down
	sel       *selection // what a drag selects
	scrubbing bool       // the button went down on the scrollbar
	resizing  bool       // the button went down on the changes panel's edge
	toast     string     // shown at the bottom of the screen for a moment
	toastGen  int

	// changes is the panel of what the prompt in view changed; records
	// records it, and tracker the run going on. See changes.go.
	changes    changesPanel
	records    *cockpit.Changes
	tracker    *cockpit.Tracker
	runMessage string // the prompt the run going on runs, by message ID

	// chosen are the models chosen for sessions' next prompts, by session;
	// a session without one runs the model its last prompt ran with. See
	// models.go.
	chosen map[string]string
	// catalog is what the connection listed last, for the model control.
	catalog *cockpit.Catalog

	// queues hold what is written while the agent works, by session; the
	// queue has the keys on prompt queueFocus, or -1. See queue.go.
	queues     map[string]*cockpit.Queue
	queueFocus int
	queueEdit  *queueEdit

	// images are the composer's images, which its text refers to by their
	// labels (see attach.go); draftImages those of the draft set aside
	// while prompt history shows. sent keeps the images of the prompts
	// sent from here, by message ID, for when one comes back.
	images      []*attachment
	draftImages []*attachment
	sent        map[string][]cockpit.Image
	sentOrder   []string
	// graphics shows pictures (see graphics.go), and strip is where the
	// strip above the composer drew each image, for the pointer.
	graphics *graphics
	strip    []stripHit
	// previewArea is where the large picture was drawn last; empty when
	// none was.
	previewArea area
	// viewed are the pictures the agent looked at, under the calls that
	// read them (see viewed.go).
	viewed viewedPictures
}

type block struct {
	rev, width int
	open, live bool
	faded      bool // after a prompt being edited
	editing    bool // the prompt being edited
	lines      []string
	// tall is how many rows a call's picture may take, and pictures how
	// many it takes at the end of the block (viewed.go).
	tall, pictures int
}

type entrySpan struct {
	id         string
	start, end int
	pictures   int // the last rows are a picture's (viewed.go)
}

func newModel(ctx context.Context, o options) *uiModel {
	st := newStyles(o.noColor)
	in := textarea.New()
	in.Placeholder = idlePlaceholder
	in.ShowLineNumbers = false
	in.CharLimit = 100_000
	// The composer's height is set to fit its text; without a maximum the
	// textarea also takes any number of lines.
	in.MaxHeight = 0
	in.SetHeight(minInputs)
	// Line breaks are the model's to insert (see newline); the textarea's own
	// binding would stop at its height.
	in.KeyMap.InsertNewline.SetEnabled(false)
	in.SetPromptFunc(2, func(line int) string {
		if line == 0 {
			return "❯ "
		}
		return "  "
	})
	focused, blurred := textarea.DefaultStyles()
	focused.CursorLine = st.text
	focused.Text = st.text
	focused.Placeholder = st.faint
	focused.Prompt = st.accent
	focused.EndOfBuffer = st.faint
	blurred.Prompt = st.faint
	blurred.Placeholder = st.faint
	in.FocusedStyle, in.BlurredStyle = focused, blurred
	in.Focus()

	vp := viewport.New(80, 20)
	vp.MouseWheelEnabled = true
	vp.MouseWheelDelta = 3
	vp.KeyMap = viewport.KeyMap{}

	history, _ := loadHistory(o.historyFile)
	m := &uiModel{
		ctx: ctx, opt: o, styles: st, input: in, view: vp,
		sessionID: o.session, tr: cockpit.NewTranscript(),
		expanded: make(map[string]bool), cache: make(map[string]block),
		follow: true, history: history, historyPos: -1, thinking: o.thinking,
		px: -1, py: -1, compact: o.compact, queueFocus: -1,
		// main picks what the terminal shows; tests get descriptions.
		graphics: newGraphics(graphicsText, false, io.Discard),
	}
	if m.sessionID == "" {
		m.sessionID, m.fresh = uuid.New().String(), true
	} else {
		m.loading = true
	}
	// -model is the model of the session it starts with, however the
	// session ran before.
	if o.model != "" {
		m.chosen = map[string]string{m.sessionID: o.model}
	}
	settings, _ := cockpit.LoadSettings(o.SettingsFile) // reported by Init
	if o.SettingsFile == "" {
		settings = cockpit.Settings{}
	}
	m.resolveConnection(settings)
	m.loadPlugins()
	prefs := cockpit.LoadPreferences(o.preferences)
	m.changes.open, m.changes.width = prefs.Changes, prefs.ChangesWidth
	if o.prompt != "" {
		m.input.SetValue(o.prompt)
		m.autoSubmit = true
	}
	return m
}

func (m *uiModel) Init() tea.Cmd {
	m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	cmds := []tea.Cmd{textarea.Blink, m.title(), watchPreferences(), m.warmChanges()}
	if m.opt.SettingsFile != "" {
		if _, err := cockpit.LoadSettings(m.opt.SettingsFile); err != nil {
			cmds = append(cmds, m.notify(err.Error()+" — /settings fixes it", "error"))
		}
	}
	switch {
	case m.loading:
		cmds = append(cmds, m.load(m.sessionID))
	case m.autoSubmit:
		cmds = append(cmds, func() tea.Msg { return submitMsg{} })
	case m.opt.pick:
		cmds = append(cmds, m.openPicker("sessions"))
	}
	return tea.Batch(cmds...)
}

func (m *uiModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	_, cmd := m.update(msg)
	// The pictures the agent looked at that are about to show are made.
	if pictures := m.viewedUpdate(msg); pictures != nil {
		cmd = tea.Batch(cmd, pictures)
	}
	if m.compact && !m.switching {
		m.commit()
		if print := m.flushScrollback(); print != nil {
			cmd = tea.Batch(cmd, print)
		}
	}
	// Whatever moved the transcript, the changes panel shows the prompt in
	// view.
	if sync := m.syncChanges(); sync != nil {
		cmd = tea.Batch(cmd, sync)
	}
	return m, cmd
}

func (m *uiModel) update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch {
	case teaMessage(msg, "printLineMessage"):
		m.scroll.busy = false // the next print may go
		return m, nil
	case teaMessage(msg, "exitAltScreenMsg"), teaMessage(msg, "enterAltScreenMsg"):
		m.switching = false
		return m, nil
	}
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height, m.ready = msg.Width, msg.Height, true
		m.graphics.measure(cellPixels())
		m.layout()
		m.refresh()
		return m, nil
	case submitMsg:
		if m.autoSubmit && !m.loading {
			m.autoSubmit = false
			return m, m.submit()
		}
		return m, nil
	case linesMsg:
		return m, m.runnerOutput(msg)
	case imagesMsg:
		return m, m.imagesPasted(msg)
	case picturesMsg:
		m.pictured(msg)
		return m, nil
	case viewedMsg:
		m.viewedMade(msg)
		return m, nil
	case tickMsg:
		if msg.gen != m.jobGen || m.state == idle {
			return m, nil
		}
		m.frame++
		return m, m.tick()
	case loadedMsg:
		return m, m.loaded(msg)
	case watchMsg:
		return m, m.watched(msg)
	case modelsMsg:
		return m, m.modelsListed(msg)
	case sessionsMsg:
		if m.picker == nil || m.picker.kind != "sessions" || msg.gen != m.pickerGen {
			return m, nil
		}
		if msg.err != nil {
			m.picker.loading = false
			m.picker.empty = "cannot list sessions: " + msg.err.Error()
			return m, nil
		}
		if m.picker.editing != "" {
			return m, nil // the list catches up when the edit ends
		}
		m.picker.setItems(m.sessionItems(msg.list))
		if msg.keep != "" {
			m.picker.selectID(msg.keep)
		} else {
			m.picker.selectCurrent()
		}
		return m, nil
	case checkMsg:
		m.checked(msg)
		return m, nil
	case branchedMsg:
		return m, m.branched(msg)
	case prefsMsg:
		// The same few stats a couple of seconds apart take up plugins that
		// changed.
		return m, tea.Batch(m.preferencesChanged(), m.pluginsChanged(), watchPreferences())
	case changesLoadedMsg:
		return m, m.changesLoaded(msg)
	case diffLoadedMsg:
		m.diffLoaded(msg)
		return m, nil
	case changeUpdateMsg:
		return m, m.changeUpdate(msg)
	case changesReloadMsg:
		if msg.gen == m.changes.reload && m.changesShown() && m.changes.message != "" {
			return m, m.loadChanges()
		}
		return m, nil
	case changesSummaryMsg:
		return m, m.changesSummary(msg)
	case panelWidthMsg:
		m.panelWidthSettled(msg)
		return m, nil
	case noticeMsg:
		if msg.gen == m.noteGen {
			m.note, m.osc52 = notice{}, ""
		}
		return m, nil
	case copiedMsg:
		if msg.err != nil {
			// No system clipboard (e.g. over SSH): ask the terminal via OSC 52.
			m.osc52 = ansi.SetSystemClipboard(msg.text)
		}
		return m, m.notify(msg.what+" copied", "info")
	case selectionCopiedMsg:
		if msg.err != nil {
			m.osc52 = ansi.SetSystemClipboard(msg.text)
		}
		n := utf8.RuneCountInString(msg.text)
		unit := "chars"
		if n == 1 {
			unit = "char"
		}
		return m, m.showToast(fmt.Sprintf("%d %s copied", n, unit))
	case toastMsg:
		if msg.gen == m.toastGen {
			m.toast = ""
			if m.note.text == "" {
				m.osc52 = ""
			}
		}
		return m, nil
	case tea.MouseMsg:
		m.px, m.py = msg.X, msg.Y
		m.debris.mouse = time.Now()
		if m.form != nil {
			return m, nil
		}
		return m, m.mouse(msg)
	case releaseKeyMsg:
		return m, m.release(msg)
	case tea.BlurMsg:
		// The pointer went elsewhere; nothing here is under it.
		m.px, m.py = -1, -1
		return m, nil
	case tea.FocusMsg:
		return m, nil
	case tea.KeyMsg:
		return m, m.filterKey(msg)
	}
	if seq, ok := csiSequence(msg); ok {
		return m, m.csi(seq)
	}
	// Cursor blinks and other component messages.
	var cmd tea.Cmd
	m.input, cmd = m.input.Update(msg)
	return m, cmd
}

// ---------------------------------------------------------------- runs

func (m *uiModel) submit() tea.Cmd {
	if m.queueEdit != nil {
		// A queued prompt being edited goes back to its place.
		return m.endQueueEdit(m.input.Value())
	}
	text := strings.TrimSpace(m.input.Value())
	if text == "" {
		// Between runs, enter on an empty prompt goes on with a paused queue.
		if q := m.queues[m.sessionID]; q != nil && len(q.Items) != 0 && m.state == idle && m.edit == nil {
			return m.resumeQueue()
		}
		return nil
	}
	if strings.HasPrefix(text, "/") {
		// A command typed over an edit ends it; what the composer held
		// before the edit comes back.
		if m.edit != nil {
			m.cancelEdit()
		} else {
			m.input.Reset()
			m.clearImages()
		}
		m.resize()
		return m.command(text)
	}
	if m.edit != nil {
		return m.runEdit(text)
	}
	if m.state != idle {
		// While the agent works, a prompt waits for the run to end.
		return m.enqueue(text, false)
	}
	return m.submitText(text, true)
}

// submitText runs a prompt: the composer's, or text of the cockpit's own
// that leaves the composer alone.
func (m *uiModel) submitText(text string, fromComposer bool) tea.Cmd {
	o := startOptions{fromComposer: fromComposer}
	if fromComposer {
		o.images = m.composerImages(text)
	}
	return m.start(text, o)
}

type startOptions struct {
	fromComposer bool
	// transcript is the session as the run starts from it, when that is not
	// the transcript on screen: an edited prompt's run starts from the
	// history before the prompt.
	transcript *cockpit.Transcript
	// rewind is the message ID of the prompt the run replaces.
	rewind string
	// messageID is the ID the prompt runs under, when it has one already:
	// a queued prompt keeps the one it was queued with.
	messageID string
	// model is the model the prompt runs with, when it has one already: a
	// queued prompt keeps the one it was queued with.
	model string
	// images go with the prompt, which refers to each by its label.
	images []cockpit.Image
}

func (m *uiModel) start(text string, o startOptions) tea.Cmd {
	switch {
	case m.loading:
		return m.notify("the session is still loading", "warn")
	case m.state != idle:
		return m.notify("a run is in progress — ctrl+c stops it", "warn")
	case strings.TrimSpace(cockpit.Clean(text)) == "":
		return m.notify("the prompt has no printable text", "warn")
	case m.external:
		return m.notify("this session is running in another window or terminal", "warn")
	}
	messageID := o.messageID
	if messageID == "" {
		messageID = uuid.New().String()
	}
	model := o.model
	if model == "" {
		model = m.nextModel()
	}
	// The workspace as the run finds it, to show what the run changes.
	tracker := m.trackChanges(messageID)
	job, err := cockpit.Start(m.ctx, m.opt.Options, cockpit.Request{
		SessionID: m.sessionID, MessageID: messageID, Prompt: text, Model: model, Thinking: m.thinking,
		Resume: !m.fresh, Rewind: o.rewind,
		Images: o.images,
	})
	if err != nil && tracker != nil {
		tracker.Cancel()
	}
	if errors.Is(err, cockpit.ErrPromptGone) {
		return m.promptGone()
	}
	if errors.Is(err, cockpit.ErrSessionBusy) {
		return m.notify("this session is running in another window or terminal", "error")
	}
	if errors.Is(err, cockpit.ErrSessionGone) {
		return m.notify("this session was deleted elsewhere — ctrl+n starts a new one, keeping the prompt", "error")
	}
	if err != nil {
		return m.notify(err.Error(), "error")
	}
	if o.fromComposer {
		m.keep(text)
		m.clearImages() // they went with the prompt
	}
	if m.edit != nil {
		// What the composer held before the edit comes back.
		m.input.SetValue(m.edit.draft)
		m.images = m.edit.images
		m.edit = nil
	}
	m.rememberImages(messageID, o.images)
	m.resize()
	if o.transcript != nil {
		m.tr = o.transcript
		clear(m.cache) // entries are new values
	}
	m.tr.SubmitModel(messageID, text, cmp.Or(model, m.conn.Model), time.Now().UTC())
	if e := m.tr.Entry("input:" + messageID); e != nil && len(o.images) != 0 {
		m.tr.WithImages(e, o.images)
	}
	// The changes panel moves on to the new prompt.
	m.tracker, m.runMessage, m.changes.pinned = tracker, messageID, false
	cmd := m.attach(job)
	if tracker != nil {
		cmd = tea.Batch(cmd, waitChanges(tracker))
	}
	return cmd
}

// Placeholders say what enter does with the composer's text.
const (
	idlePlaceholder    = "Describe the task — enter runs it; shift+enter, ctrl+j or \\ then enter adds a line; / for commands"
	runningPlaceholder = "Message the agent — enter queues it for when it finishes; ctrl+x forces it in after its tools"
)

// attach shows a run the runner has started.
func (m *uiModel) attach(job *cockpit.Job) tea.Cmd {
	m.state, m.job, m.started, m.frame, m.diag = running, job, time.Now(), 0, ""
	m.input.Placeholder = runningPlaceholder
	m.jobGen++
	m.last = outcome{}
	m.follow = true
	m.refresh()
	return tea.Batch(waitJob(m.jobGen, job), m.tick(), m.title())
}

func (m *uiModel) tick() tea.Cmd {
	gen := m.jobGen
	return tea.Tick(90*time.Millisecond, func(time.Time) tea.Msg { return tickMsg{gen} })
}

// waitJob delivers runner output in batches, so a burst of events costs one
// render instead of one per line.
func waitJob(gen int, job *cockpit.Job) tea.Cmd {
	return func() tea.Msg {
		line, ok := <-job.Lines()
		if !ok {
			return linesMsg{gen: gen, done: true, err: job.Err()}
		}
		lines := []cockpit.Line{line}
		for len(lines) < 256 {
			select {
			case line, ok := <-job.Lines():
				if !ok {
					return linesMsg{gen: gen, lines: lines}
				}
				lines = append(lines, line)
			default:
				return linesMsg{gen: gen, lines: lines}
			}
		}
		return linesMsg{gen: gen, lines: lines}
	}
}

func (m *uiModel) runnerOutput(msg linesMsg) tea.Cmd {
	if msg.gen != m.jobGen || m.job == nil {
		return nil
	}
	for _, line := range msg.lines {
		if line.Stderr {
			if text := strings.TrimSpace(cockpit.Clean(line.Text)); text != "" {
				m.diag = text
			}
			continue
		}
		entries, err := m.tr.Apply([]byte(line.Text))
		if err != nil {
			m.diag = "unreadable runner output: " + err.Error()
		}
		m.pokeChanges(entries)
	}
	delivered := m.queueDelivered()
	if !msg.done {
		m.refresh()
		return tea.Batch(waitJob(msg.gen, m.job), delivered)
	}

	stopped := errors.Is(msg.err, context.Canceled)
	m.tr.Finish(msg.err, stopped, time.Now().UTC())
	if m.tracker != nil {
		// The last snapshot: what the run left.
		m.tracker.Finish()
	}
	kind := "done"
	switch {
	case stopped:
		kind = "stopped"
	case msg.err != nil:
		kind = "failed"
	}
	m.state, m.job, m.last = idle, nil, outcome{kind, time.Now()}
	m.input.Placeholder = idlePlaceholder
	if _, err := os.Stat(cockpit.SessionPath(m.opt.SessionDir, m.sessionID)); err == nil {
		m.fresh = false
	}
	var cmds []tea.Cmd
	// A prompt the runner never received goes back to the composer.
	for i := len(m.tr.Entries) - 1; i >= 0; i-- {
		e := m.tr.Entries[i]
		if e.Kind == cockpit.KindUser {
			if e.State == cockpit.Undelivered && strings.TrimSpace(m.input.Value()) == "" {
				m.input.SetValue(e.Text)
				cmds = append(cmds, m.setImages(m.promptImages(e)))
				m.resize()
				cmds = append(cmds, m.notify("the prompt was not delivered; it is back in the composer", "warn"))
			}
			break
		}
	}
	if m.compaction != nil {
		if kind == "done" {
			cmds = append(cmds, m.compacted(*m.compaction))
		}
		m.compaction = nil
	}
	m.refresh()
	cmds = append(cmds, m.title(), tea.Tick(5*time.Second, func(time.Time) tea.Msg { return tickMsg{-1} }))
	if m.editAfterStop {
		m.editAfterStop = false
		cmds = append(cmds, m.editLast())
	}
	// What was written meanwhile runs next, or waits for the user.
	cmds = append(cmds, delivered, m.settleQueue(kind))
	return tea.Batch(cmds...)
}

func (m *uiModel) stop() tea.Cmd {
	if m.state != running {
		return nil
	}
	m.state = stopping
	m.job.Cancel()
	return m.notify("stopping the run…", "warn")
}

// shutdown stops a running job and waits for the runner to exit, and for
// the last snapshot of what it changed.
func (m *uiModel) shutdown() {
	if m.job != nil {
		m.job.Cancel()
		for range m.job.Lines() {
		}
	}
	if t := m.tracker; t != nil {
		t.Finish()
		wait := time.After(5 * time.Second)
		for {
			select {
			case _, ok := <-t.Updates():
				if !ok {
					return
				}
			case <-wait:
				return
			}
		}
	}
}

// ---------------------------------------------------------------- sessions

func (m *uiModel) load(id string) tea.Cmd {
	m.loadGen++
	m.loading = true
	return m.read(id, false)
}

// read loads a session in the background. The busy probe comes first: a run
// that ends while the file is read then shows up as a size change later.
func (m *uiModel) read(id string, quiet bool) tea.Cmd {
	gen, dir := m.loadGen, m.opt.SessionDir
	return func() tea.Msg {
		busy := cockpit.SessionBusy(dir, id)
		tr, err := cockpit.LoadSession(dir, id)
		return loadedMsg{gen: gen, id: id, tr: tr, meta: cockpit.LoadMeta(dir, id), busy: busy, quiet: quiet, err: err}
	}
}

// watch checks on a session another process is running.
func (m *uiModel) watch(delay time.Duration) tea.Cmd {
	gen, dir, id := m.loadGen, m.opt.SessionDir, m.sessionID
	return tea.Tick(delay, func(time.Time) tea.Msg {
		msg := watchMsg{gen: gen, busy: cockpit.SessionBusy(dir, id)}
		if info, err := os.Stat(cockpit.SessionPath(dir, id)); err == nil {
			msg.size = info.Size()
		}
		return msg
	})
}

func (m *uiModel) watched(msg watchMsg) tea.Cmd {
	if msg.gen != m.loadGen || !m.external || m.state != idle || m.loading {
		return nil
	}
	switch {
	case msg.size != m.tr.Size:
		return tea.Batch(m.read(m.sessionID, true), m.liveChanges())
	case !msg.busy:
		m.external = false
		m.refreshKeep()
		return tea.Batch(m.notify("the other run has finished", "info"), m.liveChanges())
	}
	return tea.Batch(m.watch(2*time.Second), m.liveChanges())
}

func (m *uiModel) loaded(msg loadedMsg) tea.Cmd {
	if msg.gen != m.loadGen {
		return nil
	}
	if msg.quiet {
		if m.state != idle || msg.id != m.sessionID {
			return nil
		}
		if msg.err != nil {
			switch {
			case !m.external:
				return nil
			case errors.Is(msg.err, os.ErrNotExist):
				m.external = false
				m.refreshKeep()
				return m.notify("the session was deleted in another window", "warn")
			}
			return m.watch(2 * time.Second) // a passing read error; keep following
		}
		// Same session, newer file: keep what the reader opened and where
		// they are; entries are new values, so the render cache starts over.
		m.tr, m.external, m.meta = msg.tr, msg.busy, msg.meta
		clear(m.cache)
		m.refresh()
		if m.external {
			return m.watch(2 * time.Second)
		}
		return m.notify("the other run has finished", "info")
	}
	m.loading = false
	if msg.err != nil {
		if errors.Is(msg.err, os.ErrNotExist) {
			// A new session under a chosen ID: the runner creates it.
			m.sessionID, m.fresh, m.external, m.meta = msg.id, true, false, cockpit.Meta{}
			m.reset(cockpit.NewTranscript())
			return tea.Batch(m.notify("new session "+shortID(msg.id), "info"), m.title(), m.resubmit())
		}
		m.autoSubmit = false
		return m.notify("cannot open session: "+msg.err.Error(), "error")
	}
	m.sessionID, m.fresh, m.external, m.meta = msg.id, false, msg.busy, msg.meta
	m.reset(msg.tr)
	if m.external {
		m.autoSubmit = false
		return tea.Batch(m.title(), m.notify("this session is running in another window or terminal — following it", "warn"), m.watch(2*time.Second))
	}
	if m.tr.Interrupted() && !m.autoSubmit {
		return tea.Batch(m.title(), m.notify("the last run did not finish — /continue picks it up, esc esc edits its prompt", "warn"))
	}
	return tea.Batch(m.title(), m.resubmit())
}

func (m *uiModel) resubmit() tea.Cmd {
	if !m.autoSubmit {
		return nil
	}
	return func() tea.Msg { return submitMsg{} }
}

func (m *uiModel) reset(tr *cockpit.Transcript) {
	if m.edit != nil {
		m.cancelEdit()
	}
	if m.queueEdit != nil {
		m.endQueueEdit(m.queueEdit.item.Text)
	}
	m.blurQueue()
	m.tr = tr
	m.cache = make(map[string]block)
	m.expanded = make(map[string]bool)
	m.expandAll, m.follow, m.unseen, m.shown = false, true, 0, 0
	m.last = outcome{}
	m.refresh()
}

func (m *uiModel) openSession(id string) tea.Cmd {
	if m.state != idle {
		return m.notify("stop the run before switching sessions (ctrl+c)", "warn")
	}
	if !cockpit.ValidSessionID(id) {
		return m.notify("invalid session ID "+id, "error")
	}
	m.closePicker()
	return m.load(id)
}

func (m *uiModel) newSession() tea.Cmd {
	if m.state != idle {
		return m.notify("stop the run before starting a new session (ctrl+c)", "warn")
	}
	m.loadGen++ // drop any load still in flight
	m.loading = false
	m.sessionID, m.fresh, m.external, m.meta = uuid.New().String(), true, false, cockpit.Meta{}
	m.reset(cockpit.NewTranscript())
	return tea.Batch(m.notify("new session", "info"), m.title())
}

func (m *uiModel) listSessions() tea.Cmd {
	m.pickerGen++
	gen, dir := m.pickerGen, m.opt.SessionDir
	return func() tea.Msg {
		list, err := cockpit.ListSessions(dir)
		return sessionsMsg{gen: gen, list: list, err: err}
	}
}

func (m *uiModel) sessionItems(list []cockpit.SessionInfo) []pickerItem {
	items := make([]pickerItem, 0, len(list))
	for _, s := range list {
		title := s.Title
		if title == "" {
			title = "untitled session"
		}
		id := s.ID
		items = append(items, pickerItem{
			title:   title,
			detail:  ago(s.UpdatedAt) + " · " + size(s.Size) + " · " + shortID(s.ID),
			search:  s.ID,
			current: s.ID == m.sessionID,
			pinned:  s.Pinned,
			id:      s.ID,
			action:  func(m *uiModel) tea.Cmd { return m.openSession(id) },
		})
	}
	return items
}

// ---------------------------------------------------------------- overlays

func (m *uiModel) openPicker(kind string) tea.Cmd {
	if m.picker != nil && m.picker.kind == kind {
		m.closePicker()
		return nil
	}
	st := m.styles
	// A list takes the keys from the changes panel too.
	m.changes.focused = false
	switch kind {
	case "palette":
		m.picker = newPicker(kind, "commands", "no commands", st)
		m.picker.setItems(m.paletteItems())
	case "sessions":
		m.picker = newPicker(kind, "sessions", "no saved sessions yet", st)
		m.picker.loading = true
		m.picker.hints = []string{"enter", "open", "^E", "rename", "^T", "pin", "^B", "duplicate", "^D", "delete", "esc", "close"}
		m.input.Blur()
		return m.listSessions()
	case "history":
		m.picker = newPicker(kind, "prompt history", "no prompts yet", st)
		items := make([]pickerItem, 0, len(m.history))
		for i := len(m.history) - 1; i >= 0; i-- {
			h := m.history[i]
			items = append(items, pickerItem{
				title:  titleForPrompt(h.Prompt),
				detail: ago(h.CreatedAt),
				search: h.Prompt,
				action: func(m *uiModel) tea.Cmd {
					m.closePicker()
					m.input.SetValue(h.Prompt)
					m.input.CursorEnd()
					m.resize()
					return nil
				},
			})
		}
		m.picker.setItems(items)
	case "models":
		m.picker = newPicker(kind, "model · next prompts", "the connection lists no models — type an ID, enter uses it", st)
		m.picker.loading = true
		m.picker.hints = []string{"enter", "use", "↑↓", "move", "esc", "close"}
		m.input.Blur()
		return m.listModels()
	case "help":
		m.picker = newPicker(kind, "keyboard", "", st)
		var items []pickerItem
		for _, k := range shortcuts {
			items = append(items, pickerItem{title: k[1], detail: k[0]})
		}
		m.picker.setItems(items)
	}
	m.input.Blur()
	return nil
}

func (m *uiModel) closePicker() {
	m.picker = nil
	m.pickerGen++
	m.input.Focus()
}

var shortcuts = [][2]string{
	{"ctrl+s  ^E ^T ^B ^D", "sessions · in the list: rename, pin, duplicate, delete"},
	{"enter", "run the prompt"},
	{"shift+enter · alt+enter · ctrl+j", "new line; \\ then enter works in any terminal"},
	{"↑ ↓", "move between lines; at the first or last, prompt history"},
	{"ctrl+v", "paste an image, which the prompt names [Image 1]; pasting or dropping image files attaches them"},
	{"right after [Image n]", "the image shows large over the transcript; ⌫ there removes it, a click on it above the composer goes there"},
	{"esc esc", "stop the run; when none runs, edit the last prompt"},
	{"click ✎ edit", "edit that prompt and run it again from there"},
	{"esc", "while editing: keep the prompt as it was"},
	{"ctrl+c", "stop the run · clear the prompt · twice to quit"},
	{"ctrl+d", "quit"},
	{"ctrl+k", "command palette"},
	{"ctrl+r", "prompt history"},
	{"ctrl+n", "new session"},
	{"ctrl+t · shift+tab", "next effort level, shared with the web cockpit"},
	{"ctrl+p · /model", "the model of the next prompts: any the connection reaches, of any provider"},
	{"alt+↑ alt+↓", "raise / lower the effort; click its bars to pick one"},
	{"ctrl+o", "expand / collapse tool output and thinking"},
	{"click", "a block's first line folds or unfolds it; in the composer, places the cursor"},
	{"drag", "select text; letting go copies it"},
	{"scrollbar", "click or drag to scroll"},
	{"ctrl+y", "copy the last answer"},
	{"ctrl+l", "clear the view (the session keeps its history); in compact, the screen"},
	{"ctrl+f", "inline ↔ fullscreen; the next start keeps it"},
	{"ctrl+g", "changes: what the prompt in view changed, file by file, beside the transcript"},
	{"tab", "complete a /command; otherwise to the changes panel and back"},
	{"in changes: ↑ ↓ ← →", "pick a file · fold a folder; click works too"},
	{"in changes: pgup pgdn · wheel", "scroll the diff"},
	{"in changes: [ ]", "the prompt before · after; scrolling the transcript does it too"},
	{"in changes: f", "follow the file the run changes, after picking another"},
	{"in changes: < > · shift+← →", "move the panel's edge; = gives the panel its width back; dragging the edge works too"},
	{"ctrl+z", "suspend to the shell (fg resumes)"},
	{"pgup pgdn · wheel", "scroll the transcript"},
	{"home end", "top / bottom of the transcript (empty prompt)"},
}

var commands = []struct{ name, args, help string }{
	{"/new", "", "start a new session"},
	{"/sessions", "", "open a saved session"},
	{"/resume", "<id>", "resume a session by ID or ID prefix"},
	{"/continue", "", "pick up an interrupted run"},
	{"/queue", "[resume|pause|clear]", "what waits for the agent: select it (↑), run it, hold it or drop it"},
	{"/compact", "[focus]", "summarize the conversation to free context; the session goes on from the summary"},
	{"/plugins", "[trust|untrust|reload]", "the workspace's plugins, and trusting the workspace with them"},
	{"/edit", "[n]", "edit prompt n, by default the last, and run it again from there"},
	{"/rename", "<title>", "title the session (- restores the first prompt)"},
	{"/pin", "", "pin or unpin the session"},
	{"/fork", "[n]", "branch before prompt n, or duplicate the session"},
	{"/export", "[path]", "save the session as Markdown"},
	{"/delete", "", "delete the session"},
	{"/settings", "", "connection: provider type, base URL, API key, model"},
	{"/history", "", "reuse an earlier prompt"},
	{"/effort", "<level>", "effort: low medium high xhigh max"},
	{"/model", "[id]", "model of the next prompts, of any provider; alone, the list"},
	{"/copy", "", "copy the last answer"},
	{"/expand", "", "expand tool output and thinking"},
	{"/collapse", "", "collapse tool output and thinking"},
	{"/clear", "", "clear the view"},
	{"/changes", "", "what the prompt in view changed, file by file (ctrl+g)"},
	{"/inline", "", "below the command: the transcript goes to the terminal's scrollback"},
	{"/fullscreen", "", "a screen of its own, with the mouse"},
	{"/help", "", "keyboard shortcuts"},
	{"/quit", "", "exit"},
}

func (m *uiModel) paletteItems() []pickerItem {
	run := func(text string) func(*uiModel) tea.Cmd {
		return func(m *uiModel) tea.Cmd {
			m.closePicker()
			return m.command(text)
		}
	}
	items := []pickerItem{
		{title: "New session", detail: "ctrl+n", action: run("/new")},
		{title: "Open a session…", detail: "ctrl+s", action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.openPicker("sessions") }},
		{title: "Prompt history…", detail: "ctrl+r", action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.openPicker("history") }},
		{title: "Connection settings…", detail: "/settings", search: "provider base url api key model", action: func(m *uiModel) tea.Cmd { return m.openSettings() }},
	}
	if m.state == idle && !m.external && m.tr.Interrupted() {
		items = append([]pickerItem{{title: "Continue the interrupted run", detail: "/continue", action: run("/continue")}}, items...)
	}
	if m.state == idle && !m.external && !m.fresh {
		items = append([]pickerItem{{title: "Compact the context", detail: "/compact", search: "summarize summary free context tokens", action: run("/compact")}}, items...)
	}
	if m.runBlocked() == "" && m.lastPrompt() != nil {
		items = append([]pickerItem{{title: "Edit the last prompt", detail: "esc esc", search: "edit rewind change prompt", action: func(m *uiModel) tea.Cmd {
			m.closePicker()
			return m.editLast()
		}}}, items...)
	}
	if !m.fresh {
		pin := "Pin the session"
		if m.meta.Pinned {
			pin = "Unpin the session"
		}
		items = append(items,
			pickerItem{title: "Rename the session…", detail: "/rename", action: run("/rename")},
			pickerItem{title: pin, detail: "/pin", action: run("/pin")},
			pickerItem{title: "Branch before a prompt…", detail: "/fork n", action: func(m *uiModel) tea.Cmd {
				m.closePicker()
				m.input.SetValue("/fork ")
				m.input.CursorEnd()
				return m.notify("type the number of the prompt to branch before", "info")
			}},
			pickerItem{title: "Duplicate the session", detail: "/fork", action: run("/fork")},
			pickerItem{title: "Export as Markdown", detail: "/export", action: run("/export")},
			pickerItem{title: "Delete the session…", detail: "/delete", action: run("/delete")},
		)
	}
	if m.state == running {
		items = append([]pickerItem{{title: "Stop the run", detail: "ctrl+c", action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.stop() }}}, items...)
	}
	if q := m.queues[m.sessionID]; q != nil && len(q.Items) != 0 {
		// What waits for the agent comes first.
		queue := []pickerItem{{title: "Select a queued message", detail: "↑", search: "queue queued edit force drop", action: run("/queue")}}
		if q.Paused || m.state == idle {
			queue = append(queue, pickerItem{title: "Run the queued messages", detail: "/queue resume", search: "queue resume continue", action: run("/queue resume")})
		}
		if q.Queued() != 0 {
			queue = append(queue, pickerItem{title: "Drop the queued messages", detail: "/queue clear", search: "queue clear remove", action: run("/queue clear")})
		}
		items = append(queue, items...)
	}
	items = append(items,
		pickerItem{title: "Copy the last answer", detail: "ctrl+y", action: run("/copy")},
		pickerItem{title: "Copy the session ID", search: m.sessionID, action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.copy(m.sessionID, "session ID") }},
		pickerItem{title: "Plugins…", detail: "/plugins", search: "extensions tools trust", action: run("/plugins")},
		pickerItem{title: "Expand tool output and thinking", detail: "ctrl+o", action: run("/expand")},
		pickerItem{title: "Collapse tool output and thinking", detail: "ctrl+o", action: run("/collapse")},
		pickerItem{title: "Clear the view", detail: "ctrl+l", action: run("/clear")},
	)
	for _, command := range m.pluginCommands {
		name, args := "/"+command.Name, command.Args
		items = append(items, pickerItem{
			title: name + " — " + command.Description, detail: command.Plugin, search: args,
			action: func(m *uiModel) tea.Cmd {
				m.closePicker()
				if args == "" {
					return m.command(name)
				}
				m.input.SetValue(name + " ")
				m.input.CursorEnd()
				m.resize()
				return m.notify(name+" takes "+args, "info")
			},
		})
	}
	changes := "Show the changes of the prompt in view"
	if m.changesShown() {
		changes = "Hide the changes"
	}
	items = append(items, pickerItem{title: changes, detail: "ctrl+g", search: "diff files changed tree", action: run("/changes")})
	if m.compact {
		items = append(items, pickerItem{title: "Fullscreen: a screen of its own, with the mouse", detail: "ctrl+f", search: "layout alternate screen", action: run("/fullscreen")})
	} else {
		items = append(items, pickerItem{title: "Inline: below the command, into the scrollback", detail: "ctrl+f", search: "layout compact scrollback", action: run("/inline")})
	}
	for _, level := range cockpit.ThinkingLevels {
		items = append(items, pickerItem{title: "Effort: " + level, search: "thinking reasoning", current: level == m.thinking, action: run("/effort " + level)})
	}
	items = append(items, pickerItem{
		title: "Model: " + orDefault(m.nextModel(), orDefault(m.conn.Model, "runner default")) + " — choose another…", detail: "ctrl+p",
		search: "model provider claude gpt grok gemini", action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.openPicker("models") },
	})
	items = append(items,
		pickerItem{title: "Keyboard shortcuts", detail: "/help", action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.openPicker("help") }},
		pickerItem{title: "Quit", detail: "ctrl+d", action: run("/quit")},
	)
	return items
}

// ---------------------------------------------------------------- commands

func (m *uiModel) command(text string) tea.Cmd {
	fields := strings.Fields(text)
	arg := strings.TrimSpace(strings.TrimPrefix(text, fields[0]))
	switch fields[0] {
	case "/help", "/?":
		return m.openPicker("help")
	case "/new":
		return m.newSession()
	case "/sessions":
		return m.openPicker("sessions")
	case "/resume":
		if arg == "" {
			return m.openPicker("sessions")
		}
		id, err := cockpit.FindSession(m.opt.SessionDir, arg)
		if err != nil {
			return m.notify(err.Error(), "warn")
		}
		return m.openSession(id)
	case "/continue":
		return m.continueRun()
	case "/queue":
		return m.queueCommand(arg)
	case "/compact":
		return m.compactContext(arg)
	case "/plugins":
		return m.plugins(arg)
	case "/edit":
		return m.editPrompt(arg)
	case "/rename":
		return m.rename(arg)
	case "/pin":
		return m.togglePin()
	case "/fork", "/branch":
		return m.fork(arg)
	case "/export":
		return m.export(arg)
	case "/delete":
		return m.confirmDelete(m.sessionID, m.sessionTitle())
	case "/settings", "/config":
		return m.openSettings()
	case "/history":
		return m.openPicker("history")
	case "/effort", "/think":
		if arg == "" {
			return m.notify("effort: "+m.thinking+" · levels: "+strings.Join(cockpit.ThinkingLevels, " "), "info")
		}
		if !cockpit.ValidThinkingLevel(arg) {
			return m.notify("effort levels: "+strings.Join(cockpit.ThinkingLevels, " "), "warn")
		}
		return m.setEffort(arg)
	case "/model", "/models":
		if arg == "" {
			return m.openPicker("models")
		}
		return m.chooseModel(arg)
	case "/copy":
		for i := len(m.tr.Entries) - 1; i >= 0; i-- {
			if e := m.tr.Entries[i]; e.Kind == cockpit.KindAssistant {
				return m.copy(e.Text, "last answer")
			}
		}
		return m.notify("no answer to copy yet", "warn")
	case "/expand", "/collapse":
		m.expandAll = fields[0] == "/expand"
		clear(m.expanded)
		m.refresh()
		return nil
	case "/clear":
		if m.state != idle || m.external {
			return m.notify("the view can be cleared once the run has finished", "warn")
		}
		m.reset(cockpit.NewTranscript())
		return m.notify("view cleared — the session keeps its history", "info")
	case "/changes":
		return m.toggleChanges()
	case "/inline":
		return m.setLayout(cockpit.LayoutCompact)
	case "/fullscreen":
		return m.setLayout(cockpit.LayoutFullscreen)
	case "/quit", "/exit":
		m.shutdownNotice()
		m.quitting = true
		return tea.Quit
	}
	if command, ok := m.pluginCommand(fields[0]); ok {
		return m.submitText(command.Expand(arg), false)
	}
	return m.notify("unknown command "+fields[0]+" — /help lists them", "warn")
}

func (m *uiModel) shutdownNotice() {
	if m.state != idle {
		m.note = notice{text: "stopping the run…", level: "warn"}
	}
}

func (m *uiModel) copy(text, what string) tea.Cmd {
	return func() tea.Msg {
		return copiedMsg{text: text, what: what, err: writeClipboard(text)}
	}
}

// writeClipboard puts text on the system clipboard; tests replace it.
var writeClipboard = clipboard.WriteAll

func (m *uiModel) notify(text, level string) tea.Cmd {
	m.noteGen++
	m.note = notice{text: text, level: level, gen: m.noteGen}
	gen := m.noteGen
	wait := 3 * time.Second
	if level == "error" {
		wait = 6 * time.Second
	}
	return tea.Tick(wait, func(time.Time) tea.Msg { return noticeMsg{gen} })
}

// ---------------------------------------------------------------- input

// handleKey routes a key to the form or the cockpit.
func (m *uiModel) handleKey(msg tea.KeyMsg) tea.Cmd {
	if m.form != nil {
		return m.formKey(msg)
	}
	return m.key(msg)
}

func (m *uiModel) key(msg tea.KeyMsg) tea.Cmd {
	if m.picker != nil {
		return m.pickerKey(msg)
	}
	if m.queueFocus >= 0 && !msg.Paste {
		if cmd, handled := m.queueKey(msg); handled {
			return cmd
		}
	}
	if m.changes.focused {
		if m.changesShown() {
			if cmd, handled := m.changesKey(msg); handled {
				return cmd
			}
		}
		// Anything else is for the composer, which takes the keys back.
		m.blurChanges()
	}
	if msg.Paste {
		return m.pasteKey(string(msg.Runes))
	}
	key := msg.String()
	if key != "ctrl+c" && key != "ctrl+d" {
		m.quitArmed = time.Time{}
	}
	if key != "esc" && key != "alt+esc" {
		m.escArmed, m.escKind = time.Time{}, ""
	}
	switch key {
	case "ctrl+c":
		switch {
		case m.queueEdit != nil:
			return m.endQueueEdit(m.queueEdit.item.Text)
		case m.state == running:
			return m.stop()
		case m.state == stopping:
			return m.notify("already stopping…", "warn")
		case m.input.Value() != "":
			m.input.Reset()
			m.clearImages()
			m.resize()
			m.historyPos = -1
			return nil
		case time.Since(m.quitArmed) < 2*time.Second:
			m.quitting = true
			return tea.Quit
		}
		m.quitArmed = time.Now()
		if q := m.queues[m.sessionID]; q != nil && len(q.Items) != 0 {
			return m.notify("press ctrl+c again to quit — "+plural(len(q.Items), "queued message is", "queued messages are")+" dropped", "warn")
		}
		return m.notify("press ctrl+c again to quit", "info")
	case "ctrl+d":
		if m.state != idle && time.Since(m.quitArmed) > 2*time.Second {
			m.quitArmed = time.Now()
			return m.notify("a run is in progress — ctrl+d again stops it and quits", "warn")
		}
		m.shutdownNotice()
		m.quitting = true
		return tea.Quit
	case "esc", "alt+esc":
		// Two escapes that arrive together read as alt+esc.
		if m.edit != nil {
			m.cancelEdit()
			return nil
		}
		if m.queueEdit != nil {
			return m.endQueueEdit(m.queueEdit.item.Text)
		}
		return m.escape(key == "alt+esc")
	case "ctrl+k":
		return m.openPicker("palette")
	case "ctrl+p":
		return m.openPicker("models")
	case "ctrl+s":
		return m.openPicker("sessions")
	case "ctrl+r":
		return m.openPicker("history")
	case "ctrl+n":
		return m.newSession()
	case "ctrl+o":
		m.expandAll = !m.expandAll
		clear(m.expanded)
		m.refresh()
		if m.compact {
			// What is printed stays as it was.
			if m.expandAll {
				return m.notify("tool output and thinking show in full from here on · ^F shows all of it", "info")
			}
			return m.notify("tool output and thinking fold from here on", "info")
		}
		return nil
	case "ctrl+t", "shift+tab":
		return m.setEffort(nextLevel(m.thinking))
	case "alt+up":
		return m.stepEffort(1)
	case "alt+down":
		return m.stepEffort(-1)
	case "ctrl+y":
		return m.command("/copy")
	case "ctrl+l":
		if m.compact {
			// Like a shell: the screen clears, the scrollback stays.
			return tea.ClearScreen
		}
		return m.command("/clear")
	case "ctrl+f":
		if m.compact {
			return m.setLayout(cockpit.LayoutFullscreen)
		}
		return m.setLayout(cockpit.LayoutCompact)
	case "ctrl+g":
		return m.toggleChanges()
	case "ctrl+x":
		return m.forceComposer()
	case "ctrl+z":
		m.leaveScreen(!m.compact)
		return tea.Suspend
	case "ctrl+v":
		// An image from the clipboard, or its text as elsewhere.
		return m.pasteClipboard(true)
	case "pgup", "pgdown", "shift+up", "shift+down":
		if m.compact {
			return nil // the terminal scrolls its own scrollback
		}
	}
	switch key {
	case "pgup":
		m.view.HalfPageUp()
		m.userScrolled()
		return nil
	case "pgdown":
		m.view.HalfPageDown()
		m.userScrolled()
		return nil
	case "shift+up":
		m.view.ScrollUp(1)
		m.userScrolled()
		return nil
	case "shift+down":
		m.view.ScrollDown(1)
		m.userScrolled()
		return nil
	case "home", "end":
		if m.input.Value() == "" && !m.compact {
			if key == "home" {
				m.view.GotoTop()
			} else {
				m.view.GotoBottom()
			}
			m.userScrolled()
			return nil
		}
	case "enter":
		// A backslash before the cursor asks for a line break, in any terminal.
		if m.backslashBeforeCursor() {
			m.input, _ = m.input.Update(tea.KeyMsg{Type: tea.KeyBackspace})
			m.newline()
			return nil
		}
		return m.submit()
	case "alt+enter", "ctrl+j":
		m.newline()
		return nil
	case "backspace":
		// Right after an image's label, the label goes at once.
		if m.eraseLabel() {
			return nil
		}
	case "tab":
		if value := m.input.Value(); m.changesShown() && (!strings.HasPrefix(value, "/") || strings.Contains(value, " ")) {
			m.focusChanges()
			return nil
		}
		m.complete()
		return nil
	case "up":
		// Over an empty composer, ↑ goes to the queue before the history.
		if q := m.queues[m.sessionID]; q != nil && len(q.Items) != 0 && m.input.Value() == "" && m.queueEdit == nil && m.edit == nil {
			m.focusQueue(len(q.Items) - 1)
			return nil
		}
		if m.input.Line() == 0 && m.browseHistory(-1) {
			return nil
		}
	case "down":
		if m.input.Line() == m.input.LineCount()-1 && m.browseHistory(1) {
			return nil
		}
	}
	var cmd tea.Cmd
	m.input, cmd = m.input.Update(msg)
	m.resize()
	return cmd
}

func (m *uiModel) pickerKey(msg tea.KeyMsg) tea.Cmd {
	if m.picker.kind == "sessions" {
		if cmd, handled := m.sessionsKey(msg); handled {
			return cmd
		}
	}
	switch msg.String() {
	case "ctrl+k", "ctrl+s", "ctrl+r":
		kind := map[string]string{"ctrl+k": "palette", "ctrl+s": "sessions", "ctrl+r": "history"}[msg.String()]
		if m.picker.kind == kind {
			m.closePicker()
			return nil
		}
		m.closePicker()
		return m.openPicker(kind)
	}
	cmd, choose, closed := m.picker.update(msg)
	switch {
	case closed:
		m.closePicker()
	case choose:
		item := m.picker.selected()
		if item == nil && m.picker.kind == "models" {
			// A model the connection does not list can still be typed.
			if typed := strings.TrimSpace(m.picker.query.Value()); typed != "" {
				m.closePicker()
				return tea.Batch(cmd, m.chooseModel(typed))
			}
		}
		if item == nil || item.action == nil {
			if m.picker.kind == "help" {
				m.closePicker()
			}
			return cmd
		}
		return tea.Batch(cmd, item.action(m))
	}
	return cmd
}

// browseHistory walks prompt history like a shell; the unsent draft comes
// back after the newest entry.
func (m *uiModel) browseHistory(step int) bool {
	if len(m.history) == 0 || (step > 0 && m.historyPos < 0) {
		return false
	}
	switch {
	case m.historyPos < 0:
		// The draft's images wait with it: history holds text alone.
		m.draft = m.input.Value()
		m.draftImages, m.images = m.images, nil
		m.historyPos = len(m.history) - 1
	case m.historyPos+step >= len(m.history):
		m.historyPos = -1
		m.input.SetValue(m.draft)
		m.clearImages()
		m.images, m.draftImages = m.draftImages, nil
		m.resize()
		return true
	default:
		m.historyPos = max(0, m.historyPos+step)
	}
	m.input.SetValue(m.history[m.historyPos].Prompt)
	m.input.CursorEnd()
	m.resize()
	return true
}

func (m *uiModel) complete() {
	value := m.input.Value()
	if !strings.HasPrefix(value, "/") || strings.Contains(value, " ") {
		return
	}
	for _, c := range m.commandTable() {
		if strings.HasPrefix(c.name, value) {
			completion := c.name
			if c.args != "" {
				completion += " "
			}
			m.input.SetValue(completion)
			m.input.CursorEnd()
			return
		}
	}
}

// commandTable is the built-in commands and the plugins' ones.
func (m *uiModel) commandTable() []struct{ name, args, help string } {
	table := append([]struct{ name, args, help string }(nil), commands...)
	for _, command := range m.pluginCommands {
		table = append(table, struct{ name, args, help string }{"/" + command.Name, command.Args, command.Description + " (" + command.Plugin + ")"})
	}
	return table
}

func (m *uiModel) mouse(msg tea.MouseMsg) tea.Cmd {
	if m.picker != nil {
		switch {
		case msg.Action == tea.MouseActionMotion && msg.Button == tea.MouseButtonNone:
			// The list's cursor follows the pointer.
			if i := m.picker.rowAt(msg.Y); i >= 0 && m.picker.editing == "" {
				m.picker.cursor = i
			}
		case msg.Button == tea.MouseButtonWheelUp:
			m.picker.move(-1)
		case msg.Button == tea.MouseButtonWheelDown:
			m.picker.move(1)
		case msg.Button == tea.MouseButtonLeft && msg.Action == tea.MouseActionPress:
			if i := m.picker.rowAt(msg.Y); i >= 0 {
				m.picker.cursor = i
				if item := m.picker.selected(); item != nil && item.action != nil {
					return item.action(m)
				}
			}
		}
		return nil
	}
	// The left button selects by dragging and clicks by letting go where it
	// went down; the wheel scrolls.
	switch {
	case msg.Button == tea.MouseButtonLeft && msg.Action == tea.MouseActionPress:
		m.press = &cell{row: msg.Y, col: msg.X}
		if m.resizing = m.onPanelEdge(msg.X, msg.Y); m.resizing {
			// The panel's edge drags; the keys stay where they are.
			m.pressEdge()
			return nil
		}
		if !m.inPanel(msg.X, msg.Y) {
			m.blurChanges()
		}
		if m.scrubbing = m.onScrollbar(msg.X, msg.Y); m.scrubbing {
			m.scrub(msg.Y)
			return nil
		}
		m.sel = m.selectionAt(msg.X, msg.Y)
		return nil
	case msg.Action == tea.MouseActionMotion && m.press != nil:
		switch {
		case m.resizing:
			// The panel takes the columns right of the pointer.
			m.setPanelWidth(m.width - msg.X)
		case m.scrubbing:
			m.scrub(msg.Y)
		default:
			m.drag(msg.X, msg.Y)
		}
		return nil
	case msg.Action == tea.MouseActionRelease && m.press != nil:
		press, sel := *m.press, m.sel
		m.press, m.sel = nil, nil
		if m.resizing {
			m.resizing = false
			m.edgeReleased()
			return nil
		}
		if m.scrubbing {
			m.scrubbing = false
			return nil
		}
		if sel != nil && sel.moved {
			return m.copySelection(sel)
		}
		return m.click(press.col, press.row)
	case msg.Action == tea.MouseActionMotion, msg.Action == tea.MouseActionRelease:
		return nil
	case msg.Button == tea.MouseButtonWheelUp || msg.Button == tea.MouseButtonWheelDown:
		if m.inPanel(msg.X, msg.Y) {
			m.changesWheel(msg.Y, msg.Button == tea.MouseButtonWheelUp)
			return nil
		}
	}
	var cmd tea.Cmd
	m.view, cmd = m.view.Update(msg)
	if msg.Button == tea.MouseButtonWheelUp || msg.Button == tea.MouseButtonWheelDown {
		m.userScrolled()
	} else {
		m.scrolled()
	}
	return cmd
}

// click acts on what is under the pointer: a task on the welcome screen, a
// prompt's header, a block that folds, or the effort meter.
func (m *uiModel) click(x, y int) tea.Cmd {
	if m.inPreview(x, y) {
		return nil // the picture covers what is under it
	}
	if m.inPanel(x, y) {
		return m.changesClick(x, y)
	}
	switch {
	case y >= transcriptTop && y < transcriptTop+m.view.Height:
		line := m.view.YOffset + y - transcriptTop
		if prompt, ok := m.starters[line]; ok && m.input.Value() == "" {
			m.input.SetValue(prompt)
			m.input.CursorEnd()
			m.resize()
			return nil
		}
		if e := m.promptHeaderAt(line); e != nil {
			if m.runBlocked() == "" {
				return m.beginEdit(e.ID)
			}
			return nil
		}
		m.toggleAt(line)
	case y > m.statusRow() && y < m.ruleRow():
		if _, index, ok := m.queueRowAt(y); ok {
			m.focusQueue(index)
		} else if label, ok := m.stripAt(x, y); ok {
			m.cursorAfter(label)
		}
	case y == m.ruleRow():
		if control, _ := m.effortControl(); x < m.width-ansi.StringWidth(control) {
			if model := m.modelControl(); model != "" && x >= m.width-ansi.StringWidth(control)-ansi.StringWidth(model) {
				return m.openPicker("models")
			}
		}
		return m.clickEffort(x)
	case y >= m.inputTop() && y < m.inputTop()+m.input.Height():
		m.blurQueue()
		m.placeCursor(x, y)
	}
	return nil
}

// promptHeaderAt returns the prompt whose header is on a transcript line.
func (m *uiModel) promptHeaderAt(line int) *cockpit.Entry {
	for _, s := range m.spans {
		if s.start == line {
			if e := m.tr.Entry(s.id); e != nil && e.Kind == cockpit.KindUser {
				return e
			}
			return nil
		}
	}
	return nil
}

func (m *uiModel) toggleAt(line int) {
	if e := m.foldAt(line); e != nil {
		m.expanded[e.ID] = !m.isOpen(e)
		m.refreshKeep()
	}
}

// foldAt returns the block a click on a transcript line folds or unfolds:
// any line of it while it is folded, its first once it is open, as the rest
// is its text, to select.
func (m *uiModel) foldAt(line int) *cockpit.Entry {
	for _, s := range m.spans {
		if line < s.start || line >= s.end {
			continue
		}
		e := m.tr.Entry(s.id)
		// An open call's picture folds it too.
		if e == nil || !expandable(e) || line != s.start && m.isOpen(e) && line < s.end-s.pictures {
			return nil
		}
		return e
	}
	return nil
}

func (m *uiModel) scrolled() {
	if m.view.AtBottom() {
		m.follow, m.unseen = true, 0
	} else {
		m.follow = false
	}
}

func nextLevel(level string) string {
	for i, known := range cockpit.ThinkingLevels {
		if known == level {
			return cockpit.ThinkingLevels[(i+1)%len(cockpit.ThinkingLevels)]
		}
	}
	return "high"
}

// ---------------------------------------------------------------- helpers

func (m *uiModel) title() tea.Cmd {
	title := orDefault(m.sessionTitle(), "new session")
	prefix := ""
	if m.state != idle {
		prefix = "● "
	}
	return tea.SetWindowTitle(prefix + cockpit.Clean(filepath.Base(m.opt.Workspace)) + " — " + title)
}

func shortID(id string) string {
	if len(id) > 8 {
		return id[:8]
	}
	return id
}

func orDefault(s, fallback string) string {
	if strings.TrimSpace(s) == "" {
		return fallback
	}
	return s
}

func ago(t time.Time) string {
	d := time.Since(t)
	switch {
	case t.IsZero():
		return ""
	case d < time.Minute:
		return "now"
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	case d < 7*24*time.Hour:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	}
	return t.Local().Format("Jan 2")
}

func size(n int64) string {
	switch {
	case n < 1024:
		return fmt.Sprintf("%d B", n)
	case n < 1<<20:
		return fmt.Sprintf("%.1f KB", float64(n)/1024)
	}
	return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
}

func tokens(n int64) string {
	switch {
	case n < 1000:
		return fmt.Sprint(n)
	case n < 1_000_000:
		return fmt.Sprintf("%.1fk", float64(n)/1000)
	}
	return fmt.Sprintf("%.2fM", float64(n)/1_000_000)
}

func clock(d time.Duration) string {
	s := int(d.Seconds())
	if s >= 3600 {
		return fmt.Sprintf("%d:%02d:%02d", s/3600, s/60%60, s%60)
	}
	return fmt.Sprintf("%02d:%02d", s/60, s%60)
}

func duration(d time.Duration) string {
	switch {
	case d < 0:
		return ""
	case d < time.Second:
		return fmt.Sprintf("%.1fs", d.Seconds())
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm %02ds", int(d.Minutes()), int(d.Seconds())%60)
	}
	return fmt.Sprintf("%dh %02dm", int(d.Hours()), int(d.Minutes())%60)
}
