package main

import (
	"fmt"
	"path/filepath"
	"reflect"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Two layouts. Fullscreen takes a screen of its own, the terminal's
// alternate screen, and keeps the transcript in a view it scrolls; the mouse
// is the cockpit's. Compact, called inline on screen since /compact compacts
// the conversation, stays below the command that started it: the
// transcript goes into the terminal's own scrollback as its entries settle,
// and only what is still changing, the status line and the composer are
// redrawn beneath it. The terminal keeps its scrolling and its selection,
// and the transcript stays on screen after the cockpit exits.

// printAbove prints lines above the program; tests replace it.
var printAbove = tea.Println

// teaMessage reports one of the messages Bubble Tea acts on itself and then
// hands to the model, by the name of its unexported type: the renderer has
// dealt with it by the time the model sees it.
func teaMessage(msg tea.Msg, name string) bool {
	t := reflect.TypeOf(msg)
	return t != nil && t.Name() == name
}

// scrollback is what compact mode has printed above itself. Entries go out
// once, in the transcript's order, when they will not change any more.
type scrollback struct {
	session string              // the session printed; another prints a header first
	tr      *cockpit.Transcript // the transcript printed from
	next    int                 // entries of tr printed
	last    *cockpit.Entry      // the last one printed, for the rule before the next
	printed map[string]bool     // entries of the session printed, by ID
	queue   []string            // lines waiting for the print in flight
	// busy is set while a print is on its way to the renderer. Prints from
	// different updates could overtake each other, so the next waits until
	// the model sees this one come by.
	busy bool
}

// settled reports an entry that will not change any more: prompts once the
// runner has them or never will, tools once they finished or nobody runs
// them any more, and have the picture they read to show.
func (m *uiModel) settled(e *cockpit.Entry) bool {
	switch e.Kind {
	case cockpit.KindUser:
		return e.State != cockpit.Pending
	case cockpit.KindTool:
		return (e.Tool.Terminal() || m.state == idle && !m.external) && m.viewedReady(e)
	}
	return true
}

// persisted reports an entry that comes from the session file, and not only
// from a run's output: those are the ones a rewind takes away. (See liveID
// in the cockpit package for the others.)
func persisted(id string) bool {
	for _, prefix := range []string{"input:", "output:", "tool:", "fork:", "compaction:", "failure:", "stop:"} {
		if strings.HasPrefix(id, prefix) {
			return true
		}
	}
	return false
}

// commit queues the entries that settled since the last update for printing.
func (m *uiModel) commit() {
	if !m.ready || m.loading {
		return
	}
	s := &m.scroll
	width := max(20, m.width-2)
	if s.session != m.sessionID {
		if s.session != "" {
			s.queue = append(s.queue, "")
		}
		s.queue = append(s.queue, m.sessionHeader(width)...)
		s.session, s.tr, s.next, s.last, s.printed = m.sessionID, m.tr, 0, nil, map[string]bool{}
	}
	if s.tr != m.tr {
		// Another transcript of the session: a rewind, a reload, or a view
		// cleared. What it has in common with what was printed stays.
		k := 0
		for k < len(m.tr.Entries) && s.printed[m.tr.Entries[k].ID] {
			k++
		}
		kept := make(map[string]bool, k)
		for _, e := range m.tr.Entries[:k] {
			kept[e.ID] = true
		}
		gone := false
		for id := range s.printed {
			gone = gone || !kept[id] && persisted(id)
		}
		s.tr, s.next, s.printed, s.last = m.tr, k, kept, nil
		if gone {
			// The marker stands between what is gone and what follows.
			s.queue = append(s.queue, "", m.rewindMarker(k, width), "")
		} else if k > 0 {
			s.last = m.tr.Entries[k-1]
		}
	}
	for s.next < len(m.tr.Entries) {
		e := m.tr.Entries[s.next]
		if !m.settled(e) {
			break
		}
		if s.last != nil && !(dense(s.last) && dense(e)) {
			s.queue = append(s.queue, m.timeline(e))
		}
		s.queue = append(s.queue, m.block(e, width, m.promptIndex(s.next), false)...)
		s.printed[e.ID] = true
		s.last = e
		s.next++
	}
}

// flushScrollback hands the queued lines to the terminal: one print at a
// time, so they land in order.
func (m *uiModel) flushScrollback() tea.Cmd {
	s := &m.scroll
	if s.busy || len(s.queue) == 0 {
		return nil
	}
	text := strings.Join(s.queue, "\n")
	s.queue, s.busy = nil, true
	return printAbove(text)
}

// promptIndex is the number of the prompts up to entry i, as shown beside
// them.
func (m *uiModel) promptIndex(i int) int {
	n := 0
	for _, e := range m.tr.Entries[:min(i+1, len(m.tr.Entries))] {
		if e.Kind == cockpit.KindUser {
			n++
		}
	}
	return n
}

// sessionHeader opens a session in the scrollback.
func (m *uiModel) sessionHeader(width int) []string {
	st := m.styles
	brand := st.accent.Render("■") + " " + st.bold.Render("KOU") + st.accent.Render("-") + st.bold.Render("CONVEYOR")
	title := orDefault(m.sessionTitle(), "new session")
	where := st.faint.Render(filepath.Base(m.opt.Workspace)+" / ") + st.text.Render(title)
	id := "session " + shortID(m.sessionID)
	if m.fresh {
		id += " (new)"
	}
	pad := strings.Repeat(" ", margin)
	lines := []string{
		pad + fitRight(brand+"  "+where, st.ghost.Render(id), width-margin),
		pad + st.ghost.Render(fitRight(m.opt.Workspace, m.runSummary(), width-margin)),
	}
	if len(m.tr.Entries) == 0 {
		lines = append(lines, wrapText("Describe the outcome you want — the agent plans, runs commands and reports back as it goes. /help lists the keys; ^F takes a screen of its own.", st.faint, width, pad, pad)...)
	}
	return append(lines, "")
}

// rewindMarker tells the scrollback that the entries above from a prompt on
// left the session: the prompt was edited, here or elsewhere.
func (m *uiModel) rewindMarker(kept, width int) string {
	st := m.styles
	pad := strings.Repeat(" ", margin)
	if len(m.tr.Entries) == 0 {
		return pad + st.faint.Render("── the view starts over; the session keeps its history "+strings.Repeat("─", max(0, width-margin-56)))
	}
	lead := fmt.Sprintf("↶ prompt %02d edited", m.promptIndex(kept-1)+1)
	rest := " — the session goes on from before it; what followed above is gone "
	return pad + st.accent.Render(lead) + st.faint.Render(rest+strings.Repeat("─", max(0, width-margin-lipgloss.Width(lead+rest))))
}

// liveLines renders the entries that are not in the scrollback yet: the
// run's, while they change. The newest show when they do not all fit.
func (m *uiModel) liveLines(room int) []string {
	if m.loading {
		return nil
	}
	width := max(20, m.width-2)
	var lines []string
	previous := m.scroll.last
	for i, e := range m.tr.Entries {
		if m.scroll.printed[e.ID] && m.scroll.tr == m.tr {
			continue
		}
		if previous != nil && !(dense(previous) && dense(e)) {
			lines = append(lines, m.timeline(e))
		}
		lines = append(lines, m.block(e, width, m.promptIndex(i), false)...)
		previous = e
	}
	if len(lines) > room && room > 1 {
		hidden := len(lines) - room + 1
		lines = append([]string{m.styles.faint.Render(fmt.Sprintf("%s⋮ %d more lines", strings.Repeat(" ", gutter-2), hidden))}, lines[hidden:]...)
	}
	return lines
}

// compactView is the part of the screen compact mode redraws: what is still
// changing, or a list or form, then the status line and the composer.
func (m *uiModel) compactView() string {
	if !m.ready || m.quitting {
		return ""
	}
	st := m.styles
	width := m.width
	dock := m.dock(hoverTarget{level: -1})
	below := append(append([]string{fit(strings.Repeat(" ", margin)+m.statusLine(), width) + m.osc52}, dock...),
		fit(m.footer(), width),
	)
	used := 2 + len(dock)
	room := max(1, m.height-used-1)
	var above []string
	var box []string
	if preview := m.previewing(); preview != nil && room >= 5 {
		box = m.previewBox(preview, min(width-4, 144), room)
	}
	switch {
	case m.form != nil:
		above = strings.Split(m.form.view(st, min(84, width-2), min(room, 22)), "\n")
	case m.picker != nil:
		above = strings.Split(m.picker.view(st, min(84, width-2), min(room, 18)), "\n")
	case len(box) != 0:
		// The image the cursor is on shows large where the run shows.
		pad := strings.Repeat(" ", max(0, (width-ansi.StringWidth(box[0]))/2))
		for _, line := range box {
			above = append(above, pad+line)
		}
	default:
		above = m.liveLines(room)
	}
	// A list or form taller than the room keeps its top, as in fullscreen.
	above = above[:min(len(above), room)]
	// A blank line sets the screen off from the scrollback above it.
	return strings.Join(append(append([]string{""}, above...), below...), "\n")
}

// ---------------------------------------------------------------- switching

// layoutName is the name of the current layout.
func (m *uiModel) layoutName() string {
	if m.compact {
		return cockpit.LayoutCompact
	}
	return cockpit.LayoutFullscreen
}

// layoutTitle is what a layout is called on screen: the compact layout goes
// by inline, as /compact compacts the conversation.
func layoutTitle(layout string) string {
	if layout == cockpit.LayoutCompact {
		return "inline"
	}
	return layout
}

// setLayout switches to a layout and makes it the one the next start takes.
func (m *uiModel) setLayout(layout string) tea.Cmd {
	if layout == m.layoutName() {
		return m.notify("already "+layoutTitle(layout), "info")
	}
	if m.opt.preferences != "" {
		_ = cockpit.SaveLayout(m.opt.preferences, layout)
	}
	m.compact = layout == cockpit.LayoutCompact
	// The pictures stay on the screen that is left; the other gets them
	// anew, once the terminal is there.
	m.leaveScreen(m.compact)
	m.press, m.sel, m.scrubbing, m.resizing, m.px, m.py = nil, nil, false, false, -1, -1
	if m.compact {
		m.blurChanges() // the panel waits for fullscreen
	}
	clear(m.cache) // prompts offer an edit only in fullscreen
	m.layout()
	m.refresh()
	// Nothing is printed until the terminal has left the other screen: a
	// print meant for the scrollback would go to the alternate screen.
	m.switching = true
	note := m.notify(layoutTitle(layout)+" · ^F switches back", "info")
	if m.compact {
		return tea.Batch(tea.ExitAltScreen, tea.DisableMouse, note)
	}
	return tea.Batch(tea.EnterAltScreen, tea.EnableMouseAllMotion, note)
}
