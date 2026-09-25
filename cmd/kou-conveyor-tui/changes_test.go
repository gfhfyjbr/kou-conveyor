package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// A driver plays the Bubble Tea runtime across the steps of a test: what
// the commands of one step deliver reaches the model in the next, as runs,
// snapshots and reloads go on between them.
type driver struct {
	t    *testing.T
	m    *uiModel
	msgs chan tea.Msg
}

func newDriver(t *testing.T, m *uiModel) *driver {
	return &driver{t: t, m: m, msgs: make(chan tea.Msg, 1024)}
}

func (d *driver) run(cmd tea.Cmd) {
	if cmd == nil {
		return
	}
	go func() {
		msg := cmd()
		if batch, ok := msg.(tea.BatchMsg); ok {
			for _, c := range batch {
				d.run(c)
			}
			return
		}
		if msg == nil {
			return
		}
		select {
		case d.msgs <- msg:
		case <-d.t.Context().Done():
		}
	}()
}

// send gives the model a message, as the terminal would.
func (d *driver) send(msg tea.Msg) {
	_, cmd := d.m.Update(msg)
	d.run(cmd)
}

// until feeds the model what comes until done reports true.
func (d *driver) until(what string, done func() bool) {
	d.t.Helper()
	timeout := time.After(20 * time.Second)
	for !done() {
		select {
		case msg := <-d.msgs:
			switch msg.(type) {
			case tickMsg, noticeMsg, toastMsg:
				continue // timers would keep the loop busy
			}
			d.send(msg)
		case <-timeout:
			d.t.Fatalf("timed out waiting for %s", what)
		}
	}
}

// prompt runs a prompt, and waits for the last snapshot of what it changed.
func (d *driver) prompt(text string) {
	d.t.Helper()
	d.m.input.SetValue(text)
	d.send(key("enter"))
	d.until("the run and its last snapshot", func() bool { return d.m.state == idle && d.m.tracker == nil })
}

// promptChanging runs a prompt during which change changes the workspace,
// the way the agent's commands would; the run is then stopped.
func (d *driver) promptChanging(text string, change func()) {
	d.t.Helper()
	m := d.m
	m.input.SetValue("wait: " + text)
	d.send(key("enter"))
	d.until("the prompt to reach the runner", func() bool {
		return m.state == running && len(m.tr.Entries) > 0 && m.tr.Entries[len(m.tr.Entries)-1].State == ""
	})
	change()
	m.tracker.Poke()
	records, session, message := m.changeRecords(), m.sessionID, m.runMessage
	d.until("a snapshot of the change", func() bool {
		ex, _ := records.Exchange(session, message)
		return ex.After != ex.Before
	})
	d.send(key("ctrl+c"))
	d.until("the run to stop", func() bool { return m.state == idle && m.tracker == nil })
}

func needGit(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
}

// changesModel is a cockpit whose prompt history is kept outside its
// workspace, which then changes only as the runs change it.
func changesModel(t *testing.T, width, height int) (*uiModel, *driver) {
	t.Helper()
	needGit(t)
	m := testModel(t)
	m.opt.historyFile = filepath.Join(t.TempDir(), "history.json")
	d := newDriver(t, m)
	d.send(tea.WindowSizeMsg{Width: width, Height: height})
	return m, d
}

func writeFile(t *testing.T, dir, name, content string) {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}

// panelSettled reports that the panel shows what it has to: the files of
// its prompt, and the diff of the one picked.
func panelSettled(m *uiModel) bool {
	c := &m.changes
	switch {
	case c.message == "":
		return true
	case c.list == nil:
		return false
	}
	f := c.file()
	if f == nil || f.Binary {
		return true
	}
	diff := c.diffs[c.diffKey(f.Path)]
	return diff != nil && !diff.loading
}

// screen is the frame as text, a line per row. It must fit the terminal.
func screen(t *testing.T, m *uiModel) []string {
	t.Helper()
	lines := strings.Split(m.View(), "\n")
	if len(lines) != m.height {
		t.Fatalf("%d lines for %d rows", len(lines), m.height)
	}
	for i, line := range lines {
		if w := ansi.StringWidth(line); w > m.width {
			t.Fatalf("line %d is %d wide for %d: %q", i, w, m.width, ansi.Strip(line))
		}
		lines[i] = ansi.Strip(line)
	}
	return lines
}

// panelText is what the panel shows, without the transcript beside it.
func panelText(t *testing.T, m *uiModel) string {
	t.Helper()
	g, ok := m.panelGeometry()
	if !ok {
		t.Fatal("the changes panel is not on screen")
	}
	var out []string
	for _, line := range screen(t, m)[g.top : g.top+g.height] {
		out = append(out, ansi.Cut(line, g.left, m.width))
	}
	return strings.Join(out, "\n")
}

// panelCell returns where text shows in the panel.
func panelCell(t *testing.T, m *uiModel, text string) cell {
	t.Helper()
	g, _ := m.panelGeometry()
	for i, line := range strings.Split(panelText(t, m), "\n") {
		if col := strings.Index(line, text); col >= 0 {
			return cell{row: g.top + i, col: g.left + ansi.StringWidth(line[:col])}
		}
	}
	t.Fatalf("%q is not in the panel:\n%s", text, panelText(t, m))
	return cell{}
}

func mustShow(t *testing.T, m *uiModel, wants ...string) {
	t.Helper()
	panel := panelText(t, m)
	for _, want := range wants {
		if !strings.Contains(panel, want) {
			t.Fatalf("no %q in the panel:\n%s", want, panel)
		}
	}
}

func TestChangesPanelShowsThePromptInView(t *testing.T) {
	m, d := changesModel(t, 150, 18)
	d.prompt("touch one")
	d.prompt("touch two")

	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })
	if !m.changes.focused || m.input.Focused() {
		t.Fatal("the panel did not take the keys")
	}
	// The newest prompt, and the file it changed, beside the transcript.
	mustShow(t, m, "CHANGES · PROMPT 02", "+1 −0 · 1 file", "M touched.txt", "─ touched.txt", "1 + touch two", "1   2   touch one")
	if pw, covers := m.panelColumns(); covers || m.view.Width != 150-margin-2-pw {
		t.Fatalf("transcript width %d beside a panel of %d", m.view.Width, pw)
	}

	// Scrolled to the top, the transcript shows the first prompt, and so
	// does the panel.
	for i := 0; !m.view.AtTop() && i < 50; i++ {
		d.send(tea.MouseMsg{Button: tea.MouseButtonWheelUp, Action: tea.MouseActionPress, X: 10, Y: 5})
	}
	d.until("the first prompt's changes", func() bool { return m.changes.list != nil && panelSettled(m) })
	mustShow(t, m, "CHANGES · PROMPT 01", "A touched.txt", "1 + touch one")
	if strings.Contains(panelText(t, m), "touch two") {
		t.Fatalf("the first prompt shows the second's change:\n%s", panelText(t, m))
	}

	// The wheel over the diff scrolls the diff, not the transcript.
	offset := m.view.YOffset
	d.send(tea.MouseMsg{Button: tea.MouseButtonWheelDown, Action: tea.MouseActionPress, X: 140, Y: 12})
	if m.view.YOffset != offset {
		t.Fatal("the wheel over the panel scrolled the transcript")
	}

	// ctrl+g closes it, and the transcript takes the room back.
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	if m.changesShown() || m.view.Width != m.width-margin-2 || !m.input.Focused() {
		t.Fatalf("closed: shown %v, width %d, composer focused %v", m.changesShown(), m.view.Width, m.input.Focused())
	}
}

func TestChangesPanelFollowsARun(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	ws := m.opt.Workspace
	writeFile(t, ws, "src/app.go", "package app\n")
	writeFile(t, ws, "docs/guide.md", "# Guide\n")
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	mustShow(t, m, "What the agent changes")

	d.send(key("tab")) // back to the composer
	m.input.SetValue("wait for changes")
	d.send(key("enter"))
	d.until("the run's changes", func() bool {
		return m.state == running && m.changes.message == m.runMessage && m.changes.list != nil
	})
	mustShow(t, m, "● LIVE", "No files changed yet.")

	// The run changes files: the panel shows the one changed last.
	writeFile(t, ws, "src/app.go", "package app\n\nfunc Run() {}\n")
	writeFile(t, ws, "src/new.go", "package app\n")
	os.Remove(filepath.Join(ws, "docs", "guide.md"))
	m.tracker.Poke()
	d.until("three changed files", func() bool {
		return m.changes.list != nil && len(m.changes.list.files) == 3 && panelSettled(m)
	})
	mustShow(t, m, "● LIVE", "docs/", "D guide.md", "src/", "M app.go", "A new.go", "+3 −1 · 3 files")
	if m.changes.selected != "docs/guide.md" {
		t.Fatalf("selected %q", m.changes.selected)
	}

	// Picking a file shows it, and stops following the run.
	at := panelCell(t, m, "new.go")
	d.run(click(m, at.col, at.row))
	d.until("the picked file's diff", func() bool { return panelSettled(m) })
	if m.changes.selected != "src/new.go" || m.changes.follow {
		t.Fatalf("selected %q, follow %v", m.changes.selected, m.changes.follow)
	}
	mustShow(t, m, "↻ follow", "1 + package app")

	writeFile(t, ws, "src/app.go", "package app\n\nfunc Run() {}\nfunc Stop() {}\n")
	m.tracker.Poke()
	d.until("the next snapshot", func() bool {
		return m.changes.list != nil && slices.Equal(m.changes.list.latest, []string{"src/app.go"}) && panelSettled(m)
	})
	if m.changes.selected != "src/new.go" {
		t.Fatalf("the picked file gave way to %q", m.changes.selected)
	}

	// f follows the run again.
	d.send(runes("f"))
	d.until("following", func() bool { return m.changes.selected == "src/app.go" && panelSettled(m) })
	mustShow(t, m, "─ src/app.go", "4 + func Stop() {}")

	// Stopped, the run takes a last snapshot, and the panel is no longer live.
	d.send(key("ctrl+c"))
	d.until("the run to stop", func() bool { return m.state == idle && m.tracker == nil && panelSettled(m) })
	if strings.Contains(panelText(t, m), "LIVE") {
		t.Fatalf("still live:\n%s", panelText(t, m))
	}
}

func TestChangesPanelKeys(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	ws := m.opt.Workspace
	d.promptChanging("one", func() {
		writeFile(t, ws, "a/x.txt", "x\n")
		writeFile(t, ws, "a/y.txt", "y\n")
		writeFile(t, ws, "b.txt", "b\n")
	})
	d.promptChanging("two", func() { writeFile(t, ws, "c.txt", "c\n") })

	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })
	mustShow(t, m, "PROMPT 02", "A c.txt")

	// [ and ] step from prompt to prompt, and take the transcript along.
	d.send(runes("["))
	d.until("the first prompt's changes", func() bool { return m.changes.list != nil && panelSettled(m) })
	mustShow(t, m, "PROMPT 01", "a/", "A x.txt", "A y.txt", "A b.txt", "─ a/x.txt")
	if !m.changes.pinned {
		t.Fatal("the prompt picked did not stay")
	}
	d.send(runes("["))
	if !strings.Contains(m.note.text, "first prompt") {
		t.Fatalf("note %q", m.note.text)
	}

	// ↑ ↓ go along the tree; a file shows as the cursor lands on it.
	d.send(key("down"))
	d.until("y.txt", func() bool { return m.changes.selected == "a/y.txt" && panelSettled(m) })
	mustShow(t, m, "─ a/y.txt", "1 + y")
	d.send(key("up"))
	d.send(key("up"))
	if r := m.changes.rows[m.changes.cursor]; !r.folder || r.path != "a" {
		t.Fatalf("cursor on %+v", r)
	}
	// ← folds the folder, → unfolds it.
	d.send(key("left"))
	if len(m.changes.rows) != 2 || strings.Contains(panelText(t, m), "x.txt") && !strings.Contains(panelText(t, m), "─ a/x.txt") {
		t.Fatalf("folded rows %+v", m.changes.rows)
	}
	d.send(key("right"))
	if len(m.changes.rows) != 4 {
		t.Fatalf("unfolded rows %+v", m.changes.rows)
	}

	// Keys the panel has no use for go to the composer, which takes the
	// keys back.
	d.send(runes("h"))
	if m.changes.focused || m.input.Value() != "h" {
		t.Fatalf("focused %v, composer %q", m.changes.focused, m.input.Value())
	}
	// tab goes back and forth.
	d.send(key("tab"))
	if !m.changes.focused {
		t.Fatal("tab did not reach the panel")
	}
	d.send(key("tab"))
	if m.changes.focused || !m.input.Focused() {
		t.Fatal("tab did not go back to the composer")
	}

	// Scrolling the transcript lets go of the prompt [ picked.
	d.send(key("pgdown"))
	d.send(key("end"))
	m.input.SetValue("")
	d.send(key("end"))
	d.until("the newest prompt again", func() bool { return m.changes.list != nil && panelSettled(m) })
	mustShow(t, m, "PROMPT 02")
}

func TestSelectingInTheDiffCopiesTheText(t *testing.T) {
	copied := fakeClipboard(t)
	m, d := changesModel(t, 150, 30)
	d.prompt("touch one")
	d.prompt("touch two")
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })

	g, _ := m.panelGeometry()
	from := panelCell(t, m, "touch two")
	to := panelCell(t, m, "touch one")
	dragAndCopy(t, m, from, cell{to.row, to.col + len("touch one") - 1})
	if len(*copied) != 1 || (*copied)[0] != "touch two\ntouch one" {
		t.Fatalf("copied %q", *copied)
	}
	// From the gutter, the numbers come along.
	dragAndCopy(t, m, cell{from.row, g.content}, cell{from.row, from.col + len("touch two") - 1})
	if got := (*copied)[1]; got != "      1 + touch two" {
		t.Fatalf("copied %q", got)
	}

	// The pointer says what a click does.
	file := panelCell(t, m, "touched.txt")
	move(m, file.col, file.row)
	if h := m.hover(); h.kind != hoverChangeRow || m.pointerShape(h) != "pointer" || m.hoverHint(h) != "click shows the file's diff" {
		t.Fatalf("hover %+v", h)
	}
	move(m, from.col+1, from.row)
	if h := m.hover(); h.kind != hoverText || m.pointerShape(h) != "text" {
		t.Fatalf("hover %+v", h)
	}
	closer := panelCell(t, m, "×")
	move(m, closer.col, closer.row)
	if h := m.hover(); h.kind != hoverPanelButton || h.id != "close" {
		t.Fatalf("hover %+v", h)
	}
	d.run(click(m, closer.col, closer.row))
	if m.changesShown() {
		t.Fatal("× did not close the panel")
	}
}

// Where the terminal is too narrow for both, the panel covers the
// transcript; every size still fits.
func TestChangesPanelFitsTheTerminal(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	d.prompt("touch one")
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })
	for _, size := range [][2]int{{220, 50}, {150, 30}, {110, 20}, {109, 20}, {60, 12}, {40, 10}, {24, 8}} {
		d.send(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		screen(t, m)
		_, covers := m.panelColumns()
		if covers != (size[0] < 110) {
			t.Fatalf("%v: covers %v", size, covers)
		}
	}
	d.send(tea.WindowSizeMsg{Width: 80, Height: 24})
	mustShow(t, m, "CHANGES", "A touched.txt", "1 + touch one")
	// Covering the transcript, the panel takes the clicks on it.
	at := panelCell(t, m, "touched.txt")
	d.run(click(m, at.col, at.row))
	if !m.changes.focused || m.changes.selected != "touched.txt" {
		t.Fatalf("focused %v, selected %q", m.changes.focused, m.changes.selected)
	}
}

// dragEdge drags the left button from one cell to another, through the
// columns between, as the terminal reports it.
func dragEdge(d *driver, from, to cell) {
	d.send(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: from.col, Y: from.row})
	step := 1
	if to.col < from.col {
		step = -1
	}
	for x := from.col; x != to.col; {
		x += step
		d.send(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: x, Y: to.row})
	}
	d.send(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: to.col, Y: to.row})
}

// The panel's edge drags, and < > move it: the transcript takes what the
// panel leaves, each keeps room of its own, and the width is kept for the
// next start.
func TestChangesPanelResizes(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	m.opt.preferences = filepath.Join(t.TempDir(), "preferences.json")
	saved := func() int { return cockpit.LoadPreferences(m.opt.preferences).ChangesWidth }
	width := func() int {
		pw, covers := m.panelColumns()
		if covers || m.view.Width != m.width-margin-2-pw {
			t.Fatalf("a panel of %d (covering %v) beside a transcript of %d", pw, covers, m.view.Width)
		}
		screen(t, m)
		return pw
	}
	edge := func() cell {
		g, _ := m.panelGeometry()
		return cell{row: g.top + 4, col: g.left}
	}
	d.prompt("touch one")
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })
	if w := width(); w != 150*9/20 {
		t.Fatalf("default width %d", w)
	}

	// The edge says what it does, and lights up.
	at := edge()
	move(m, at.col, at.row)
	h := m.hover()
	if h.kind != hoverPanelEdge || m.pointerShape(h) != "ew-resize" || !strings.Contains(m.hoverHint(h), "drag to resize the changes") {
		t.Fatalf("hover %+v, shape %q, hint %q", h, m.pointerShape(h), m.hoverHint(h))
	}
	if line := screen(t, m)[at.row]; !strings.Contains(line, "┃") {
		t.Fatalf("the edge does not light up: %q", line)
	}
	// Beside it, the transcript's scrollbar and the panel are what they were.
	if move(m, at.col+1, at.row); m.hover().kind == hoverPanelEdge {
		t.Fatal("the panel's inside drags")
	}

	// Dragged 20 columns to the left, the panel takes 20 more, and keeps
	// them; the keys stay in the panel.
	dragEdge(d, at, cell{at.row, at.col - 20})
	if w := width(); w != 87 || saved() != 87 || !m.changes.focused {
		t.Fatalf("width %d, saved %d, focused %v", w, saved(), m.changes.focused)
	}
	mustShow(t, m, "CHANGES · PROMPT 01", "A touched.txt", "1 + touch one")
	// While it drags, the status line says how wide it is.
	at = edge()
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: at.col, Y: at.row})
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: at.col + 2, Y: at.row + 3})
	if hint := m.hoverHint(m.hover()); hint != "the changes take 85 columns · letting go keeps them" || m.pointerShape(m.hover()) != "ew-resize" {
		t.Fatalf("hint %q", hint)
	}
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: at.col + 2, Y: at.row + 3})

	// However far it goes, the transcript keeps 40 columns and the panel 36.
	at = edge()
	dragEdge(d, at, cell{at.row, 3})
	if w := width(); w != 150-transcriptMinColumns || saved() != w {
		t.Fatalf("widest: %d, saved %d", w, saved())
	}
	at = edge()
	dragEdge(d, at, cell{at.row, 149})
	if w := width(); w != panelMinColumns || saved() != w {
		t.Fatalf("narrowest: %d, saved %d", w, saved())
	}
	mustShow(t, m, "CHANGES", "touched.txt")

	// < and > move the edge four columns, as shift+← and shift+→ do; the
	// width is kept once the keys stop.
	d.send(runes("<"))
	d.send(runes("<"))
	d.send(key("shift+left"))
	d.send(key("shift+right"))
	if w := width(); w != panelMinColumns+8 || !m.changes.focused || m.input.Value() != "" {
		t.Fatalf("keys: width %d, focused %v, composer %q", w, m.changes.focused, m.input.Value())
	}
	d.until("the width saved", func() bool { return saved() == panelMinColumns+8 })

	// = gives the panel its default width back, and so does a double click
	// on the edge.
	d.send(runes("="))
	if w := width(); w != 150*9/20 || saved() != 0 {
		t.Fatalf("restored: %d, saved %d", w, saved())
	}
	d.send(runes(">"))
	d.until("the width saved", func() bool { return saved() == 150*9/20-4 })
	at = edge()
	d.run(click(m, at.col, at.row))
	d.run(click(m, at.col, at.row))
	if w := width(); w != 150*9/20 || saved() != 0 {
		t.Fatalf("after a double click: %d, saved %d", w, saved())
	}
	// A drag that comes back to where it started is a click: a default
	// width stays the default.
	at = edge()
	dragEdge(d, at, cell{at.row, at.col - 1})
	dragEdge(d, cell{at.row, at.col - 1}, at)
	if m.changes.width == 0 || saved() != m.changes.width {
		t.Fatalf("a drag chose %d, saved %d", m.changes.width, saved())
	}
	d.send(runes("="))
	m.changes.edgeClick = time.Time{}
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: at.col, Y: at.row})
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: at.col - 1, Y: at.row})
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: at.col, Y: at.row})
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: at.col, Y: at.row})
	if m.changes.width != 0 || saved() != 0 {
		t.Fatalf("a drag back and forth chose %d, saved %d", m.changes.width, saved())
	}
	// Two clicks far apart are two clicks.
	m.changes.edgeClick = time.Time{}
	m.changes.width = 60
	m.relayout()
	d.run(click(m, edge().col, at.row))
	m.changes.edgeClick = m.changes.edgeClick.Add(-time.Second)
	d.run(click(m, edge().col, at.row))
	if w := width(); w != 60 {
		t.Fatalf("slow clicks restored the width: %d", w)
	}

	// The next start takes the width kept; a smaller terminal makes it
	// smaller, and gives it back as it grows.
	dragEdge(d, edge(), cell{at.row, edge().col - 30})
	if saved() != 90 {
		t.Fatalf("saved %d", saved())
	}
	if next := newModel(t.Context(), m.opt); !next.changes.open || next.changes.width != 90 {
		t.Fatalf("the next start: open %v, width %d", next.changes.open, next.changes.width)
	}
	d.send(tea.WindowSizeMsg{Width: 120, Height: 30})
	if w := width(); w != 120-transcriptMinColumns {
		t.Fatalf("in 120 columns: %d", w)
	}
	d.send(tea.WindowSizeMsg{Width: 200, Height: 30})
	if w := width(); w != 90 {
		t.Fatalf("in 200 columns: %d", w)
	}
	// Where the panel covers the transcript, there is no edge to move.
	d.send(tea.WindowSizeMsg{Width: 100, Height: 30})
	g, _ := m.panelGeometry()
	if move(m, g.left, g.top+4); m.hover().kind == hoverPanelEdge {
		t.Fatal("a covering panel has an edge")
	}
	d.send(runes("<"))
	if pw, covers := m.panelColumns(); !covers || pw != 100 || !strings.Contains(m.note.text, "cover the transcript") {
		t.Fatalf("covering: %d %v, note %q", pw, covers, m.note.text)
	}
}

// Without git, the panel says why there is nothing to show.
func TestChangesPanelWithoutGit(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	t.Setenv("PATH", t.TempDir())
	d.prompt("touch one")
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.until("the changes", func() bool { return panelSettled(m) })
	mustShow(t, m, "Changes are recorded with git, which is not on PATH.")
}

// Inline, what a run changed is summed up in the scrollback.
func TestInlineRunsSumUpTheirChanges(t *testing.T) {
	needGit(t)
	m, scrollback := compactModel(t)
	d := newDriver(t, m)
	m.input.SetValue("touch one")
	d.send(key("enter"))
	d.until("the summary", func() bool {
		return m.state == idle && strings.Contains(scrollback(), "^G shows the diffs")
	})
	if !strings.Contains(scrollback(), "± ") || !strings.Contains(scrollback(), "touched.txt") {
		t.Fatalf("scrollback:\n%s", scrollback())
	}
	// ctrl+g takes a screen of its own for the panel.
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	if m.compact || !m.changesShown() {
		t.Fatalf("compact %v, shown %v", m.compact, m.changesShown())
	}
}

// A prompt forced in while the agent works belongs to the run of the prompt
// before it: the panel stays on that run, and goes on showing what it
// changes. It steps over forced prompts too.
func TestChangesPanelStaysOnTheRunAfterAForcedPrompt(t *testing.T) {
	m, d := changesModel(t, 150, 30)
	d.send(tea.KeyMsg{Type: tea.KeyCtrlG})
	d.send(tea.KeyMsg{Type: tea.KeyTab})
	m.input.SetValue("steer touch")
	d.send(key("enter"))
	forced := false
	d.until("the run", func() bool {
		if running := m.tr.Running(); !forced && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
			forced = true
			m.input.SetValue("use pnpm")
			d.send(ctrlX)
		}
		return m.state == idle && m.tracker == nil
	})
	if got := kinds(m.tr); got != "user,tool:done,user,tool:done,assistant" {
		t.Fatalf("transcript = %s", got)
	}
	// The run's last snapshot reaches the panel, which stayed on the run.
	d.until("the changes", func() bool {
		return panelSettled(m) && m.changes.list != nil && len(m.changes.list.files) == 1
	})
	if c := &m.changes; c.message != promptMessage(m.tr.Entries[0]) {
		t.Fatalf("the panel shows %q", c.message)
	}
	mustShow(t, m, "CHANGES · PROMPT 01", "A touched.txt")
	if !strings.Contains(ansi.Strip(strings.Join(m.lines, "\n")), "⚡ forced in") {
		t.Fatal("the transcript does not mark the forced prompt")
	}
	// [ and ] find no other prompt to step to.
	d.send(tea.KeyMsg{Type: tea.KeyTab})
	d.send(runes("]"))
	if m.changes.message != promptMessage(m.tr.Entries[0]) || !strings.Contains(m.note.text, "last prompt") {
		t.Fatalf("stepped to %q, note %q", m.changes.message, m.note.text)
	}
}
