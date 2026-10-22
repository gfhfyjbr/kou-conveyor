package main

// A surface of the genui plugin (examples/plugins/genui) in the transcript:
// the block of a UI call draws the surface the call showed with the
// terminal's means, as the web cockpit draws it with its own — metrics as a
// strip, charts in block characters, tables, code, diffs, inputs and
// buttons. The newest call of a surface is live: the files it reads are
// looked at again every second and a half, off the loop, and its inputs and
// buttons work — ctrl+] or a click focuses them, and a button's event
// reaches the agent as the user's message. The calls before it show what
// their surface showed then. An Html shows its alt: the page itself draws in
// the web cockpit and the iPhone app.

import (
	"errors"
	"fmt"
	"maps"
	"math"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/surface"
)

const (
	// surfaceRows are the lines a Markdown, a Code or a Diff shows at most.
	surfaceRows = 16
	// surfacePoll is how often a live surface's files are looked at.
	surfacePoll = 1500 * time.Millisecond
)

// surfaceTickMsg is the time to look at the live surfaces' files again.
type surfaceTickMsg struct{}

// surfaceReadMsg is what a look at the files found changed.
type surfaceReadMsg struct {
	live  map[string]*surfaceLive // by version path
	codes map[string]*surfaceCode // by path#id
}

type surfaceCallInfo struct {
	rev  int
	call surface.Call
	ok   bool
}

type surfaceVersion struct {
	v     *surface.Object
	comps map[string]*surface.Object
	err   error
}

// surfaceLive is what a live surface's files say now.
type surfaceLive struct {
	sources, infos *surface.Object
	stamp          string // the files' sizes and times when read
}

// surfaceCode is the file a Code shows, read and highlighted.
type surfaceCode struct {
	file, text string
	from       int
	cut        bool
	err        error
	runs       []int32
	stamp      string
}

type surfaceCodeJob struct{ key, file, lines, stamp string }

// surfaceJob is what a look at a surface's files reads.
type surfaceJob struct {
	path    string
	version *surface.Object
	sources bool
	stamp   string
	codes   []surfaceCodeJob
}

// surfaceControl is where a control was drawn in its block's card: on a
// row of the card's body, from a column of it to another.
type surfaceControl struct {
	id, kind      string
	index         int // the tab of a Tabs
	row, from, to int
}

// surfaceFocus is the control the keys go to.
type surfaceFocus struct {
	entry, control string
	cursor         int // a TextField's, in runes; -1 at the end
}

// surfaceBook is what the cockpit keeps of the transcript's surfaces.
type surfaceBook struct {
	root    *os.Root // the workspace, which surfaces read from
	rootErr error
	reader  *surface.Reader

	calls    map[string]surfaceCallInfo // by entry: the surface its call showed
	versions map[string]surfaceVersion  // by version path, read once
	// heads are the entries of the surfaces' newest calls, by surface (nil
	// until counted); paths the entry of each version.
	heads, paths map[string]string

	live   map[string]*surfaceLive   // by version path: a head's files now
	codes  map[string]*surfaceCode   // by path#id: the file a Code shows
	wanted map[string]surfaceCodeJob // the Codes a drawing asked for
	edits  map[string][]surface.Edit // by version path: what its inputs set
	used   map[string]bool           // path#id: a once Button pressed
	folds  map[string]bool           // path#id: a Disclosure opened or closed
	tabs   map[string]int            // path#id: the tab a Tabs shows

	controls map[string][]surfaceControl // by entry, as last drawn
	focus    *surfaceFocus

	ticking, reading bool
	// clicked tells the mouse's release that it hit a control, and so
	// leaves the focus where the click put it.
	clicked bool
}

func (b *surfaceBook) init() {
	if b.calls != nil {
		return
	}
	b.reader = surface.NewReader()
	b.calls, b.versions = map[string]surfaceCallInfo{}, map[string]surfaceVersion{}
	b.paths = map[string]string{}
	b.live, b.codes, b.wanted = map[string]*surfaceLive{}, map[string]*surfaceCode{}, map[string]surfaceCodeJob{}
	b.edits = map[string][]surface.Edit{}
	b.used, b.folds, b.tabs = map[string]bool{}, map[string]bool{}, map[string]int{}
	b.controls = map[string][]surfaceControl{}
}

// set keeps a value an input set, in place of the one it set before.
func (b *surfaceBook) set(path, pointer string, value any) {
	edits := b.edits[path]
	for i := range edits {
		if edits[i].Pointer == pointer {
			edits[i].Value = value
			return
		}
	}
	b.edits[path] = append(edits, surface.Edit{Pointer: pointer, Value: value})
}

func (m *uiModel) surfaceRoot() (*os.Root, error) {
	b := &m.surfaces
	if b.root == nil && b.rootErr == nil {
		if m.opt.Workspace == "" {
			b.rootErr = errors.New("no workspace")
		} else {
			b.root, b.rootErr = os.OpenRoot(m.opt.Workspace)
		}
	}
	return b.root, b.rootErr
}

func (m *uiModel) surfaceEntry(id string) *cockpit.Entry {
	if m.tr == nil {
		return nil
	}
	return m.tr.Entry(id)
}

// surfaceCallOf is the surface a finished call of the UI tool showed.
func (m *uiModel) surfaceCallOf(e *cockpit.Entry) (surface.Call, bool) {
	if e == nil || e.Kind != cockpit.KindTool || e.Tool == nil || !surface.IsTool(e.Tool.Name) || !e.Tool.Terminal() {
		return surface.Call{}, false
	}
	b := &m.surfaces
	b.init()
	if c, ok := b.calls[e.ID]; ok && c.rev == e.Rev {
		return c.call, c.ok
	}
	call, ok := surface.ParseCall(e.Tool.Output)
	b.calls[e.ID] = surfaceCallInfo{rev: e.Rev, call: call, ok: ok}
	return call, ok
}

// surfaceVersionOf reads a version of a surface, once: it does not change.
func (m *uiModel) surfaceVersionOf(p string) surfaceVersion {
	b := &m.surfaces
	b.init()
	if v, ok := b.versions[p]; ok {
		return v
	}
	var r surfaceVersion
	root, err := m.surfaceRoot()
	if err == nil {
		r.v, err = surface.ReadVersion(root, p)
	}
	r.err = err
	if r.v != nil {
		r.comps = surface.Components(r.v)
	}
	b.versions[p] = r
	return r
}

// surfaceScan finds the newest call of each surface, and the entry of each
// version.
func (m *uiModel) surfaceScan() (heads, paths map[string]string) {
	heads, paths = map[string]string{}, map[string]string{}
	if m.tr == nil {
		return heads, paths
	}
	for _, e := range m.tr.Entries {
		call, ok := m.surfaceCallOf(e)
		if !ok {
			continue
		}
		paths[call.Path] = e.ID
		if call.Deleted {
			delete(heads, call.Surface)
			continue
		}
		heads[call.Surface] = e.ID
	}
	return heads, paths
}

// surfaceHead says whether an entry is its surface's newest call: the live
// one.
func (m *uiModel) surfaceHead(e *cockpit.Entry) bool {
	call, ok := m.surfaceCallOf(e)
	if !ok || call.Deleted {
		return false
	}
	b := &m.surfaces
	if b.heads == nil {
		b.heads, b.paths = m.surfaceScan()
	}
	return b.heads[call.Surface] == e.ID
}

// surfaceDoc is a version's data model: with its files as they are now
// when it is live, as they were when it was made when not.
func (m *uiModel) surfaceDoc(call surface.Call, v *surface.Object, head bool) (doc, sources, infos *surface.Object) {
	if l := m.surfaces.live[call.Path]; head && l != nil {
		sources, infos = l.sources, l.infos
	} else {
		sources, infos = surface.Snapshot(v)
	}
	return surface.Model(v, sources, infos, m.surfaces.edits[call.Path]), sources, infos
}

// surfaceState is a surface's call as it stands.
type surfaceState struct {
	call    surface.Call
	version *surface.Object
	comps   map[string]*surface.Object
	doc     *surface.Object
	head    bool
}

func (m *uiModel) surfaceStateOf(e *cockpit.Entry) (surfaceState, bool) {
	call, ok := m.surfaceCallOf(e)
	if !ok || call.Deleted {
		return surfaceState{}, false
	}
	r := m.surfaceVersionOf(call.Path)
	if r.v == nil {
		return surfaceState{}, false
	}
	head := m.surfaceHead(e)
	doc, _, _ := m.surfaceDoc(call, r.v, head)
	return surfaceState{call: call, version: r.v, comps: r.comps, doc: doc, head: head}, true
}

// surfaceRedraw drops an entry's drawing, for the next render to draw it
// anew: what a surface shows changes without its entry changing.
func (m *uiModel) surfaceRedraw(id string) {
	delete(m.cache, id)
	delete(m.reflow.next, id)
}

// ---------------------------------------------------------------- the loop

// surfacesUpdate follows the surfaces after every message: what their files
// said, which calls are the newest, and when to look at the files again.
func (m *uiModel) surfacesUpdate(msg tea.Msg) tea.Cmd {
	b := &m.surfaces
	b.init()
	tick := false
	changed := map[string]bool{} // version paths
	switch msg := msg.(type) {
	case surfaceTickMsg:
		b.ticking, tick = false, true
	case surfaceReadMsg:
		b.reading = false
		for p, l := range msg.live {
			b.live[p] = l
			changed[p] = true
		}
		for key, c := range msg.codes {
			b.codes[key] = c
			p, _, _ := strings.Cut(key, "#")
			changed[p] = true
		}
	case tea.MouseMsg:
		// A click elsewhere takes the keys back from the surface.
		if msg.Action == tea.MouseActionRelease && b.focus != nil && !b.clicked {
			m.blurSurface()
		}
		if msg.Action == tea.MouseActionRelease {
			b.clicked = false
		}
	}
	redraw := map[string]bool{}
	heads, paths := m.surfaceScan()
	if !maps.Equal(heads, b.heads) {
		// The call that was live is a snapshot now, and the new one live.
		for _, id := range b.heads {
			redraw[id] = true
		}
		for _, id := range heads {
			redraw[id] = true
		}
	}
	b.heads, b.paths = heads, paths
	for p := range changed {
		if id, ok := paths[p]; ok {
			redraw[id] = true
		}
	}
	if f := b.focus; f != nil {
		if e := m.surfaceEntry(f.entry); e == nil || !m.surfaceHead(e) {
			b.focus = nil
			m.input.Focus()
			redraw[f.entry] = true
		}
	}
	for id := range redraw {
		m.surfaceRedraw(id)
	}
	if len(redraw) != 0 {
		m.refresh()
	}
	return m.surfaceSchedule(tick)
}

// surfaceSchedule reads the live surfaces' files now when it is time, or
// waits to; with no live surface reading files, nothing.
func (m *uiModel) surfaceSchedule(tick bool) tea.Cmd {
	b := &m.surfaces
	if b.reading || m.tr == nil {
		return nil
	}
	root, err := m.surfaceRoot()
	if err != nil {
		return nil
	}
	now := tick || len(b.wanted) != 0
	var jobs []surfaceJob
	asked := map[string]bool{}
	for _, id := range b.heads {
		call, ok := m.surfaceCallOf(m.surfaceEntry(id))
		if !ok || call.Deleted {
			continue
		}
		r := m.surfaceVersionOf(call.Path)
		if r.v == nil {
			continue
		}
		job := surfaceJob{path: call.Path, version: r.v}
		if len(surface.SourcesOf(r.v)) != 0 {
			job.sources = true
			if l := b.live[call.Path]; l != nil {
				job.stamp = l.stamp
			} else {
				now = true // not read yet
			}
		}
		for cid, c := range r.comps {
			if surface.Str(c, "component") != "Code" || has(c, "text") {
				continue
			}
			file := surface.Clean(surface.Str(c, "file"))
			if file == "" {
				continue
			}
			key := call.Path + "#" + cid
			code := surfaceCodeJob{key: key, file: file, lines: codeSpec(c)}
			if have := b.codes[key]; have != nil && have.file == file {
				code.stamp = have.stamp
			}
			job.codes = append(job.codes, code)
			asked[key] = true
		}
		if job.sources || len(job.codes) != 0 {
			jobs = append(jobs, job)
		}
	}
	for key, code := range b.wanted {
		if !asked[key] {
			jobs = append(jobs, surfaceJob{codes: []surfaceCodeJob{code}})
		}
	}
	if len(jobs) == 0 {
		return nil
	}
	if now {
		b.reading = true
		clear(b.wanted)
		reader := b.reader
		return func() tea.Msg { return surfaceRead(root, reader, jobs) }
	}
	if !b.ticking {
		b.ticking = true
		return tea.Tick(surfacePoll, func(time.Time) tea.Msg { return surfaceTickMsg{} })
	}
	return nil
}

// surfaceRead looks at the files of the jobs, and reads those that changed.
func surfaceRead(root *os.Root, reader *surface.Reader, jobs []surfaceJob) tea.Msg {
	msg := surfaceReadMsg{live: map[string]*surfaceLive{}, codes: map[string]*surfaceCode{}}
	for _, job := range jobs {
		if job.sources {
			if stamp := sourcesStamp(root, job.version); stamp != job.stamp {
				sources, infos := surface.Live(reader, root, job.path, job.version)
				msg.live[job.path] = &surfaceLive{sources: sources, infos: infos, stamp: stamp}
			}
		}
		for _, job := range job.codes {
			stamp := surface.Stamp(root, job.file)
			if stamp == job.stamp {
				continue
			}
			text, from, cut, err := surface.ReadCode(root, job.file, job.lines)
			code := &surfaceCode{file: job.file, text: text, from: from, cut: cut, err: err, stamp: stamp}
			if err == nil {
				code.runs = highlight.Highlight(job.file, text, time.Now().Add(100*time.Millisecond)).Runs
			}
			msg.codes[job.key] = code
		}
	}
	return msg
}

// sourcesStamp names the state of a version's files: their sizes and times.
func sourcesStamp(root *os.Root, v *surface.Object) string {
	var b strings.Builder
	for _, s := range surface.SourcesOf(v) {
		b.WriteString(s.Name + "=" + surface.Stamp(root, s.File) + "\n")
	}
	return b.String()
}

// ---------------------------------------------------------------- the block

// surfaceTool draws the block of a call of the UI tool that showed a
// surface; for any other call, it says it is not one.
func (m *uiModel) surfaceTool(e *cockpit.Entry, width int, open bool) ([]string, bool) {
	call, ok := m.surfaceCallOf(e)
	if !ok {
		return nil, false
	}
	b := &m.surfaces
	delete(b.controls, e.ID)
	st := m.styles
	head := m.surfaceHead(e)
	var r surfaceVersion
	if !call.Deleted {
		r = m.surfaceVersionOf(call.Path)
	}
	title := orDefault(call.Title, call.Surface)
	var doc, infos *surface.Object
	if r.v != nil {
		title = orDefault(oneLine(surface.Str(r.v, "title")), title)
		doc, _, infos = m.surfaceDoc(call, r.v, head)
	}

	glyph := st.ghost.Render("◇")
	if head {
		glyph = st.accent.Render("◇")
	}
	stamp := st.ghost.Render(fmt.Sprintf("%*s", gutter-3, localTime(e.At)))
	left := stamp + " " + glyph + " " + st.label.Render("UI") + "  " + st.text.Render(oneLine(title)) +
		st.faint.Render(fmt.Sprintf(" v%d", call.Version))
	if r.v != nil {
		if glance := oneLine(surface.Fill(doc, r.comps, surface.Str(r.v, "glance"))); glance != "" {
			left += st.muted.Render(" · " + glance)
		}
	}
	var right string
	switch {
	case call.Deleted:
		right = st.faint.Render("removed")
	case r.v == nil:
		right = st.err.Render("unreadable")
	case head:
		right = st.ok.Render("● live")
		if t, ok := newest(infos); ok {
			right += st.ghost.Render(" " + localTime(t))
		}
	default:
		right = st.ghost.Render("snapshot")
	}
	chevron := "▸"
	if open {
		chevron = "▾"
	}
	lines := []string{fitRight(left, right+"  "+st.ghost.Render(chevron), width)}
	if !open {
		return lines, true
	}

	body := max(10, width-gutter)
	if r.v == nil {
		// What the agent got as text stands in for what cannot be drawn.
		var text []string
		if call.Deleted {
			text = wrapText("The agent removed this surface.", st.muted, body, "", "")
		} else {
			text = wrapText(fmt.Sprintf("%s cannot be read: %v", call.Path, r.err), st.err, body, "", "")
		}
		if t := strings.TrimSpace(cockpit.Clean(call.Text)); t != "" && !call.Deleted {
			text = append(text, "")
			text = append(text, wrapText(t, st.muted, body, "", "")...)
		}
		return append(lines, m.card(text, st.rule, "", nil, width)...), true
	}
	d := &surfaceDraw{m: m, st: st, path: call.Path, version: r.v, comps: r.comps, doc: doc, live: head,
		width: body, cursor: -1, joints: map[int]string{}, stack: map[string]bool{}}
	focused := b.focus != nil && b.focus.entry == e.ID
	if focused {
		d.focus, d.cursor = b.focus.control, b.focus.cursor
	}
	d.node("root", 0)
	switch {
	case r.comps["root"] == nil:
		d.none(`Nothing to show: the surface has no component "root".`)
	case len(d.lines) == 0:
		d.none("Nothing to show yet.")
	}
	d.footer(call, infos, head && !focused && len(d.controls) != 0)
	b.controls[e.ID] = d.controls
	return append(lines, m.card(d.lines, st.rule, d.top, d.joints, width)...), true
}

// newest is when the newest of a surface's files changed.
func newest(infos *surface.Object) (time.Time, bool) {
	var best time.Time
	for _, name := range infos.Keys() {
		info := surface.ObjectOf(prop(infos, name))
		if t, err := time.Parse(time.RFC3339, surface.Str(info, "mtime")); err == nil && t.After(best) {
			best = t
		}
	}
	return best, !best.IsZero()
}

// surfaceDraw draws a surface's components into the body of its card.
type surfaceDraw struct {
	m       *uiModel
	st      styles
	path    string
	version *surface.Object
	comps   map[string]*surface.Object
	doc     *surface.Object
	live    bool // the newest call: its inputs and buttons work

	width, indent int
	focus         string // the control focused, if the focus is here
	cursor        int

	top      string // the card's title: the title of a Section it starts with
	lines    []string
	joints   map[int]string // rows that are edges between Sections
	controls []surfaceControl
	stack    map[string]bool // the components being drawn, against cycles
}

func (d *surfaceDraw) room() int { return max(4, d.width-d.indent) }

func (d *surfaceDraw) add(lines ...string) {
	pad := strings.Repeat(" ", d.indent)
	for _, line := range lines {
		if line != "" {
			line = pad + line
		}
		d.lines = append(d.lines, line)
	}
}

// blank puts a row of air after what was drawn, unless there is one.
func (d *surfaceDraw) blank() {
	if n := len(d.lines); n != 0 && d.lines[n-1] != "" {
		d.lines = append(d.lines, "")
	}
}

// trim drops the rows of air at the end, up to an edge.
func (d *surfaceDraw) trim() {
	for n := len(d.lines); n != 0 && d.lines[n-1] == ""; n = len(d.lines) {
		if _, joint := d.joints[n-1]; joint {
			return
		}
		d.lines = d.lines[:n-1]
	}
}

// none says there is nothing, or how much more there is.
func (d *surfaceDraw) none(text string) {
	d.add(wrapText(text, d.st.ghost, d.room(), "", "")...)
}

func (d *surfaceDraw) focused(id, text string) string {
	if id != "" && id == d.focus {
		return d.st.selection.Render(ansi.Strip(text))
	}
	return text
}

// value is a prop's value, or nil when its binding finds none.
func (d *surfaceDraw) value(v any) any {
	x, ok := surface.Resolve(d.doc, v)
	if !ok {
		return nil
	}
	return x
}

// fill is a text prop as it shows: "" when there is none.
func (d *surfaceDraw) fill(v any) string {
	if v == nil {
		return ""
	}
	return surface.TextOf(d.doc, d.comps, v)
}

// text is a component's text prop on one line.
func (d *surfaceDraw) text(c *surface.Object, key string) string {
	return oneLine(d.fill(prop(c, key)))
}

// bound says whether an input works: in the live call, and bound to a
// pointer outside the sources.
func (d *surfaceDraw) bound(c *surface.Object) bool {
	ptr, ok := surface.Binding(prop(c, "value"))
	return d.live && ok && surface.Inside(ptr)
}

// inlinePiece is a component drawn within a line, and the control it is.
type inlinePiece struct {
	text, id, kind string
	index          int
}

// flow lays pieces out in lines, sep columns apart, and notes where the
// controls among them went.
func (d *surfaceDraw) flow(pieces []inlinePiece, sep int) {
	room := d.room()
	line, used := "", 0
	var pending []surfaceControl
	flush := func() {
		if used == 0 {
			return
		}
		row := len(d.lines)
		for _, c := range pending {
			c.row = row
			d.controls = append(d.controls, c)
		}
		d.add(line)
		line, used, pending = "", 0, nil
	}
	for _, p := range pieces {
		if p.text == "" {
			continue
		}
		text := fit(p.text, room)
		w := ansi.StringWidth(text)
		if used != 0 && used+sep+w > room {
			flush()
		}
		if used != 0 {
			line += strings.Repeat(" ", sep)
			used += sep
		}
		if p.id != "" {
			pending = append(pending, surfaceControl{id: p.id, kind: p.kind, index: p.index, from: d.indent + used, to: d.indent + used + w})
		}
		line += text
		used += w
	}
	flush()
}

// inlineKinds are the components that draw within a line: a Row of them
// is one line, or as many as it takes.
var inlineKinds = map[string]bool{"Metric": true, "Badge": true, "Button": true, "Toggle": true, "FileLink": true, "Select": true, "Slider": true}

// controlKinds are the components the keys can go to.
var controlKinds = map[string]bool{"Button": true, "TextField": true, "Toggle": true, "Select": true, "Slider": true,
	"Tabs": true, "Disclosure": true, "FileLink": true}

func (d *surfaceDraw) node(id string, depth int) {
	c := d.comps[id]
	if c == nil || d.stack[id] || depth > 32 {
		return
	}
	d.stack[id] = true
	defer delete(d.stack, id)
	switch kind := surface.Str(c, "component"); kind {
	case "Column":
		d.children(c, depth)
	case "Row", "Grid":
		d.drawRow(c, depth)
	case "Section":
		d.drawSection(c, depth)
	case "Tabs":
		d.drawTabs(c, id, depth)
	case "Disclosure":
		d.drawDisclosure(c, id, depth)
	case "Text":
		d.drawText(c)
	case "Markdown":
		d.drawMarkdown(c)
	case "Metric":
		d.drawStrip([]string{id})
	case "Badge", "Button", "Toggle", "FileLink", "Select", "Slider":
		d.flow([]inlinePiece{d.inline(id, c)}, 0)
	case "Progress":
		d.drawProgress(c)
	case "Table":
		d.drawTable(c)
	case "List":
		d.drawList(c)
	case "Chart":
		d.drawChart(c)
	case "Image":
		d.drawImage(c)
	case "Code":
		d.drawCode(c, id)
	case "Diff":
		d.drawDiff(c, id)
	case "TextField":
		d.drawField(c, id)
	case "Html":
		d.drawHtml(c)
	default:
		d.drawUnknown(c)
	}
}

func (d *surfaceDraw) children(c *surface.Object, depth int) {
	gap := surface.Str(c, "gap")
	spaced := gap == "large" || depth == 0 && gap != "none" && gap != "small"
	for i, id := range childIDs(c) {
		if i > 0 && spaced {
			d.blank()
		}
		d.node(id, depth+1)
	}
}

// drawRow draws a row of Metrics as a strip, and a row of what draws within
// a line as a line; another row is its children one under another.
func (d *surfaceDraw) drawRow(c *surface.Object, depth int) {
	ids := childIDs(c)
	metrics, inline := len(ids) != 0, len(ids) != 0
	for _, id := range ids {
		kind := surface.Str(d.comps[id], "component")
		metrics = metrics && kind == "Metric"
		inline = inline && inlineKinds[kind]
	}
	switch {
	case metrics:
		d.drawStrip(ids)
	case inline:
		var pieces []inlinePiece
		for _, id := range ids {
			pieces = append(pieces, d.inline(id, d.comps[id]))
		}
		d.flow(pieces, 3)
	default:
		d.children(c, depth)
	}
}

func (d *surfaceDraw) drawSection(c *surface.Object, depth int) {
	st := d.st
	title := strings.ToUpper(d.text(c, "title"))
	switch {
	case title == "":
	case len(d.lines) == 0 && d.top == "" && d.indent == 0:
		d.top = title // the card's own edge
	case depth <= 1 && d.indent == 0:
		d.trim()
		d.joints[len(d.lines)] = title
		d.lines = append(d.lines, "")
	default:
		d.blank()
		d.add(st.rule.Render("── ") + st.label.Render(title) + st.rule.Render(" ──"))
	}
	d.children(c, depth)
}

func (d *surfaceDraw) drawTabs(c *surface.Object, id string, depth int) {
	st := d.st
	tabs := surface.ListOf(prop(c, "tabs"))
	if len(tabs) == 0 {
		return
	}
	at := clamp(d.m.surfaces.tabs[d.path+"#"+id], 0, len(tabs)-1)
	var pieces []inlinePiece
	for i, tab := range tabs {
		label := orDefault(d.text(surface.ObjectOf(tab), "label"), fmt.Sprintf("Tab %d", i+1))
		text := st.muted.Render(" " + label + " ")
		if i == at {
			text = st.accentLabel.Render("[" + label + "]")
			if d.focus == id {
				text = st.selection.Render("[" + label + "]")
			}
		}
		pieces = append(pieces, inlinePiece{text: text, id: id, kind: "Tabs", index: i})
	}
	d.flow(pieces, 1)
	d.node(surface.Str(surface.ObjectOf(tabs[at]), "child"), depth+1)
}

func (d *surfaceDraw) drawDisclosure(c *surface.Object, id string, depth int) {
	st := d.st
	open := surface.Bool(c, "open")
	if v, ok := d.m.surfaces.folds[d.path+"#"+id]; ok {
		open = v
	}
	chevron := "▸ "
	if open {
		chevron = "▾ "
	}
	text := st.ghost.Render(chevron) + st.text.Render(orDefault(d.text(c, "label"), "Details"))
	d.flow([]inlinePiece{{text: d.focused(id, text), id: id, kind: "Disclosure"}}, 0)
	if open {
		d.indent += 2
		d.children(c, depth)
		d.indent -= 2
	}
}

func (d *surfaceDraw) drawText(c *surface.Object) {
	st := d.st
	text := strings.TrimRight(cockpit.Clean(d.fill(prop(c, "text"))), "\n")
	if strings.TrimSpace(text) == "" {
		return
	}
	style := st.text
	switch surface.Str(c, "style") {
	case "heading":
		style = st.bold
	case "caption":
		style = st.faint
	case "mono":
		style = st.codeBlock
	}
	d.add(wrapText(text, style, d.room(), "", "")...)
}

func (d *surfaceDraw) drawMarkdown(c *surface.Object) {
	text := strings.TrimSpace(cockpit.Clean(d.fill(prop(c, "text"))))
	if text == "" {
		return
	}
	lines := renderMarkdown(d.st, text, d.room(), d.st.text)
	more := len(lines) - surfaceRows
	if more > 0 {
		lines = lines[:surfaceRows]
	}
	d.add(lines...)
	if more > 0 {
		d.none(plural(more, "more line", "more lines"))
	}
}

// metricValue is a Metric's number, unit and change.
func (d *surfaceDraw) metricValue(c *surface.Object) string {
	st := d.st
	p := surface.Metric(d.doc, c)
	if p.Missing {
		return st.ghost.Render("—")
	}
	out := st.bold.Render(p.Text)
	if p.Unit != "" {
		out += st.faint.Render(" " + p.Unit)
	}
	if p.Change != nil && p.Change.Text != "" {
		style := st.faint
		switch p.Tone {
		case "good":
			style = st.ok
		case "bad":
			style = st.err
		}
		out += " " + style.Render(p.Change.Text)
	}
	return out
}

// drawStrip draws Metrics side by side: their values, and under each its
// label, as the web cockpit's strip.
func (d *surfaceDraw) drawStrip(ids []string) {
	type cell struct {
		value, label string
		named        bool
		w            int
	}
	room := d.room()
	var cells []cell
	for _, id := range ids {
		c := d.comps[id]
		if c == nil {
			continue
		}
		value := fit(d.metricValue(c), room)
		label := fit(strings.ToUpper(d.text(c, "label")), room)
		cells = append(cells, cell{value: value, label: d.st.label.Render(label), named: label != "",
			w: max(ansi.StringWidth(value), ansi.StringWidth(label))})
	}
	var row []cell
	used := 0
	emit := func() {
		if len(row) == 0 {
			return
		}
		var values, labels []string
		named := false
		for i, c := range row {
			value, label := c.value, c.label
			if i < len(row)-1 {
				value, label = padTo(value, c.w), padTo(label, c.w)
			}
			values, labels = append(values, value), append(labels, label)
			named = named || c.named
		}
		d.add(strings.Join(values, "   "))
		if named {
			d.add(strings.Join(labels, "   "))
		}
		row, used = nil, 0
	}
	for _, c := range cells {
		if used != 0 && used+3+c.w > room {
			emit()
		}
		if used != 0 {
			used += 3
		}
		row = append(row, c)
		used += c.w
	}
	emit()
}

// inline draws a component that draws within a line.
func (d *surfaceDraw) inline(id string, c *surface.Object) inlinePiece {
	st := d.st
	key := d.path + "#" + id
	switch surface.Str(c, "component") {
	case "Metric":
		text := d.metricValue(c)
		if label := d.text(c, "label"); label != "" {
			text = st.label.Render(strings.ToUpper(label)) + " " + text
		}
		return inlinePiece{text: text}
	case "Badge":
		text := d.text(c, "text")
		if text == "" {
			return inlinePiece{}
		}
		return inlinePiece{text: toneStyle(st, surface.Str(c, "tone")).Render(strings.ToUpper(text))}
	case "Button":
		used := surface.Bool(c, "once") && d.m.surfaces.used[key]
		off := !d.live || used || truthy(d.value(prop(c, "disabled")))
		label := orDefault(d.text(c, "label"), "Button")
		if used {
			label += " ✓"
		}
		style := st.chip
		switch surface.Str(c, "style") {
		case "primary":
			style = st.button
		case "danger":
			style = toneStyle(st, "err")
		}
		if off {
			return inlinePiece{text: st.buttonOff.Render(label)}
		}
		return inlinePiece{text: d.focused(id, style.Render(label)), id: id, kind: "Button"}
	case "Toggle":
		box := "[ ]"
		if d.value(prop(c, "value")) == true {
			box = "[x]"
		}
		label := d.text(c, "label")
		if !d.bound(c) {
			return inlinePiece{text: st.ghost.Render(strings.TrimSpace(box + " " + label))}
		}
		return inlinePiece{text: d.focused(id, st.accent.Render(box)+" "+st.text.Render(label)), id: id, kind: "Toggle"}
	case "Select":
		options := selectOptions(c)
		current, ok := surface.Resolve(d.doc, prop(c, "value"))
		shown := "—"
		if at := selectAt(options, current, ok); at >= 0 {
			shown = options[at].label
		} else if ok && current != nil {
			shown = valueText(current)
		}
		text := st.accent.Render("‹ ") + st.text.Render(oneLine(shown)) + st.accent.Render(" ›")
		if label := d.text(c, "label"); label != "" {
			text = st.muted.Render(label) + " " + text
		}
		if !d.bound(c) {
			return inlinePiece{text: st.ghost.Render(ansi.Strip(text))}
		}
		return inlinePiece{text: d.focused(id, text), id: id, kind: "Select"}
	case "Slider":
		const cells = 12
		lo, hi, _ := sliderRange(c)
		v, ok := surface.AsNumber(d.value(prop(c, "value")))
		var text string
		if ok {
			at := 0
			if hi > lo {
				at = int(math.Round((math.Max(lo, math.Min(hi, v)) - lo) / (hi - lo) * (cells - 1)))
			}
			text = st.accent.Render(strings.Repeat("─", at)+"●"+strings.Repeat("─", cells-1-at)) + " " + st.text.Render(surface.Num(v, -1))
		} else {
			text = st.ghost.Render(strings.Repeat("─", cells) + " —")
		}
		if label := d.text(c, "label"); label != "" {
			text = st.muted.Render(label) + " " + text
		}
		if !d.bound(c) {
			return inlinePiece{text: st.ghost.Render(ansi.Strip(text))}
		}
		return inlinePiece{text: d.focused(id, text), id: id, kind: "Slider"}
	case "FileLink":
		file, line := d.fileOf(c)
		label := orDefault(d.text(c, "label"), fileLine(file, line))
		if file == "" {
			return inlinePiece{text: st.ghost.Render("↗ " + orDefault(label, "a file"))}
		}
		return inlinePiece{text: d.focused(id, st.accent.Render("↗ ")+st.link.Render(label)), id: id, kind: "FileLink"}
	}
	return inlinePiece{}
}

// fileOf is the file and line a FileLink names.
func (d *surfaceDraw) fileOf(c *surface.Object) (string, int) {
	return surfaceFileOf(d.doc, d.comps, c)
}

func surfaceFileOf(doc *surface.Object, comps map[string]*surface.Object, c *surface.Object) (string, int) {
	file := ""
	if v := prop(c, "file"); v != nil {
		file = surface.Clean(surface.TextOf(doc, comps, v))
	}
	line := 0
	if v, ok := surface.Resolve(doc, prop(c, "line")); ok {
		if n, ok := surface.AsNumber(v); ok && n >= 1 {
			line = int(n)
		}
	}
	return file, line
}

func (d *surfaceDraw) drawProgress(c *surface.Object) {
	st := d.st
	label := d.text(c, "label")
	v, ok := surface.AsNumber(d.value(prop(c, "value")))
	top := 1.0
	if raw := prop(c, "max"); raw != nil {
		if n, ok := surface.AsNumber(d.value(raw)); ok && n != 0 {
			top = n
		}
	}
	room := d.room()
	lead := ""
	if label != "" {
		lead = st.muted.Render(fit(label, room/2)) + "  "
	}
	bar := max(6, min(30, room-ansi.StringWidth(lead)-5))
	filled, shown := 0, "—"
	if ok {
		f := math.Max(0, math.Min(1, v/top))
		filled, shown = int(math.Round(f*float64(bar))), strconv.Itoa(int(math.Round(f*100)))+"%"
	}
	d.add(lead + st.accent.Render(strings.Repeat("█", filled)) + st.ghost.Render(strings.Repeat("░", bar-filled)) + " " + st.text.Render(shown))
}

// rowsOf is the list a Table or a List shows, or what to say instead.
func (d *surfaceDraw) rowsOf(c *surface.Object, key, empty string) ([]any, bool) {
	v, ok := surface.Resolve(d.doc, prop(c, key))
	list, isList := v.([]any)
	switch {
	case !ok || v == nil || isList && len(list) == 0:
		d.none(empty)
		return nil, false
	case !isList:
		d.none("Not a list: " + fit(oneLine(surface.Shown(v, true)), 60))
		return nil, false
	}
	if surface.Bool(c, "reverse") {
		list = slices.Clone(list)
		slices.Reverse(list)
	}
	return list, true
}

// limitOf is how many rows a Table or a List shows.
func limitOf(c *surface.Object) int {
	if n, ok := surface.Int(c, "limit"); ok && n > 0 {
		return n
	}
	return 10
}

type tableColumn struct{ key, label, align string }

// tableColumns are the columns a Table names, or else the fields of its
// first row of fields.
func tableColumns(c *surface.Object, rows []any) []tableColumn {
	var cols []tableColumn
	for _, v := range surface.ListOf(prop(c, "columns")) {
		switch v := v.(type) {
		case string:
			cols = append(cols, tableColumn{key: v, label: v})
		case *surface.Object:
			if key := surface.Str(v, "key"); key != "" {
				cols = append(cols, tableColumn{key: key, label: orDefault(surface.Str(v, "label"), key), align: surface.Str(v, "align")})
			}
		}
	}
	if len(cols) != 0 {
		return cols
	}
	for _, row := range rows {
		if o := surface.ObjectOf(row); o != nil {
			for _, key := range o.Keys() {
				if len(cols) < 8 {
					cols = append(cols, tableColumn{key: key, label: key})
				}
			}
			return cols
		}
	}
	return []tableColumn{{key: "value", label: "value"}}
}

func (d *surfaceDraw) drawTable(c *surface.Object) {
	st := d.st
	rows, ok := d.rowsOf(c, "rows", "No rows yet")
	if !ok {
		return
	}
	shown := rows[:min(limitOf(c), len(rows))]
	cols := tableColumns(c, rows)
	type cellText struct {
		text    string
		missing bool
		number  bool
	}
	cells := make([][]cellText, len(shown))
	for i, row := range shown {
		o := surface.ObjectOf(row)
		for j, col := range cols {
			var v any
			has := false
			if o != nil {
				v, has = o.Get(col.key)
			} else if j == 0 {
				v, has = row, true
			}
			var cell cellText
			switch x := v.(type) {
			case nil:
				cell = cellText{text: "—", missing: true}
			case float64:
				cell = cellText{text: surface.Num(x, -1), number: true}
			case string:
				cell = cellText{text: oneLine(x)}
			default:
				cell = cellText{text: oneLine(surface.Shown(v, has))}
			}
			if !has {
				cell = cellText{text: "—", missing: true}
			}
			cells[i] = append(cells[i], cell)
		}
	}
	widths := make([]int, len(cols))
	aligns := make([]string, len(cols))
	for j, col := range cols {
		widths[j] = ansi.StringWidth(col.label)
		numbers, any := true, false
		for i := range cells {
			cell := cells[i][j]
			widths[j] = max(widths[j], ansi.StringWidth(cell.text))
			if !cell.missing {
				any = true
				numbers = numbers && cell.number
			}
		}
		widths[j] = min(widths[j], 40)
		aligns[j] = col.align
		if aligns[j] == "" && any && numbers {
			aligns[j] = "right"
		}
	}
	// What does not fit is shortened, the widest first; columns that do not
	// fit even so are left out.
	room := d.room()
	n := len(cols)
	for n > 1 && 3*n+2*(n-1) > room {
		n--
	}
	widths = widths[:n]
	total := func() int {
		t := 2 * (len(widths) - 1)
		for _, w := range widths {
			t += w
		}
		return t
	}
	for total() > room {
		widest := 0
		for j, w := range widths {
			if w > widths[widest] {
				widest = j
			}
		}
		if widths[widest] <= 3 {
			break
		}
		widths[widest]--
	}
	var head []string
	for j := range widths {
		head = append(head, st.label.Render(alignTo(fit(strings.ToUpper(cols[j].label), widths[j]), widths[j], aligns[j])))
	}
	d.add(strings.Join(head, "  "))
	d.add(st.rule.Render(strings.Repeat("─", min(total(), room))))
	for i := range cells {
		var line []string
		for j := range widths {
			cell := cells[i][j]
			text := alignTo(fit(cell.text, widths[j]), widths[j], aligns[j])
			if cell.missing {
				line = append(line, st.ghost.Render(text))
			} else {
				line = append(line, st.text.Render(text))
			}
		}
		d.add(strings.TrimRight(strings.Join(line, "  "), " "))
	}
	if more := len(rows) - len(shown); more > 0 {
		where := " after these"
		if surface.Bool(c, "reverse") {
			where = " before these"
		}
		d.none(plural(more, "more row", "more rows") + where)
	}
	if left := len(cols) - n; left > 0 {
		var names []string
		for _, col := range cols[n:] {
			names = append(names, col.label)
		}
		d.none(plural(left, "column", "columns") + " not shown here: " + strings.Join(names, ", "))
	}
}

func (d *surfaceDraw) drawList(c *surface.Object) {
	st := d.st
	items, ok := d.rowsOf(c, "items", "Nothing yet")
	if !ok {
		return
	}
	shown := items[:min(limitOf(c), len(items))]
	template, isTemplate := prop(c, "text").(string)
	for _, item := range shown {
		text := surface.Shown(item, true)
		if isTemplate && surface.ObjectOf(item) != nil {
			text = surface.FillItem(item, template)
		}
		d.add(wrapText(oneLine(text), st.text, d.room(), st.ghost.Render("· "), "  ")...)
	}
	if more := len(items) - len(shown); more > 0 {
		d.none(surface.Num(float64(more), -1) + " more")
	}
}

// chartBlocks are the eighths of a cell a chart's columns fill.
var chartBlocks = []rune(" ▁▂▃▄▅▆▇█")

// seriesColors are the colours of a chart's series, as the web cockpit's.
var seriesColors = []lipgloss.TerminalColor{colorAccent, colorSynType, colorSynBuiltin, colorSynConst, colorSynFunc, colorOK}

func (d *surfaceDraw) drawChart(c *surface.Object) {
	st := d.st
	alt := d.text(c, "alt")
	about := func(s string) string {
		if alt != "" {
			return s + " · " + alt
		}
		return s
	}
	data, ok := surface.Resolve(d.doc, prop(c, "data"))
	points, isList := data.([]any)
	switch {
	case !ok || data == nil || isList && len(points) == 0:
		d.none(about("No points yet"))
		return
	case !isList:
		d.none(about("No points"))
		return
	}
	limit := 2000
	if n, ok := surface.Int(c, "limit"); ok && n > 0 {
		limit = min(n, limit)
	}
	points = points[max(0, len(points)-limit):]
	var fields []string
	switch y := prop(c, "y").(type) {
	case string:
		fields = []string{y}
	case []any:
		for _, f := range y {
			if s, ok := f.(string); ok && len(fields) < 6 {
				fields = append(fields, s)
			}
		}
	}
	type series struct {
		name string
		vals []float64
	}
	var all []series
	for _, field := range fields {
		s := series{name: field, vals: make([]float64, len(points))}
		count := 0
		for i, point := range points {
			s.vals[i] = math.NaN()
			v, _ := surface.ObjectOf(point).Get(field)
			if n, ok := surface.AsNumber(v); ok && !math.IsInf(n, 0) {
				s.vals[i] = n
				count++
			}
		}
		if count != 0 {
			all = append(all, s)
		}
	}
	if len(all) == 0 {
		d.none(about("No numbers in " + orDefault(strings.Join(fields, ", "), "the points")))
		return
	}
	digits, ok := surface.Int(c, "digits")
	if !ok {
		digits = -1
	}
	unit := d.text(c, "unit")
	number := func(v float64) string {
		if unit != "" {
			return surface.Num(v, digits) + " " + unit
		}
		return surface.Num(v, digits)
	}
	xOf := func(i int) string {
		field := surface.Str(c, "x")
		if field == "" {
			return strconv.Itoa(i + 1)
		}
		v, ok := surface.ObjectOf(points[i]).Get(field)
		return oneLine(surface.Shown(v, ok))
	}
	first, last := xOf(0), xOf(len(points)-1)
	room := d.room()

	if len(all) == 1 {
		s := all[0]
		lo, hi, _ := rangeOf(s.vals)
		top, bottom := " "+number(hi), " "+number(lo)
		w := max(4, room-max(ansi.StringWidth(top), ansi.StringWidth(bottom)))
		cols := chartColumns(s.vals, w)
		rows := make([]strings.Builder, 3)
		for _, v := range cols {
			level := 0
			switch {
			case math.IsNaN(v):
			case hi > lo:
				level = 1 + int(math.Round((v-lo)/(hi-lo)*23))
			default:
				level = 12
			}
			for r := range rows {
				rows[r].WriteRune(chartBlocks[clamp(level-(2-r)*8, 0, 8)])
			}
		}
		// The scale's ends go next to the columns, however few they are.
		for r := range rows {
			line := st.accent.Render(rows[r].String())
			switch r {
			case 0:
				line += st.faint.Render(top)
			case 2:
				line += st.faint.Render(bottom)
			}
			d.add(line)
		}
		d.add(st.ghost.Render(spread(first, last, max(len(cols), ansi.StringWidth(first)+ansi.StringWidth(last)+2))))
		// The legend, as the web cockpit's: what the chart shows, in words,
		// and the series' last value.
		v, _ := lastOf(s.vals)
		legend := st.accent.Render("■") + " " + st.muted.Render(s.name) + " " + st.text.Render(number(v))
		if alt != "" {
			legend = st.muted.Render(alt) + "   " + legend
		}
		d.add(fit(legend, room))
		return
	}

	// Series apart, each on its own scale.
	nameW, lastW := 0, 0
	lasts := make([]string, len(all))
	for i, s := range all {
		nameW = max(nameW, ansi.StringWidth(s.name))
		v, _ := lastOf(s.vals)
		lasts[i] = number(v)
		lastW = max(lastW, ansi.StringWidth(lasts[i]))
	}
	nameW = min(nameW, 16)
	w := max(4, room-nameW-lastW-2)
	// The points are as many in every series: the columns are as wide.
	w = len(chartColumns(all[0].vals, w))
	for i, s := range all {
		lo, hi, _ := rangeOf(s.vals)
		var line strings.Builder
		cols := chartColumns(s.vals, w)
		for _, v := range cols {
			switch {
			case math.IsNaN(v):
				line.WriteRune(' ')
			case hi > lo:
				line.WriteRune(chartBlocks[1+int(math.Round((v-lo)/(hi-lo)*7))])
			default:
				line.WriteRune(chartBlocks[4])
			}
		}
		style := st.text
		if !st.noColor {
			style = lipgloss.NewStyle().Foreground(seriesColors[i%len(seriesColors)])
		}
		d.add(st.muted.Render(padTo(fit(s.name, nameW), nameW)) + " " + style.Render(padTo(line.String(), w)) + " " + st.text.Render(lasts[i]))
	}
	d.add(st.ghost.Render(strings.Repeat(" ", nameW+1) + spread(first, last, max(w, ansi.StringWidth(first)+ansi.StringWidth(last)+2))))
	if alt != "" {
		d.add(wrapText(alt, st.muted, room, "", "")...)
	}
}

// chartColumns are the values a chart's columns show: a few columns a value
// when they are few, the mean of each column's values when they are many
// (NaN where none is a number).
func chartColumns(vals []float64, width int) []float64 {
	if width <= 0 || len(vals) == 0 {
		return nil
	}
	if len(vals) <= width {
		per := max(1, min(4, width/len(vals)))
		out := make([]float64, 0, len(vals)*per)
		for _, v := range vals {
			for range per {
				out = append(out, v)
			}
		}
		return out
	}
	out := make([]float64, width)
	for i := range out {
		from, to := i*len(vals)/width, (i+1)*len(vals)/width
		sum, n := 0.0, 0
		for _, v := range vals[from:to] {
			if !math.IsNaN(v) {
				sum += v
				n++
			}
		}
		out[i] = math.NaN()
		if n != 0 {
			out[i] = sum / float64(n)
		}
	}
	return out
}

func rangeOf(vals []float64) (lo, hi float64, ok bool) {
	lo, hi = math.Inf(1), math.Inf(-1)
	for _, v := range vals {
		if !math.IsNaN(v) {
			lo, hi, ok = math.Min(lo, v), math.Max(hi, v), true
		}
	}
	if !ok {
		return 0, 0, false
	}
	return lo, hi, true
}

func lastOf(vals []float64) (float64, bool) {
	for i := len(vals) - 1; i >= 0; i-- {
		if !math.IsNaN(vals[i]) {
			return vals[i], true
		}
	}
	return 0, false
}

// spread puts left at the start of width and right at its end.
func spread(left, right string, width int) string {
	if left == right {
		return left
	}
	gap := width - ansi.StringWidth(left) - ansi.StringWidth(right)
	if gap < 1 {
		return left + " … " + right
	}
	return left + strings.Repeat(" ", gap) + right
}

func (d *surfaceDraw) drawImage(c *surface.Object) {
	st := d.st
	lead := st.chip.Render("IMAGE") + " "
	rest := strings.Repeat(" ", ansi.StringWidth(lead))
	d.add(wrapText(orDefault(d.text(c, "alt"), "a picture"), st.text, d.room(), lead, rest)...)
	file := ""
	if v := prop(c, "file"); v != nil {
		file = surface.Clean(d.fill(v))
	}
	if file != "" {
		d.add(wrapText(file+" · shown in the web cockpit and the iPhone app", st.ghost, d.room(), rest, rest)...)
	}
}

func (d *surfaceDraw) drawCode(c *surface.Object, id string) {
	st := d.st
	file := ""
	if v := prop(c, "file"); v != nil {
		file = surface.Clean(d.fill(v))
	}
	lines, lang := codeSpec(c), surface.Str(c, "language")
	var text string
	var runs []int32
	cut := false
	textProp, inline := c.Get("text")
	switch {
	case inline || file == "":
		text = d.fill(textProp)
		name := langFile(lang)
		if name == "" {
			name = orDefault(file, "code.txt")
		}
		runs = highlight.Highlight(name, text, time.Now().Add(50*time.Millisecond)).Runs
	default:
		key := d.path + "#" + id
		code := d.m.surfaces.codes[key]
		if code == nil || code.file != file {
			// Read off the loop (surfaceSchedule), and drawn when it is.
			d.m.surfaces.wanted[key] = surfaceCodeJob{key: key, file: file, lines: lines}
			d.none("Reading " + file + "…")
			return
		}
		if code.err != nil {
			d.add(wrapText(file+": "+code.err.Error(), st.err, d.room(), "", "")...)
			return
		}
		text, runs, cut = code.text, code.runs, code.cut
	}
	label := "CODE"
	switch {
	case !inline && file != "" && lines != "":
		label = file + ":" + lines
	case !inline && file != "":
		label = file
	case lang != "":
		label = strings.ToUpper(lang)
	}
	all := strings.Split(strings.TrimRight(text, "\n"), "\n")
	more := len(all) - surfaceRows
	if more > 0 {
		text = strings.Join(all[:surfaceRows], "\n")
	}
	d.add(st.rule.Render(corners.tl+" ") + st.faint.Render(fit(label, d.room()-2)))
	for _, row := range codeRows(text, runs, max(4, d.room()-2)) {
		if row == nil {
			d.add(st.rule.Render("│ ") + st.ghost.Render("…"))
			continue
		}
		var line strings.Builder
		for _, piece := range row {
			line.WriteString(st.token(piece.class).Render(piece.text))
		}
		d.add(st.rule.Render("│ ") + line.String())
	}
	d.add(st.rule.Render(corners.bl))
	if more > 0 {
		d.none(plural(more, "more line", "more lines"))
	}
	if cut {
		d.none("The file goes on: what is past 256 KB is not shown.")
	}
}

func (d *surfaceDraw) drawDiff(c *surface.Object, id string) {
	st := d.st
	patch, own := prop(c, "patch").(string)
	cut := false
	if !own {
		kept := diffOf(d.version, id)
		if kept == nil {
			d.none("The call kept no diff.")
			return
		}
		if e := surface.Str(kept, "error"); e != "" {
			d.add(wrapText(e, st.err, d.room(), "", "")...)
			return
		}
		patch, cut = surface.Str(kept, "patch"), surface.Bool(kept, "cut")
	}
	var body []string
	added, removed := 0, 0
	for _, line := range strings.Split(strings.TrimRight(patch, "\n"), "\n") {
		switch {
		case strings.HasPrefix(line, "diff "), strings.HasPrefix(line, "index "),
			strings.HasPrefix(line, "--- "), strings.HasPrefix(line, "+++ "):
			continue
		case strings.HasPrefix(line, "+"):
			added++
		case strings.HasPrefix(line, "-"):
			removed++
		}
		body = append(body, line)
	}
	if strings.TrimSpace(patch) == "" || len(body) == 0 {
		d.none("No changes.")
		return
	}
	room := d.room()
	head := "the patch"
	if !own {
		head = orDefault(surface.Str(c, "file"), "the change") + " against " + orDefault(surface.Str(c, "against"), "HEAD")
	}
	d.add(fitRight(st.faint.Render(head), st.ok.Render(fmt.Sprintf("+%d", added))+" "+st.err.Render(fmt.Sprintf("−%d", removed)), room))
	shown := body[:min(len(body), surfaceRows)]
	for _, line := range shown {
		text := fit(strings.ReplaceAll(cockpit.Clean(line), "\t", "    "), room)
		switch {
		case strings.HasPrefix(line, "@@"):
			d.add(st.diffHunk.Render(padTo(text, room)))
		case strings.HasPrefix(line, "+"):
			d.add(st.diffAdd.Render(padTo(text, room)))
		case strings.HasPrefix(line, "-"):
			d.add(st.diffDel.Render(padTo(text, room)))
		default:
			d.add(st.muted.Render(text))
		}
	}
	if more := len(body) - len(shown); more > 0 {
		d.none(plural(more, "more line", "more lines"))
	}
	if cut {
		d.none("The diff was cut at 64 KB.")
	}
}

func (d *surfaceDraw) drawField(c *surface.Object, id string) {
	st := d.st
	label := d.text(c, "label")
	text := valueText(d.value(prop(c, "value")))
	hint := d.text(c, "placeholder")
	bound := d.bound(c)
	room := d.room()
	lead := ""
	if label != "" {
		lead = st.muted.Render(label) + " "
		if ansi.StringWidth(lead) > room/2 {
			d.add(st.muted.Render(fit(label, room)))
			lead = ""
		}
	}
	box := max(4, room-ansi.StringWidth(lead)-4)
	var inner string
	switch {
	case bound && d.focus == id:
		inner = st.selection.Render(fieldWindow(text, d.cursor, box))
	case text == "":
		inner = st.ghost.Render(padTo(fit(hint, box), box))
	default:
		inner = st.text.Render(padTo(fit(strings.ReplaceAll(text, "\n", "⏎"), box), box))
	}
	line := lead + st.rule.Render("[ ") + inner + st.rule.Render(" ]")
	if !bound {
		line = st.ghost.Render(ansi.Strip(line))
	} else {
		d.controls = append(d.controls, surfaceControl{id: id, kind: "TextField", row: len(d.lines), from: d.indent,
			to: d.indent + ansi.StringWidth(line)})
	}
	d.add(line)
}

// fieldWindow is a TextField's text as it is being edited: on one line,
// the cursor in it, and as much around the cursor as fits.
func fieldWindow(text string, cursor, width int) string {
	runes := []rune(strings.ReplaceAll(text, "\n", "⏎"))
	if cursor < 0 || cursor > len(runes) {
		cursor = len(runes)
	}
	shown := make([]rune, 0, len(runes)+1)
	shown = append(append(append(shown, runes[:cursor]...), '▏'), runes[cursor:]...)
	start := 0
	if len(shown) > width {
		start = clamp(cursor-width/2, 0, len(shown)-width)
	}
	return padTo(fit(string(shown[start:min(len(shown), start+width)]), width), width)
}

func (d *surfaceDraw) drawHtml(c *surface.Object) {
	st := d.st
	lead := st.chip.Render("HTML") + " "
	rest := strings.Repeat(" ", ansi.StringWidth(lead))
	d.add(wrapText(orDefault(d.text(c, "alt"), "a page of the agent's"), st.text, d.room(), lead, rest)...)
	where := "an inline page"
	if file := surface.Clean(surface.Str(c, "file")); file != "" {
		where = file
	}
	d.add(wrapText(where+" · drawn live in the web cockpit and the iPhone app", st.ghost, d.room(), rest, rest)...)
}

func (d *surfaceDraw) drawUnknown(c *surface.Object) {
	st := d.st
	lead := st.chip.Render(orDefault(surface.Str(c, "component"), "?")) + " "
	text := d.text(c, "alt")
	if text == "" {
		d.add(lead + st.ghost.Render("not drawn here"))
		return
	}
	d.add(wrapText(text, st.text, d.room(), lead, strings.Repeat(" ", ansi.StringWidth(lead)))...)
}

// footer ends the card: the files the surface reads, its warnings and its
// version, and how to reach its controls.
func (d *surfaceDraw) footer(call surface.Call, infos *surface.Object, focusable bool) {
	st := d.st
	d.indent = 0
	d.trim()
	d.blank()
	var parts []string
	for _, s := range surface.SourcesOf(d.version) {
		info := surface.ObjectOf(prop(infos, s.Name))
		problem := surface.Str(info, "error")
		count, counted := surface.Get(d.doc, "/sources/"+s.Name+"/count")
		bytes, sized := surface.AsNumber(prop(info, "size"))
		switch n, isNumber := count.(float64); {
		case info == nil:
			parts = append(parts, st.ghost.Render(s.Name))
		case problem == "no such file":
			parts = append(parts, st.ghost.Render(s.Name+" · no file yet"))
		case problem != "":
			parts = append(parts, st.err.Render(s.Name+" · "+problem))
		case counted && isNumber:
			parts = append(parts, st.faint.Render(s.Name+" · "+plural(int(n), "row", "rows")))
		case sized:
			parts = append(parts, st.faint.Render(s.Name+" · "+size(int64(bytes))))
		default:
			parts = append(parts, st.faint.Render(s.Name))
		}
	}
	if n := len(call.Warnings); n != 0 {
		parts = append(parts, st.warn.Render(fmt.Sprintf("⚠ %d", n)))
	}
	parts = append(parts, st.ghost.Render(fmt.Sprintf("%s v%d", call.Surface, call.Version)))
	line := strings.Join(parts, st.ghost.Render("  ·  "))
	if focusable {
		line = fitRight(line, st.ghost.Render("ctrl+] focus"), d.width)
	} else {
		line = fit(line, d.width)
	}
	d.lines = append(d.lines, line)
}

// ---------------------------------------------------------------- helpers

func prop(o *surface.Object, key string) any {
	v, _ := o.Get(key)
	return v
}

func has(o *surface.Object, key string) bool {
	_, ok := o.Get(key)
	return ok
}

func childIDs(c *surface.Object) []string {
	var out []string
	for _, v := range surface.ListOf(prop(c, "children")) {
		if id, ok := v.(string); ok && id != "" {
			out = append(out, id)
		}
	}
	return out
}

// oneLine is a text clean and on one line.
func oneLine(s string) string {
	return strings.Join(strings.Fields(cockpit.Clean(s)), " ")
}

// valueText is a value as a page's String() has it: "" for none.
func valueText(v any) string {
	switch x := v.(type) {
	case nil:
		return ""
	case string:
		return x
	case bool:
		return strconv.FormatBool(x)
	case float64:
		return strconv.FormatFloat(x, 'f', -1, 64)
	}
	return surface.Encode(v)
}

// truthy is a value as a page's if reads it.
func truthy(v any) bool {
	switch x := v.(type) {
	case nil:
		return false
	case bool:
		return x
	case float64:
		return x != 0 && !math.IsNaN(x)
	case string:
		return x != ""
	}
	return true
}

// same is a page's ===, for the values options have.
func same(a, b any) bool {
	switch x := a.(type) {
	case nil:
		return b == nil
	case string:
		y, ok := b.(string)
		return ok && x == y
	case float64:
		y, ok := b.(float64)
		return ok && x == y
	case bool:
		y, ok := b.(bool)
		return ok && x == y
	}
	return false
}

func padTo(s string, width int) string {
	if n := ansi.StringWidth(s); n < width {
		return s + strings.Repeat(" ", width-n)
	}
	return s
}

func alignTo(s string, width int, align string) string {
	n := ansi.StringWidth(s)
	if n >= width {
		return s
	}
	switch align {
	case "right":
		return strings.Repeat(" ", width-n) + s
	case "center":
		left := (width - n) / 2
		return strings.Repeat(" ", left) + s + strings.Repeat(" ", width-n-left)
	}
	return s + strings.Repeat(" ", width-n)
}

// toneStyle is the chip of a Badge's tone.
func toneStyle(st styles, tone string) lipgloss.Style {
	var fg lipgloss.TerminalColor
	switch tone {
	case "ok":
		fg = colorOK
	case "warn":
		fg = colorWarn
	case "err":
		fg = colorErr
	case "info":
		fg = colorSynType
	}
	if fg == nil || st.noColor {
		return st.chip
	}
	return lipgloss.NewStyle().Background(colorChip).Foreground(fg).Bold(true).Padding(0, 1)
}

func fileLine(file string, line int) string {
	if line > 0 {
		return fmt.Sprintf("%s:%d", file, line)
	}
	return file
}

// sliderRange is a Slider's least and greatest values, and its step.
func sliderRange(c *surface.Object) (lo, hi, step float64) {
	lo, hi, step = 0, 100, 1
	if n, ok := surface.AsNumber(prop(c, "min")); ok {
		lo = n
	}
	if n, ok := surface.AsNumber(prop(c, "max")); ok {
		hi = n
	}
	if n, ok := surface.AsNumber(prop(c, "step")); ok && n > 0 {
		step = n
	}
	return lo, hi, step
}

// codeSpec is the range of lines a Code names: "10-40", or a line.
func codeSpec(c *surface.Object) string {
	switch v := prop(c, "lines").(type) {
	case string:
		return strings.TrimSpace(v)
	case float64:
		if v >= 1 {
			return strconv.Itoa(int(v))
		}
	}
	return ""
}

// langFile is a name the highlighter knows a language by: it goes by the
// extension.
func langFile(lang string) string {
	lang = strings.ToLower(strings.TrimSpace(lang))
	if lang == "" {
		return ""
	}
	extensions := map[string]string{"golang": "go", "python": "py", "javascript": "js", "typescript": "ts",
		"shell": "sh", "bash": "sh", "zsh": "sh", "rust": "rs", "markdown": "md", "yml": "yaml", "ruby": "rb",
		"kotlin": "kt", "c++": "cpp", "csharp": "cs", "c#": "cs"}
	if ext, ok := extensions[lang]; ok {
		lang = ext
	}
	return "code." + lang
}

// diffOf is the diff a version kept for a Diff, as the call took it.
func diffOf(version *surface.Object, id string) *surface.Object {
	snapshot := surface.ObjectOf(prop(version, "snapshot"))
	return surface.ObjectOf(prop(surface.ObjectOf(prop(snapshot, "diffs")), id))
}

type selectOption struct {
	value any
	label string
}

func selectOptions(c *surface.Object) []selectOption {
	var out []selectOption
	for _, o := range surface.ListOf(prop(c, "options")) {
		if obj := surface.ObjectOf(o); obj != nil && has(obj, "value") {
			v := prop(obj, "value")
			label := surface.Shown(v, true)
			if l := prop(obj, "label"); l != nil {
				label = valueText(l)
			}
			out = append(out, selectOption{value: v, label: label})
			continue
		}
		out = append(out, selectOption{value: o, label: surface.Shown(o, true)})
	}
	return out
}

// selectAt is the option a Select's value is, or -1.
func selectAt(options []selectOption, current any, ok bool) int {
	if !ok {
		return -1
	}
	for i, o := range options {
		if same(o.value, current) {
			return i
		}
	}
	if current != nil {
		text := valueText(current)
		for i, o := range options {
			if valueText(o.value) == text {
				return i
			}
		}
	}
	return -1
}

func surfaceHasControls(comps map[string]*surface.Object) bool {
	for _, c := range comps {
		if controlKinds[surface.Str(c, "component")] {
			return true
		}
	}
	return false
}

// ---------------------------------------------------------------- the keys

// surfaceFocusable are an entry's controls, in the order the keys go
// through them.
func (m *uiModel) surfaceFocusable(id string) []surfaceControl {
	var out []surfaceControl
	seen := map[string]bool{}
	for _, c := range m.surfaces.controls[id] {
		if !seen[c.id] {
			seen[c.id] = true
			out = append(out, c)
		}
	}
	return out
}

// focusSurface gives the keys to the controls of the newest live surface
// that has any.
func (m *uiModel) focusSurface() (tea.Cmd, bool) {
	b := &m.surfaces
	if m.tr == nil {
		return nil, false
	}
	any := false
	for i := len(m.tr.Entries) - 1; i >= 0; i-- {
		e := m.tr.Entries[i]
		if !m.surfaceHead(e) {
			continue
		}
		any = true
		s, ok := m.surfaceStateOf(e)
		if !ok || !surfaceHasControls(s.comps) {
			continue
		}
		if !m.isOpen(e) {
			m.expanded[e.ID] = true
		}
		m.surfaceRedraw(e.ID)
		m.refreshKeep()
		controls := m.surfaceFocusable(e.ID)
		if len(controls) == 0 {
			continue
		}
		m.blurQueue()
		m.blurChanges()
		m.input.Blur()
		b.focus = &surfaceFocus{entry: e.ID, control: controls[0].id, cursor: -1}
		m.surfaceRedraw(e.ID)
		m.refreshKeep()
		m.surfaceReveal(e.ID, controls[0].row)
		return nil, true
	}
	if any {
		return m.notify("the surfaces here have no inputs or buttons", "info"), true
	}
	return nil, false
}

// surfaceReveal scrolls to a row of an entry's card, when it is out of view.
func (m *uiModel) surfaceReveal(id string, row int) {
	for _, s := range m.spans {
		if s.id != id {
			continue
		}
		line := s.start + 2 + row
		if line < m.view.YOffset || line >= m.view.YOffset+m.view.Height {
			m.view.SetYOffset(max(0, line-m.view.Height/2))
			m.scrolled()
		}
		return
	}
}

// blurSurface gives the keys back to the prompt.
func (m *uiModel) blurSurface() {
	b := &m.surfaces
	f := b.focus
	if f == nil {
		return
	}
	b.focus = nil
	if m.queueFocus < 0 {
		m.input.Focus()
	}
	m.surfaceRedraw(f.entry)
	m.refreshKeep()
}

// surfaceMove moves the focus to another of an entry's controls.
func (m *uiModel) surfaceMove(id string, controls []surfaceControl, to int) {
	f := m.surfaces.focus
	n := len(controls)
	if f == nil || n == 0 {
		return
	}
	to = (to%n + n) % n
	f.control, f.cursor = controls[to].id, -1
	m.surfaceRedraw(id)
	m.refreshKeep()
	m.surfaceReveal(id, controls[to].row)
}

// surfaceKey takes the keys while a surface's control has the focus, and
// ctrl+] to give it the focus. It says whether it took the key.
func (m *uiModel) surfaceKey(msg tea.KeyMsg) (tea.Cmd, bool) {
	b := &m.surfaces
	if b.focus == nil {
		if msg.Paste || msg.String() != "ctrl+]" {
			return nil, false
		}
		return m.focusSurface()
	}
	f := b.focus
	e := m.surfaceEntry(f.entry)
	controls := m.surfaceFocusable(f.entry)
	if e == nil || len(controls) == 0 {
		m.blurSurface()
		return nil, false
	}
	at := slices.IndexFunc(controls, func(c surfaceControl) bool { return c.id == f.control })
	if at < 0 {
		// The control is gone (a once Button pressed): the first takes over.
		at, f.control, f.cursor = 0, controls[0].id, -1
		m.surfaceRedraw(e.ID)
		m.refreshKeep()
	}
	c := controls[at]
	field := c.kind == "TextField"
	multiline := field && surface.Bool(m.surfaceComponent(e, c.id), "multiline")
	if msg.Paste {
		if !field {
			m.blurSurface()
			return nil, false
		}
		text := cockpit.Clean(string(msg.Runes))
		if !multiline {
			text = strings.ReplaceAll(text, "\n", " ")
		}
		return m.surfaceText(e, c.id, insertRunes([]rune(text))), true
	}
	switch key := msg.String(); key {
	case "esc", "ctrl+]":
		m.blurSurface()
		return nil, true
	case "tab", "down":
		m.surfaceMove(e.ID, controls, at+1)
		return nil, true
	case "shift+tab", "up":
		m.surfaceMove(e.ID, controls, at-1)
		return nil, true
	case "enter":
		if field {
			m.surfaceMove(e.ID, controls, at+1)
			return nil, true
		}
		return m.surfaceAct(e, c.id, -1, 1), true
	case " ":
		if field {
			return m.surfaceText(e, c.id, insertRunes([]rune{' '})), true
		}
		return m.surfaceAct(e, c.id, -1, 1), true
	case "left", "right":
		step := 1
		if key == "left" {
			step = -1
		}
		switch c.kind {
		case "TextField":
			return m.surfaceText(e, c.id, func(r []rune, at int) ([]rune, int) { return r, clamp(at+step, 0, len(r)) }), true
		case "Select", "Slider", "Tabs":
			return m.surfaceAct(e, c.id, -1, step), true
		}
		m.surfaceMove(e.ID, controls, at+step)
		return nil, true
	case "pgup", "pgdown":
		if c.kind == "Slider" {
			step := 10
			if key == "pgdown" {
				step = -10
			}
			return m.surfaceAct(e, c.id, -1, step), true
		}
		return nil, false // the transcript scrolls, and the focus stays
	case "home", "end":
		if field {
			return m.surfaceText(e, c.id, func(r []rune, at int) ([]rune, int) {
				if key == "home" {
					return r, 0
				}
				return r, len(r)
			}), true
		}
		return nil, false
	case "backspace":
		if field {
			return m.surfaceText(e, c.id, func(r []rune, at int) ([]rune, int) {
				if at == 0 {
					return r, 0
				}
				return append(r[:at-1:at-1], r[at:]...), at - 1
			}), true
		}
	case "delete":
		if field {
			return m.surfaceText(e, c.id, func(r []rune, at int) ([]rune, int) {
				if at >= len(r) {
					return r, at
				}
				return append(r[:at:at], r[at+1:]...), at
			}), true
		}
	case "alt+enter", "ctrl+j":
		if multiline {
			return m.surfaceText(e, c.id, insertRunes([]rune{'\n'})), true
		}
	}
	if field && msg.Type == tea.KeyRunes && !msg.Alt {
		return m.surfaceText(e, c.id, insertRunes(msg.Runes)), true
	}
	// Any other key is the prompt's.
	m.blurSurface()
	return nil, false
}

func insertRunes(add []rune) func([]rune, int) ([]rune, int) {
	return func(r []rune, at int) ([]rune, int) {
		out := make([]rune, 0, len(r)+len(add))
		out = append(append(append(out, r[:at]...), add...), r[at:]...)
		return out, at + len(add)
	}
}

func (m *uiModel) surfaceComponent(e *cockpit.Entry, id string) *surface.Object {
	call, ok := m.surfaceCallOf(e)
	if !ok {
		return nil
	}
	return m.surfaceVersionOf(call.Path).comps[id]
}

// surfaceText changes the text of the TextField focused, and moves its
// cursor.
func (m *uiModel) surfaceText(e *cockpit.Entry, id string, change func([]rune, int) ([]rune, int)) tea.Cmd {
	f := m.surfaces.focus
	s, ok := m.surfaceStateOf(e)
	if !ok || f == nil || !s.head {
		return nil
	}
	ptr, ok := surface.Binding(prop(s.comps[id], "value"))
	if !ok || !surface.Inside(ptr) {
		return nil
	}
	current, _ := surface.Get(s.doc, ptr)
	runes := []rune(valueText(current))
	at := f.cursor
	if at < 0 || at > len(runes) {
		at = len(runes)
	}
	next, at := change(runes, at)
	f.cursor = at
	if string(next) != string(runes) {
		m.surfaces.set(s.call.Path, ptr, string(next))
	}
	m.surfaceRedraw(e.ID)
	m.refreshKeep()
	return nil
}

// surfaceAct does what a control does: a step of a Select, a Slider or a
// Tabs (or the tab given), a Toggle switched, a Disclosure opened or
// closed, a Button pressed.
func (m *uiModel) surfaceAct(e *cockpit.Entry, id string, index, step int) tea.Cmd {
	s, ok := m.surfaceStateOf(e)
	if !ok {
		return nil
	}
	b := &m.surfaces
	c := s.comps[id]
	key := s.call.Path + "#" + id
	redraw := func() {
		m.surfaceRedraw(e.ID)
		m.refreshKeep()
	}
	switch kind := surface.Str(c, "component"); kind {
	case "Tabs":
		n := len(surface.ListOf(prop(c, "tabs")))
		if n == 0 {
			return nil
		}
		if index >= 0 {
			b.tabs[key] = clamp(index, 0, n-1)
		} else {
			b.tabs[key] = ((b.tabs[key]+step)%n + n) % n
		}
		redraw()
	case "Disclosure":
		open := surface.Bool(c, "open")
		if v, ok := b.folds[key]; ok {
			open = v
		}
		b.folds[key] = !open
		redraw()
	case "FileLink":
		file, line := surfaceFileOf(s.doc, s.comps, c)
		if file == "" {
			return nil
		}
		place := fileLine(file, line)
		return m.copy(place, place)
	case "Toggle", "Select", "Slider":
		ptr, ok := surface.Binding(prop(c, "value"))
		if !s.head || !ok || !surface.Inside(ptr) {
			return nil
		}
		current, has := surface.Get(s.doc, ptr)
		switch kind {
		case "Toggle":
			b.set(s.call.Path, ptr, current != true)
		case "Select":
			options := selectOptions(c)
			n := len(options)
			if n == 0 {
				return nil
			}
			at := selectAt(options, current, has)
			switch {
			case at < 0 && step > 0:
				at = 0
			case at < 0:
				at = n - 1
			default:
				at = ((at+step)%n + n) % n
			}
			b.set(s.call.Path, ptr, surface.Clone(options[at].value))
		case "Slider":
			lo, hi, by := sliderRange(c)
			v, ok := surface.AsNumber(current)
			if !ok {
				v = lo
			}
			v = math.Max(lo, math.Min(hi, v+float64(step)*by))
			v = math.Max(lo, math.Min(hi, lo+math.Round((v-lo)/by)*by))
			b.set(s.call.Path, ptr, math.Round(v*1e10)/1e10)
		}
		redraw()
	case "Button":
		return m.surfacePress(e, s, id)
	}
	return nil
}

// surfacePress does what a Button's action says: an event for the agent,
// values set, a file's path or a text copied.
func (m *uiModel) surfacePress(e *cockpit.Entry, s surfaceState, id string) tea.Cmd {
	b := &m.surfaces
	c := s.comps[id]
	key := s.call.Path + "#" + id
	once := surface.Bool(c, "once")
	if !s.head || once && b.used[key] {
		return nil
	}
	if v, ok := surface.Resolve(s.doc, prop(c, "disabled")); ok && truthy(v) {
		return nil
	}
	action := surface.ObjectOf(prop(c, "action"))
	done := false
	var cmd tea.Cmd
	switch {
	case has(action, "event"):
		event := surface.ObjectOf(prop(action, "event"))
		name := surface.Str(event, "name")
		if name == "" {
			return m.notify("the button's event has no name", "warn")
		}
		text := surface.EventText(s.call.Surface, s.call.Version, name, surface.EventData(s.doc, prop(event, "data")))
		cmd, done = m.surfaceSend(name, text)
	case has(action, "set"):
		set := surface.ObjectOf(prop(action, "set"))
		for _, ptr := range set.Keys() {
			if !surface.Inside(ptr) {
				continue
			}
			v, ok := surface.Resolve(s.doc, prop(set, ptr))
			if !ok {
				v = nil
			}
			b.set(s.call.Path, ptr, surface.Clone(v))
		}
		done = true
	case has(action, "open"):
		// The terminal opens no files: the path goes to the clipboard.
		if file := surface.Clean(surface.Str(action, "open")); file != "" {
			cmd, done = m.copy(file, file), true
		}
	case has(action, "copy"):
		var text string
		switch v := prop(action, "copy").(type) {
		case string:
			text = surface.Fill(s.doc, s.comps, v)
		default:
			x, ok := surface.Resolve(s.doc, v)
			text = surface.Shown(x, ok)
		}
		cmd, done = m.copy(text, "the text"), true
	}
	if done && once {
		b.used[key] = true
	}
	m.surfaceRedraw(e.ID)
	m.refreshKeep()
	return cmd
}

// surfaceSend hands an event to the agent: it runs as the user's prompt
// between runs, and waits in the queue while the agent works. It says
// whether the agent will read it.
func (m *uiModel) surfaceSend(name, text string) (tea.Cmd, bool) {
	switch {
	case m.external:
		return m.notify("this session is running in another window or terminal", "warn"), false
	case m.loading:
		return m.notify("the session is still loading", "warn"), false
	case m.state == idle:
		cmd := m.start(text, startOptions{})
		if m.state == idle {
			return cmd, false
		}
		return tea.Batch(cmd, m.showToast("sent to the agent: "+name)), true
	}
	q := m.queue()
	q.Add(text, m.nextModel())
	m.resize()
	if q.Paused {
		return m.notify("queued for the agent: "+name+" — the queue is paused", "info"), true
	}
	return m.notify("queued for the agent: "+name+" — it runs when the agent finishes", "info"), true
}

// surfaceClick acts on a click on a surface's control: an input takes the
// focus, any other control acts. It says whether the click hit one.
func (m *uiModel) surfaceClick(line, x int) (tea.Cmd, bool) {
	b := &m.surfaces
	for _, s := range m.spans {
		if line < s.start || line >= s.end {
			continue
		}
		row, col := line-s.start-2, x-gutter
		for _, c := range b.controls[s.id] {
			if c.row != row || col < c.from || col >= c.to {
				continue
			}
			e := m.surfaceEntry(s.id)
			if e == nil {
				return nil, false
			}
			b.clicked = true
			if c.kind == "TextField" || c.kind == "Slider" {
				if !m.surfaceHead(e) {
					return nil, true
				}
				m.blurQueue()
				m.blurChanges()
				m.input.Blur()
				if old := b.focus; old != nil {
					m.surfaceRedraw(old.entry)
				}
				b.focus = &surfaceFocus{entry: e.ID, control: c.id, cursor: -1}
				m.surfaceRedraw(e.ID)
				m.refreshKeep()
				return nil, true
			}
			if f := b.focus; f != nil && f.entry == e.ID {
				f.control, f.cursor = c.id, -1
				m.surfaceRedraw(e.ID)
			}
			return m.surfaceAct(e, c.id, c.index, 1), true
		}
		return nil, false
	}
	return nil, false
}

// surfaceHints are the key hints while a surface's control has the focus.
func (m *uiModel) surfaceHints() []string {
	kind := ""
	if f := m.surfaces.focus; f != nil {
		for _, c := range m.surfaces.controls[f.entry] {
			if c.id == f.control {
				kind = c.kind
				break
			}
		}
	}
	var pairs []string
	switch kind {
	case "Button":
		pairs = []string{"enter", "press"}
	case "TextField":
		pairs = []string{"type", "to fill it", "enter", "next"}
	case "Toggle":
		pairs = []string{"space", "switch"}
	case "Select":
		pairs = []string{"← →", "choose"}
	case "Slider":
		pairs = []string{"← →", "move", "pgup pgdn", "by ten"}
	case "Tabs":
		pairs = []string{"← →", "tab"}
	case "Disclosure":
		pairs = []string{"enter", "open or close"}
	case "FileLink":
		pairs = []string{"enter", "copy the path"}
	}
	return append(pairs, "tab", "next", "⇧tab", "back", "esc", "to the prompt")
}

// surfacePalette is the palette's way to the surfaces' controls.
func (m *uiModel) surfacePalette() []pickerItem {
	b := &m.surfaces
	b.init()
	if b.heads == nil {
		b.heads, b.paths = m.surfaceScan()
	}
	if len(b.heads) == 0 {
		return nil
	}
	return []pickerItem{{title: "Focus the newest surface", detail: "ctrl+]", search: "ui surface genui controls buttons inputs",
		action: func(m *uiModel) tea.Cmd {
			m.closePicker()
			cmd, _ := m.focusSurface()
			return cmd
		}}}
}
