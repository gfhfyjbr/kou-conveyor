package main

import (
	"context"
	"fmt"
	"maps"
	"os"
	"path"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The changes panel shows what the prompt in view changed in the workspace,
// file by file, at the right of the transcript: the files as a tree, and
// the diff of the one picked below them. Scrolling the transcript to another
// prompt shows that prompt's changes, and [ and ] step from prompt to
// prompt. The first file shows first; while a run goes on, the file it
// changed last does, until another is picked. Every run is recorded, open
// panel or not (see cockpit.Changes), and the web cockpit shows the same
// records.

type changesPanel struct {
	open    bool
	focused bool // the keys are the panel's
	// pinned is set when [ or ] picked the prompt: the panel stays on it
	// until the transcript is scrolled.
	pinned   bool
	session  string
	message  string      // the prompt shown, by message ID; "" for none
	list     *changeList // nil while it loads
	gen      int
	selected string // the file whose diff shows
	follow   bool   // show the file the run changed last
	rows     []changeRow
	cursor   int // the tree row the keys are on
	treeTop  int // the first tree row shown
	scroll   int // the first diff line shown
	folded   map[string]bool
	diffs    map[string]*fileDiff // by message, snapshot and path
	order    []string             // the keys of diffs, oldest first
	drawn    drawnDiff
	reload   int // the reload waiting to go, so a burst of snapshots makes one

	// width is the columns the panel was given by dragging its edge, or
	// with < and >; 0 leaves it its default, a share of the terminal.
	width int
	// dragFrom is the panel's width when the button went down on its edge,
	// as chosen (0 for the default) and as shown; edgeClick is when a click
	// on the edge let go last, for a double click.
	dragFrom  struct{ chosen, shown int }
	edgeClick time.Time
	saveGen   int // the saving of the width waiting to go, after keys
}

// changeList is what the run of a prompt changed, as the panel lists it.
type changeList struct {
	available      bool
	reason         string // why it is not
	before, tree   string // the snapshots compared
	latest         []string
	files          []cockpit.FileChange
	added, removed int
}

type fileDiff struct {
	loading   bool
	patch     string
	truncated bool
	err       error
}

// drawnDiff is the diff shown, as drawn for a width.
type drawnDiff struct {
	key    string
	width  int
	lines  []string
	gutter int // the columns of line numbers before the text
}

// changeRow is a row of the tree: a folder, or a file.
type changeRow struct {
	folder bool
	name   string // what the row shows
	path   string
	depth  int
	file   *cockpit.FileChange
}

type (
	changesLoadedMsg struct {
		gen  int
		list *changeList
	}
	diffLoadedMsg struct {
		key  string
		diff *fileDiff
	}
	changeUpdateMsg struct {
		tracker *cockpit.Tracker
		update  cockpit.ChangeUpdate
		closed  bool
	}
	changesReloadMsg  struct{ gen int }
	changesSummaryMsg struct{ list *changeList }
	panelWidthMsg     struct{ gen int }
)

// ---------------------------------------------------------------- recording

// changeRecords is where the runs' changes are recorded.
func (m *uiModel) changeRecords() *cockpit.Changes {
	if m.records == nil {
		m.records = cockpit.NewChanges(m.opt.Workspace, m.opt.SessionDir, m.opt.LogDir)
	}
	return m.records
}

// warmChanges takes a snapshot of the workspace in the background, so the
// one a run starts from finds most of the work done. Only where sessions
// were kept before: opening the cockpit somewhere leaves nothing behind.
func (m *uiModel) warmChanges() tea.Cmd {
	if _, err := os.Stat(m.opt.SessionDir); err != nil {
		return nil
	}
	records, ctx := m.changeRecords(), m.ctx
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
		defer cancel()
		_, _ = records.Snapshot(ctx)
		return nil
	}
}

// trackChanges takes the snapshot the run of a prompt starts from. A
// workspace that takes too long runs without its changes recorded, rather
// than keep the prompt waiting.
func (m *uiModel) trackChanges(message string) *cockpit.Tracker {
	ctx, cancel := context.WithTimeout(m.ctx, 2*time.Second)
	defer cancel()
	tracker, err := m.changeRecords().Begin(ctx, m.sessionID, message)
	if err != nil {
		return nil
	}
	return tracker
}

// waitChanges delivers what the run's snapshots find.
func waitChanges(t *cockpit.Tracker) tea.Cmd {
	return func() tea.Msg {
		update, ok := <-t.Updates()
		return changeUpdateMsg{tracker: t, update: update, closed: !ok}
	}
}

// pokeChanges asks for a snapshot when a tool call finished: it may have
// changed files.
func (m *uiModel) pokeChanges(entries []*cockpit.Entry) {
	if m.tracker == nil {
		return
	}
	for _, e := range entries {
		if e.Kind == cockpit.KindTool && e.Tool.Terminal() {
			m.tracker.Poke()
			return
		}
	}
}

func (m *uiModel) changeUpdate(msg changeUpdateMsg) tea.Cmd {
	if msg.closed {
		if m.tracker == msg.tracker {
			m.tracker = nil
		}
		return nil
	}
	cmds := []tea.Cmd{waitChanges(msg.tracker)}
	if m.changesShown() && m.changes.message == msg.update.Message {
		cmds = append(cmds, m.reloadChanges())
	}
	if msg.update.Final && (m.compact || !m.changesShown()) {
		records, session, message := m.changeRecords(), m.sessionID, msg.update.Message
		cmds = append(cmds, func() tea.Msg {
			return changesSummaryMsg{list: readChanges(records, session, message, nil)}
		})
	}
	return tea.Batch(cmds...)
}

// changesSummary says what a run that ended changed, when the panel is not
// there to show it: in the scrollback inline, in the status line otherwise.
func (m *uiModel) changesSummary(msg changesSummaryMsg) tea.Cmd {
	l := msg.list
	if !l.available || len(l.files) == 0 {
		return nil
	}
	st := m.styles
	if m.compact {
		var names []string
		for _, f := range l.files[:min(3, len(l.files))] {
			names = append(names, f.Path)
		}
		what := strings.Join(names, ", ")
		if more := len(l.files) - len(names); more > 0 {
			what += fmt.Sprintf(" and %d more", more)
		}
		pad := strings.Repeat(" ", gutter-2)
		line := pad + st.accent.Render("±") + " " + st.muted.Render(what) + "  " + changeCounts(st, l.added, l.removed) +
			st.faint.Render("  · ^G shows the diffs")
		m.scroll.queue = append(m.scroll.queue, pad+st.rule.Render("│"), fit(line, max(20, m.width-2)))
		return nil
	}
	if m.changesShown() {
		return nil
	}
	return m.notify(fmt.Sprintf("%s changed · +%d −%d — ^G shows the diffs", plural(len(l.files), "file", "files"), l.added, l.removed), "info")
}

// ---------------------------------------------------------------- the panel

// changesShown reports that the panel is on screen: open, in fullscreen.
func (m *uiModel) changesShown() bool { return m.changes.open && !m.compact && m.ready }

// toggleChanges opens the panel with the keys in it, or closes it. Inline
// there is no room for it: the cockpit takes a screen of its own first.
func (m *uiModel) toggleChanges() tea.Cmd {
	if m.compact {
		return tea.Batch(m.setLayout(cockpit.LayoutFullscreen), m.openChanges())
	}
	if m.changes.open {
		return m.closeChanges()
	}
	return m.openChanges()
}

func (m *uiModel) openChanges() tea.Cmd {
	c := &m.changes
	c.open, c.session, c.message, c.pinned = true, "", "", false
	m.focusChanges()
	m.saveChangesPanel(true)
	m.relayout()
	return m.syncChanges()
}

func (m *uiModel) closeChanges() tea.Cmd {
	c := &m.changes
	c.open, c.pinned = false, false
	m.blurChanges()
	m.saveChangesPanel(false)
	m.relayout()
	return nil
}

// saveChangesPanel makes the next start open the panel, or not.
func (m *uiModel) saveChangesPanel(open bool) {
	if m.opt.preferences == "" {
		return
	}
	if cockpit.SaveChangesPanel(m.opt.preferences, open) == nil {
		m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	}
}

// focusChanges gives the keys to the panel; blurChanges gives them back
// to the composer.
func (m *uiModel) focusChanges() {
	if m.changesShown() {
		m.changes.focused = true
		m.input.Blur()
	}
}

func (m *uiModel) blurChanges() {
	if m.changes.focused {
		m.changes.focused = false
		m.input.Focus()
	}
}

// relayout fits the transcript to the room the panel leaves it, keeping
// the entry at the top of the view in its place.
func (m *uiModel) relayout() {
	if !m.ready {
		return
	}
	id, into := "", 0
	for _, s := range m.spans {
		if s.end > m.view.YOffset {
			id, into = s.id, max(0, m.view.YOffset-s.start)
			break
		}
	}
	bottom := m.view.AtBottom()
	m.layout()
	m.refreshKeep()
	if bottom || m.follow {
		m.view.GotoBottom()
		return
	}
	for _, s := range m.spans {
		if s.id == id {
			m.view.SetYOffset(s.start + min(into, max(0, s.end-s.start-1)))
			return
		}
	}
}

// userScrolled notes that the reader scrolled the transcript: the panel
// follows it again.
func (m *uiModel) userScrolled() {
	m.changes.pinned = false
	m.scrolled()
}

func promptMessage(e *cockpit.Entry) string { return strings.TrimPrefix(e.ID, "input:") }

// promptInView is the prompt whose part of the transcript is in view: at
// the bottom, the newest; above it, the last that starts above a third of
// the way down.
func (m *uiModel) promptInView() *cockpit.Entry {
	var found *cockpit.Entry
	probe := m.view.YOffset + m.view.Height/3
	bottom := m.view.AtBottom()
	for _, s := range m.spans {
		e := m.tr.Entry(s.id)
		if e == nil || e.Kind != cockpit.KindUser {
			continue
		}
		if found != nil && !bottom && s.start > probe {
			break
		}
		found = e
	}
	return found
}

// syncChanges shows the changes of the prompt in view, when that is another
// than the one shown.
func (m *uiModel) syncChanges() tea.Cmd {
	c := &m.changes
	if !m.changesShown() || m.loading {
		return nil
	}
	if c.session == m.sessionID && c.pinned {
		return nil
	}
	c.pinned = false
	message := ""
	if e := m.promptInView(); e != nil {
		message = promptMessage(e)
	}
	if c.session == m.sessionID && c.message == message {
		return nil
	}
	return m.showChanges(message)
}

func (m *uiModel) showChanges(message string) tea.Cmd {
	c := &m.changes
	c.session, c.message = m.sessionID, message
	c.list, c.rows, c.selected, c.follow = nil, nil, "", true
	c.cursor, c.treeTop, c.scroll = 0, 0, 0
	c.gen++ // what is loading is another prompt's
	if message == "" {
		return nil
	}
	return m.loadChanges()
}

// stepPrompt shows the changes of the prompt before or after the one shown,
// and scrolls the transcript to it.
func (m *uiModel) stepPrompt(step int) tea.Cmd {
	c := &m.changes
	var prompts []*cockpit.Entry
	at := -1
	for _, e := range m.tr.Entries {
		if e.Kind != cockpit.KindUser {
			continue
		}
		if promptMessage(e) == c.message {
			at = len(prompts)
		}
		prompts = append(prompts, e)
	}
	if len(prompts) == 0 {
		return m.notify("there are no prompts yet", "info")
	}
	next := at + step
	if at < 0 {
		next = len(prompts) - 1
	}
	if next < 0 || next >= len(prompts) {
		which := "first"
		if step > 0 {
			which = "last"
		}
		return m.notify("this is the "+which+" prompt", "info")
	}
	c.pinned = true
	m.reveal(prompts[next].ID)
	return m.showChanges(promptMessage(prompts[next]))
}

// changesLive reports that the prompt shown is the one running.
func (m *uiModel) changesLive() bool {
	c := &m.changes
	switch {
	case c.message == "" || c.session != m.sessionID:
		return false
	case m.state != idle:
		return c.message == m.runMessage
	case m.external:
		last := m.lastPrompt()
		return last != nil && promptMessage(last) == c.message
	}
	return false
}

// liveChanges reloads the changes of a run another window is running,
// which only its snapshots tell about.
func (m *uiModel) liveChanges() tea.Cmd {
	if !m.changesShown() || !m.changesLive() {
		return nil
	}
	return m.loadChanges()
}

// reloadChanges reloads the changes shown in a moment: snapshots come in
// bursts.
func (m *uiModel) reloadChanges() tea.Cmd {
	c := &m.changes
	c.reload++
	gen := c.reload
	return tea.Tick(120*time.Millisecond, func(time.Time) tea.Msg { return changesReloadMsg{gen} })
}

func (m *uiModel) loadChanges() tea.Cmd {
	c := &m.changes
	c.gen++
	gen, records, session, message, known := c.gen, m.changeRecords(), c.session, c.message, c.list
	return func() tea.Msg {
		return changesLoadedMsg{gen: gen, list: readChanges(records, session, message, known)}
	}
}

// readChanges reads what the run of a prompt changed. known is what was
// read before: its files hold as long as the snapshots are the same.
func readChanges(records *cockpit.Changes, session, message string, known *changeList) *changeList {
	l := &changeList{reason: "No changes were recorded for this prompt."}
	ex, ok := records.Exchange(session, message)
	if !ok {
		if err := records.Available(); err != nil {
			l.reason = cockpit.Sentence(err.Error())
		}
		return l
	}
	l.available, l.before, l.tree, l.latest = true, ex.Before, ex.After, ex.Latest
	if known != nil && known.available && known.before == l.before && known.tree == l.tree {
		l.files, l.added, l.removed = known.files, known.added, known.removed
		return l
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	files, err := records.Files(ctx, ex.Before, ex.After)
	if err != nil {
		l.available, l.reason = false, cockpit.Sentence("cannot list the changes: "+err.Error())
		return l
	}
	l.files = files
	for _, f := range files {
		l.added += f.Added
		l.removed += f.Removed
	}
	return l
}

func (m *uiModel) changesLoaded(msg changesLoadedMsg) tea.Cmd {
	c := &m.changes
	if msg.gen != c.gen {
		return nil
	}
	cursorPath := ""
	if c.cursor < len(c.rows) {
		cursorPath = c.rows[c.cursor].path
	}
	c.list = msg.list
	c.rows = changeRows(c.list.files, c.folded)
	order := fileOrder(c.list.files)
	latest := ""
	if m.changesLive() {
		for _, p := range c.list.latest {
			if slices.Contains(order, p) {
				latest = p
				break
			}
		}
	}
	switch {
	case c.follow && latest != "":
		m.selectFile(latest)
		cursorPath = latest
	case !slices.Contains(order, c.selected):
		first := ""
		if len(order) > 0 {
			first = order[0]
		}
		m.selectFile(first)
		cursorPath = first
	}
	m.cursorTo(cursorPath)
	return m.loadDiff()
}

// selectFile shows a file's diff from its top.
func (m *uiModel) selectFile(path string) {
	c := &m.changes
	if c.selected != path {
		c.scroll = 0
	}
	c.selected = path
}

// pickFile shows the diff of a file the reader picked. It stops following
// the run until f.
func (m *uiModel) pickFile(path string) tea.Cmd {
	m.selectFile(path)
	if m.changesLive() {
		m.changes.follow = false
	}
	return m.loadDiff()
}

func (m *uiModel) followChanges() tea.Cmd {
	m.changes.follow = true
	return m.loadChanges()
}

// cursorTo puts the keys' cursor on a row, and the row in view.
func (m *uiModel) cursorTo(path string) {
	c := &m.changes
	for i, r := range c.rows {
		if r.path == path {
			c.cursor = i
			break
		}
	}
	c.cursor = clamp(c.cursor, 0, max(0, len(c.rows)-1))
	if g, ok := m.panelGeometry(); ok && g.treeRows > 0 {
		if c.cursor < c.treeTop {
			c.treeTop = c.cursor
		}
		if c.cursor >= c.treeTop+g.treeRows {
			c.treeTop = c.cursor - g.treeRows + 1
		}
	}
}

// file is the file whose diff shows.
func (c *changesPanel) file() *cockpit.FileChange {
	if c.list == nil {
		return nil
	}
	for i := range c.list.files {
		if c.list.files[i].Path == c.selected {
			return &c.list.files[i]
		}
	}
	return nil
}

func (c *changesPanel) diffKey(path string) string {
	return c.message + " " + c.list.tree + " " + path
}

// keptDiffs is how many diffs the panel keeps; each may be half a megabyte.
const keptDiffs = 40

// remember keeps a diff. A live run makes one per snapshot, so the oldest
// go.
func (c *changesPanel) remember(key string, d *fileDiff) {
	if c.diffs == nil {
		c.diffs = map[string]*fileDiff{}
	}
	if _, ok := c.diffs[key]; !ok {
		c.order = append(c.order, key)
		for len(c.order) > keptDiffs {
			delete(c.diffs, c.order[0])
			c.order = c.order[1:]
		}
	}
	c.diffs[key] = d
}

func (m *uiModel) loadDiff() tea.Cmd {
	c := &m.changes
	f := c.file()
	if f == nil || f.Binary {
		return nil
	}
	key := c.diffKey(f.Path)
	if _, ok := c.diffs[key]; ok {
		return nil
	}
	c.remember(key, &fileDiff{loading: true})
	records, before, after, file, old := m.changeRecords(), c.list.before, c.list.tree, f.Path, f.OldPath
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		patch, truncated, err := records.Diff(ctx, before, after, file, old)
		// What files hold is shown, never obeyed: no escape sequences.
		return diffLoadedMsg{key: key, diff: &fileDiff{patch: cockpit.Clean(patch), truncated: truncated, err: err}}
	}
}

func (m *uiModel) diffLoaded(msg diffLoadedMsg) {
	c := &m.changes
	if _, ok := c.diffs[msg.key]; ok {
		c.diffs[msg.key] = msg.diff
	}
}

// ---------------------------------------------------------------- the tree

type changeFolder struct {
	folders map[string]*changeFolder
	files   []*cockpit.FileChange
}

// changeRows lays files out as a tree: folders first, then files, by name.
// A folder that holds nothing but another folder shares its row, and the
// files of folded folders are left out.
func changeRows(files []cockpit.FileChange, folded map[string]bool) []changeRow {
	root := &changeFolder{folders: map[string]*changeFolder{}}
	for i := range files {
		f := &files[i]
		node := root
		parts := strings.Split(f.Path, "/")
		for _, part := range parts[:len(parts)-1] {
			next := node.folders[part]
			if next == nil {
				next = &changeFolder{folders: map[string]*changeFolder{}}
				node.folders[part] = next
			}
			node = next
		}
		node.files = append(node.files, f)
	}
	var rows []changeRow
	var walk func(node *changeFolder, at string, depth int)
	walk = func(node *changeFolder, at string, depth int) {
		for _, name := range slices.Sorted(maps.Keys(node.folders)) {
			folder, label, full := node.folders[name], name, path.Join(at, name)
			for len(folder.files) == 0 && len(folder.folders) == 1 {
				only := slices.Collect(maps.Keys(folder.folders))[0]
				folder, label, full = folder.folders[only], label+"/"+only, full+"/"+only
			}
			rows = append(rows, changeRow{folder: true, name: label + "/", path: full, depth: depth})
			if !folded[full] {
				walk(folder, full, depth+1)
			}
		}
		for _, f := range node.files {
			rows = append(rows, changeRow{name: path.Base(f.Path), path: f.Path, depth: depth, file: f})
		}
	}
	walk(root, "", 0)
	return rows
}

// fileOrder lists the files as the unfolded tree shows them.
func fileOrder(files []cockpit.FileChange) []string {
	var order []string
	for _, r := range changeRows(files, nil) {
		if !r.folder {
			order = append(order, r.path)
		}
	}
	return order
}

// fold folds or unfolds a folder of the tree.
func (m *uiModel) fold(folder string, folded bool) {
	c := &m.changes
	if c.list == nil {
		return
	}
	if c.folded == nil {
		c.folded = map[string]bool{}
	}
	if folded {
		c.folded[folder] = true
	} else {
		delete(c.folded, folder)
	}
	c.rows = changeRows(c.list.files, c.folded)
	m.cursorTo(folder)
}

// moveCursor moves the keys' cursor along the tree; the file it lands on
// shows.
func (m *uiModel) moveCursor(step int) tea.Cmd {
	c := &m.changes
	if len(c.rows) == 0 {
		return nil
	}
	c.cursor = clamp(c.cursor+step, 0, len(c.rows)-1)
	r := c.rows[c.cursor]
	m.cursorTo(r.path)
	if r.folder {
		return nil
	}
	return m.pickFile(r.path)
}

// ---------------------------------------------------------------- layout

// The panel's width. Beside the transcript it takes a share of the terminal
// unless its edge was dragged; a drag leaves both it and the transcript
// room, and keys move the edge a few columns at a time.
const (
	sideBySide           = 110 // the narrowest terminal with the panel beside the transcript
	panelMinColumns      = 36
	transcriptMinColumns = 40
	panelStep            = 4
	doubleClick          = 500 * time.Millisecond
)

// panelColumns is how wide the panel is, its border included, and whether
// it covers the transcript, as it does where the two do not fit side by
// side; 0 when it is not on screen.
func (m *uiModel) panelColumns() (int, bool) {
	switch {
	case !m.changesShown():
		return 0, false
	case m.width < sideBySide:
		return m.width, true
	case m.changes.width > 0:
		return clamp(m.changes.width, panelMinColumns, m.width-transcriptMinColumns), false
	}
	return clamp(m.width*9/20, 44, 120), false
}

// onPanelEdge reports whether a cell is on the panel's edge, the border it
// has beside the transcript, which drags.
func (m *uiModel) onPanelEdge(x, y int) bool {
	g, ok := m.panelGeometry()
	return ok && g.left > 0 && x == g.left && y >= g.top && y < g.top+g.height
}

// setPanelWidth gives the panel a width, within what leaves the transcript
// its room, and fits the transcript to what is left.
func (m *uiModel) setPanelWidth(columns int) {
	before, _ := m.panelColumns()
	m.changes.width = clamp(columns, panelMinColumns, max(panelMinColumns, m.width-transcriptMinColumns))
	if after, _ := m.panelColumns(); after != before {
		m.relayout()
	}
}

// stepPanel moves the panel's edge by columns, to the left for more; the
// width is saved once the keys stop for a moment, so a key held down saves
// it once.
func (m *uiModel) stepPanel(columns int) tea.Cmd {
	pw, covers := m.panelColumns()
	if pw == 0 {
		return nil
	}
	if covers {
		return m.notify(fmt.Sprintf("the changes cover the transcript below %d columns; there is no edge to move", sideBySide), "info")
	}
	m.setPanelWidth(pw + columns)
	m.changes.saveGen++
	gen := m.changes.saveGen
	return tea.Tick(400*time.Millisecond, func(time.Time) tea.Msg { return panelWidthMsg{gen} })
}

// panelWidthSettled saves the width the keys left the panel.
func (m *uiModel) panelWidthSettled(msg panelWidthMsg) {
	if msg.gen == m.changes.saveGen {
		m.saveChangesWidth()
	}
}

// restorePanelWidth gives the panel its default width back.
func (m *uiModel) restorePanelWidth() {
	if m.changes.width == 0 {
		return
	}
	m.changes.width = 0
	m.changes.saveGen++ // a save the keys left waiting is this one's
	m.relayout()
	m.saveChangesWidth()
}

// pressEdge starts a drag of the panel's edge.
func (m *uiModel) pressEdge() {
	c := &m.changes
	c.dragFrom.chosen = c.width
	c.dragFrom.shown, _ = m.panelColumns()
}

// edgeReleased ends a drag of the panel's edge: a width that changed is
// kept, and a double click gives the panel its default width back.
func (m *uiModel) edgeReleased() {
	c := &m.changes
	if pw, _ := m.panelColumns(); pw != c.dragFrom.shown {
		c.edgeClick = time.Time{}
		c.saveGen++
		m.saveChangesWidth()
		return
	}
	// Back where it started, the edge was clicked: a default stays one.
	c.width = c.dragFrom.chosen
	if time.Since(c.edgeClick) < doubleClick {
		c.edgeClick = time.Time{}
		m.restorePanelWidth()
		return
	}
	c.edgeClick = time.Now()
}

// saveChangesWidth makes the next start give the panel the width it has.
func (m *uiModel) saveChangesWidth() {
	if m.opt.preferences == "" {
		return
	}
	if cockpit.SaveChangesWidth(m.opt.preferences, m.changes.width) == nil {
		m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	}
}

// panelGeometry is where the parts of the panel are on screen. Rows are
// screen rows; the text starts at the column content.
type panelGeometry struct {
	left, width       int
	content, inner    int // the text's first column and its width
	top, height       int
	treeTop, treeRows int
	diffHead          int // the row of the file's name above its diff, or -1
	diffTop, diffRows int
}

func (m *uiModel) panelGeometry() (panelGeometry, bool) {
	pw, _ := m.panelColumns()
	if pw == 0 || m.picker != nil || m.form != nil {
		return panelGeometry{}, false
	}
	// Beside the transcript the panel runs down to the bottom of the
	// screen, as the web cockpit's; covering it, it takes the transcript's
	// rows and leaves the dock.
	g := panelGeometry{left: m.width - pw, width: pw, top: m.top(), height: m.height - m.top(), diffHead: -1}
	if g.left == 0 {
		g.height = m.view.Height
	}
	g.content, g.inner = g.left+2, max(1, pw-3)
	body := g.height - 1
	c := &m.changes
	if l := c.list; l != nil && l.available && len(l.files) > 0 && body >= 2 {
		g.treeTop = g.top + 1
		g.treeRows = max(1, min(len(c.rows), max(3, body/3), body-2))
		g.diffHead = g.treeTop + g.treeRows
		g.diffTop = g.diffHead + 1
		g.diffRows = max(0, g.top+g.height-g.diffTop)
		return g, true
	}
	g.diffTop, g.diffRows = g.top+1, max(0, body)
	return g, true
}

// inPanel reports a screen cell the panel covers.
func (m *uiModel) inPanel(x, y int) bool {
	g, ok := m.panelGeometry()
	return ok && x >= g.left && y >= g.top && y < g.top+g.height
}

// ---------------------------------------------------------------- drawing

// changesView draws the panel: a line per transcript row, each as wide as
// the panel.
func (m *uiModel) changesView(g panelGeometry, hover hoverTarget) []string {
	st := m.styles
	c := &m.changes
	border := st.rule.Render("│")
	switch {
	case hover.kind == hoverPanelEdge:
		border = st.accent.Render("┃")
	case c.focused:
		border = st.accent.Render("│")
	}
	out := make([]string, g.height)
	put := func(row int, text, bar string) {
		if row < 0 || row >= g.height {
			return
		}
		text = fit(text, g.inner)
		out[row] = border + " " + text + strings.Repeat(" ", max(0, g.inner-ansi.StringWidth(text))) + bar
	}
	hovered := func(row int) bool { return hover.row == g.top+row }

	header, closeAt, followAt := m.changesHeader(g.inner)
	if hovered(0) && hover.kind == hoverPanelButton {
		at := closeAt
		if hover.id == "follow" {
			at = followAt
		}
		header = m.lightUp(header, at[0], at[1])
	}
	put(0, header, " ")

	lines, _, note := m.diffView(g.inner)
	if g.treeRows > 0 {
		c.treeTop = clamp(c.treeTop, 0, max(0, len(c.rows)-g.treeRows))
		latest := map[string]bool{}
		if m.changesLive() {
			for _, p := range c.list.latest {
				latest[p] = true
			}
		}
		for i := range g.treeRows {
			at := c.treeTop + i
			if at >= len(c.rows) {
				put(1+i, "", " ")
				continue
			}
			r := c.rows[at]
			line := m.treeLine(r, g.inner, latest[r.path])
			switch {
			case !r.folder && r.path == c.selected,
				c.focused && at == c.cursor,
				hovered(1+i) && hover.kind == hoverChangeRow:
				line = m.lightUp(line, 0, g.inner)
			}
			put(1+i, line, " ")
		}
		if f := c.file(); f != nil {
			put(g.diffHead-g.top, m.diffHeading(f, g.inner), " ")
		}
	}
	if note == "" && lines == nil {
		note = m.changesNote()
	}
	if note != "" {
		for i, line := range wrapText(note, st.faint, g.inner, "", "") {
			put(g.diffTop-g.top+1+i, line, " ")
		}
	} else {
		c.scroll = clamp(c.scroll, 0, max(0, len(lines)-g.diffRows))
		thumbStart, thumbEnd := scrollThumb(len(lines), g.diffRows, c.scroll)
		for i := range g.diffRows {
			at := c.scroll + i
			line, bar := "", " "
			if at < len(lines) {
				line = m.selected(inDiff, at, lines[at], 0)
			}
			if len(lines) > g.diffRows {
				bar = st.rule.Render("│")
				if i >= thumbStart && i < thumbEnd {
					bar = st.muted.Render("┃")
				}
			}
			put(g.diffTop-g.top+i, line, bar)
		}
	}
	for i := range out {
		if out[i] == "" {
			out[i] = border + strings.Repeat(" ", g.width-1)
		}
	}
	return out
}

// scrollThumb is where the thumb of a scrollbar is: rows [start, end) of
// its height.
func scrollThumb(total, height, offset int) (int, int) {
	if total <= height || height <= 0 {
		return 0, height
	}
	size := max(1, height*height/total)
	start := (height - size) * offset / max(1, total-height)
	return start, start + size
}

// changesNote says what the panel shows when it has no diff to.
func (m *uiModel) changesNote() string {
	c := &m.changes
	l := c.list
	switch {
	case c.message == "":
		return "What the agent changes in the workspace shows here: the changes of the prompt in view, file by file, as the transcript scrolls to it."
	case l == nil:
		return "Loading…"
	case !l.available:
		return l.reason
	case len(l.files) == 0 && m.changesLive():
		return "No files changed yet."
	case len(l.files) == 0:
		return "This prompt changed no files."
	}
	return ""
}

// changesHeader is the panel's first row, and the columns of its buttons in
// it: close, and follow the run, [0, 0) when there is none.
func (m *uiModel) changesHeader(width int) (line string, closeAt, followAt [2]int) {
	st := m.styles
	c := &m.changes
	label := st.label.Render("CHANGES")
	if c.focused {
		label = st.accentLabel.Render("CHANGES")
	}
	left := label
	if n := m.promptNumber("input:" + c.message); n > 0 {
		left += st.faint.Render(" · ") + st.muted.Render(fmt.Sprintf("PROMPT %02d", n))
	}
	live := m.changesLive()
	if live {
		left += "  " + st.accentLabel.Render("● LIVE")
	}
	right := ""
	if l := c.list; l != nil && l.available && len(l.files) > 0 {
		right = changeCounts(st, l.added, l.removed) + st.faint.Render(" · "+plural(len(l.files), "file", "files")) + "  "
	}
	follow := ""
	if live && !c.follow {
		follow = st.accent.Render("↻ follow")
		right = follow + "  " + right
	}
	right += st.muted.Render("×")
	line = fitRight(left, right, width)
	end := ansi.StringWidth(line)
	closeAt = [2]int{end - 1, end}
	if follow != "" {
		start := end - ansi.StringWidth(right)
		followAt = [2]int{start, start + ansi.StringWidth(follow)}
	}
	return line, closeAt, followAt
}

// treeLine draws a row of the tree.
func (m *uiModel) treeLine(r changeRow, width int, latest bool) string {
	st := m.styles
	c := &m.changes
	indent := strings.Repeat("  ", r.depth)
	if r.folder {
		chevron := "▾"
		if c.folded[r.path] {
			chevron = "▸"
		}
		return " " + indent + st.faint.Render(chevron) + " " + st.muted.Render(r.name)
	}
	f := r.file
	mark := " "
	if f.Path == c.selected {
		mark = st.accent.Render("▌")
	}
	left := mark + indent + statusLetter(st, f.Status) + " " + st.text.Render(r.name)
	if latest {
		left += st.accent.Render(" ●")
	}
	right := changeCounts(st, f.Added, f.Removed)
	if f.Binary {
		right = st.faint.Render("binary")
	}
	return fitRight(left, right, width)
}

// diffHeading names the file above its diff.
func (m *uiModel) diffHeading(f *cockpit.FileChange, width int) string {
	st := m.styles
	right := ""
	if !f.Binary && (f.Added > 0 || f.Removed > 0) {
		right = " " + changeCounts(st, f.Added, f.Removed)
	}
	name := f.Path
	if f.OldPath != "" {
		name = f.OldPath + " → " + f.Path
	}
	room := max(4, width-ansi.StringWidth(right)-4)
	if ansi.StringWidth(name) > room {
		// The end of a path says the most.
		name = ansi.TruncateLeft(name, ansi.StringWidth(name)-room+1, "…")
	}
	left := st.rule.Render("─ ") + st.text.Render(name) + " "
	fill := max(0, width-ansi.StringWidth(left)-ansi.StringWidth(right))
	return left + st.rule.Render(strings.Repeat("─", fill)) + right
}

func statusLetter(st styles, status string) string {
	switch status {
	case "added":
		return st.ok.Bold(true).Render("A")
	case "modified":
		return st.warn.Bold(true).Render("M")
	case "deleted":
		return st.err.Bold(true).Render("D")
	case "renamed":
		return st.accent.Bold(true).Render("R")
	case "typechange":
		return st.accent.Bold(true).Render("T")
	}
	return st.faint.Render("?")
}

func changeCounts(st styles, added, removed int) string {
	return st.ok.Render(fmt.Sprintf("+%d", added)) + " " + st.err.Render(fmt.Sprintf("−%d", removed))
}

func plural(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return strconv.Itoa(n) + " " + many
}

// diffView is the diff of the file shown, drawn for a width, and its gutter;
// or a note that says why there is none.
func (m *uiModel) diffView(width int) (lines []string, gutter int, note string) {
	c := &m.changes
	f := c.file()
	switch {
	case f == nil:
		return nil, 0, ""
	case f.Binary:
		return nil, 0, "A binary file; its content is not shown."
	}
	key := c.diffKey(f.Path)
	d := c.diffs[key]
	switch {
	case d == nil || d.loading:
		return nil, 0, "Loading…"
	case d.err != nil:
		return nil, 0, cockpit.Sentence("cannot show the diff: " + d.err.Error())
	}
	if c.drawn.key != key || c.drawn.width != width {
		lines, gutter := diffLines(m.styles, d.patch, width)
		if d.truncated {
			lines = append(lines, "", m.styles.faint.Render("The diff goes on; the rest is not shown."))
		}
		c.drawn = drawnDiff{key: key, width: width, lines: lines, gutter: gutter}
	}
	if len(c.drawn.lines) == 0 {
		if f.Status == "renamed" {
			return nil, 0, "Renamed; the content is the same."
		}
		return nil, 0, "Only the file's mode changed."
	}
	return c.drawn.lines, c.drawn.gutter, ""
}

var hunkHeader = regexp.MustCompile(`^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$`)

// diffLines draws a unified diff with each line's numbers, old and new, in
// a gutter before it; long lines wrap below it. The patch's own headers are
// left out: the panel names the file.
func diffLines(st styles, patch string, width int) ([]string, int) {
	patchLines := strings.Split(strings.TrimSuffix(patch, "\n"), "\n")
	widest := 1
	for _, line := range patchLines {
		if h := hunkHeader.FindStringSubmatch(line); h != nil {
			for _, at := range [][2]string{{h[1], h[2]}, {h[3], h[4]}} {
				start, _ := strconv.Atoi(at[0])
				count := 1
				if at[1] != "" {
					count, _ = strconv.Atoi(at[1])
				}
				widest = max(widest, len(strconv.Itoa(start+count)))
			}
		}
	}
	digits := max(3, widest)
	gutter := 2*digits + 4 // "old new ± "
	room := max(8, width-gutter)
	number := func(n int) string {
		if n <= 0 {
			return strings.Repeat(" ", digits)
		}
		return fmt.Sprintf("%*d", digits, n)
	}
	var out []string
	add := func(old, new int, sign string, signStyle, textStyle lipgloss.Style, text string) {
		text = strings.ReplaceAll(text, "\t", "    ")
		for i, part := range strings.Split(ansi.Hardwrap(text, room, true), "\n") {
			lead := st.faint.Render(number(old)+" "+number(new)+" ") + signStyle.Render(sign) + " "
			if i > 0 {
				lead = strings.Repeat(" ", gutter)
			}
			// The line's colour runs to the edge, like a highlighted row.
			out = append(out, lead+textStyle.Render(part+strings.Repeat(" ", max(0, room-ansi.StringWidth(part)))))
		}
	}
	old, new, inHunk := 0, 0, false
	for _, line := range patchLines {
		if h := hunkHeader.FindStringSubmatch(line); h != nil {
			old, _ = strconv.Atoi(h[1])
			new, _ = strconv.Atoi(h[3])
			inHunk = true
			// A band, as in the web cockpit, where each part of the file starts.
			head := fit(line, width)
			out = append(out, st.diffHunk.Render(head+strings.Repeat(" ", max(0, width-ansi.StringWidth(head)))))
			continue
		}
		switch {
		case strings.HasPrefix(line, "diff --git "):
			inHunk = false // another file's headers follow
		case !inHunk:
		case strings.HasPrefix(line, "+"):
			add(0, new, "+", st.ok, st.diffAdd, line[1:])
			new++
		case strings.HasPrefix(line, "-"):
			add(old, 0, "−", st.err, st.diffDel, line[1:])
			old++
		case strings.HasPrefix(line, " ") || line == "":
			add(old, new, " ", st.faint, st.muted, strings.TrimPrefix(line, " "))
			old++
			new++
		case strings.HasPrefix(line, `\`):
			out = append(out, strings.Repeat(" ", gutter)+st.faint.Italic(true).Render(strings.TrimSpace(strings.TrimPrefix(line, `\`))))
		}
	}
	return out, gutter
}

// scrollDiff scrolls the diff by n lines.
func (m *uiModel) scrollDiff(n int) {
	g, ok := m.panelGeometry()
	if !ok {
		return
	}
	lines, _, _ := m.diffView(g.inner)
	c := &m.changes
	c.scroll = clamp(c.scroll+n, 0, max(0, len(lines)-g.diffRows))
}

// ---------------------------------------------------------------- input

// changesKey handles a key while the panel has the keys. It reports false
// for the keys it leaves to the composer, which then takes the keys back.
func (m *uiModel) changesKey(msg tea.KeyMsg) (tea.Cmd, bool) {
	c := &m.changes
	g, _ := m.panelGeometry()
	half := max(1, g.diffRows/2)
	onFolder := c.cursor < len(c.rows) && c.rows[c.cursor].folder
	switch msg.String() {
	case "up":
		return m.moveCursor(-1), true
	case "down":
		return m.moveCursor(1), true
	case "left", "right":
		if onFolder {
			m.fold(c.rows[c.cursor].path, msg.String() == "left")
		}
	case "enter", " ":
		// On a file they are the composer's: enter runs the prompt.
		if !onFolder {
			return nil, false
		}
		r := c.rows[c.cursor]
		m.fold(r.path, !c.folded[r.path])
	case "pgup":
		m.scrollDiff(-half)
	case "pgdown":
		m.scrollDiff(half)
	case "shift+up":
		m.scrollDiff(-1)
	case "shift+down":
		m.scrollDiff(1)
	case "home":
		c.scroll = 0
	case "end":
		m.scrollDiff(1 << 30)
	case "[":
		return m.stepPrompt(-1), true
	case "]":
		return m.stepPrompt(1), true
	case "f":
		if !m.changesLive() {
			return nil, false
		}
		return m.followChanges(), true
	case "<", "shift+left":
		return m.stepPanel(panelStep), true
	case ">", "shift+right":
		return m.stepPanel(-panelStep), true
	case "=":
		m.restorePanelWidth()
	case "tab", "esc", "alt+esc":
		m.blurChanges()
	case "ctrl+g":
		return m.closeChanges(), true
	default:
		return nil, false
	}
	return nil, true
}

// changesWheel scrolls the part of the panel under the pointer.
func (m *uiModel) changesWheel(y int, up bool) {
	g, ok := m.panelGeometry()
	if !ok {
		return
	}
	c := &m.changes
	if y >= g.treeTop && y < g.treeTop+g.treeRows {
		step := 1
		if up {
			step = -1
		}
		c.treeTop = clamp(c.treeTop+step, 0, max(0, len(c.rows)-g.treeRows))
		return
	}
	if up {
		m.scrollDiff(-3)
	} else {
		m.scrollDiff(3)
	}
}

// changesClick acts on a click in the panel, which takes the keys.
func (m *uiModel) changesClick(x, y int) tea.Cmd {
	g, ok := m.panelGeometry()
	if !ok {
		return nil
	}
	c := &m.changes
	m.focusChanges()
	switch {
	case y == g.top:
		_, closeAt, followAt := m.changesHeader(g.inner)
		col := x - g.content
		switch {
		case col >= closeAt[0] && col < closeAt[1]:
			return m.closeChanges()
		case col >= followAt[0] && col < followAt[1]:
			return m.followChanges()
		}
	case y >= g.treeTop && y < g.treeTop+g.treeRows:
		at := c.treeTop + y - g.treeTop
		if at >= len(c.rows) {
			return nil
		}
		c.cursor = at
		if r := c.rows[at]; r.folder {
			m.fold(r.path, !c.folded[r.path])
			return nil
		}
		return m.pickFile(c.rows[at].path)
	}
	return nil
}

// changesHover is what the pointer is over in the panel.
func (m *uiModel) changesHover(x, y int) hoverTarget {
	at := hoverTarget{row: y, level: -1}
	g, ok := m.panelGeometry()
	if !ok {
		return at
	}
	c := &m.changes
	col := x - g.content
	switch {
	case y == g.top:
		_, closeAt, followAt := m.changesHeader(g.inner)
		switch {
		case col >= closeAt[0] && col < closeAt[1]:
			at.kind, at.id = hoverPanelButton, "close"
		case col >= followAt[0] && col < followAt[1]:
			at.kind, at.id = hoverPanelButton, "follow"
		}
	case y >= g.treeTop && y < g.treeTop+g.treeRows:
		if i := c.treeTop + y - g.treeTop; i < len(c.rows) {
			at.kind, at.id, at.folder = hoverChangeRow, c.rows[i].path, c.rows[i].folder
		}
	case y >= g.diffTop && y < g.diffTop+g.diffRows:
		lines, _, note := m.diffView(g.inner)
		if i := c.scroll + y - g.diffTop; note == "" && i < len(lines) && col >= 0 && col < ansi.StringWidth(strings.TrimRight(ansi.Strip(lines[i]), " ")) {
			at.kind = hoverText
		}
	}
	return at
}
