package main

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The pointer. What it is over shows whether a click does something: the
// row lights up and the status line says what; text takes a selection and
// the composer a cursor. Terminals that let applications set the pointer's
// shape (OSC 22: kitty, Ghostty, foot, xterm and others) show a hand over
// what can be clicked and an I-beam over text.

type hoverKind int

const (
	hoverNone        hoverKind = iota
	hoverText                  // transcript text, for selecting
	hoverComposer              // the composer: a click places the cursor
	hoverEdit                  // a prompt's header: a click edits the prompt
	hoverFold                  // a block that a click folds or unfolds
	hoverStarter               // a task on the welcome screen
	hoverEffort                // the effort control
	hoverPicker                // a row of a list
	hoverScrollbar             // the transcript's scrollbar
	hoverChangeRow             // a file or folder in the changes panel
	hoverPanelButton           // a button of the changes panel
	hoverQueue                 // a prompt of the queue: a click selects it
	hoverAttachment            // an image above the composer: a click shows it large
	hoverPreview               // the large picture of an image
	hoverModel                 // the model control: a click lists the models
	hoverPanelEdge             // the changes panel's edge: it drags
	hoverRun                   // the run button: a click runs, queues, forces in or stops
)

type hoverTarget struct {
	kind   hoverKind
	row    int    // screen row
	id     string // the entry, for hoverEdit and hoverFold; the path or button in the changes panel
	level  int    // hoverEffort: the bar, or -1 beside the bars
	folder bool   // hoverChangeRow: a folder
	// from and to are the columns of what is under the pointer, [from, to),
	// where that is narrower than the row: a task on the welcome screen, a
	// control of the composer.
	from, to int
}

// hover is what is under the pointer, as the screen is now.
func (m *uiModel) hover() hoverTarget {
	x, y := m.px, m.py
	none := hoverTarget{level: -1}
	switch {
	case x < 0 || y < 0 || m.form != nil:
		return none
	case m.resizing:
		// The edge is the pointer's wherever it goes while it drags.
		return hoverTarget{kind: hoverPanelEdge, row: y, level: -1}
	case m.picker != nil:
		if m.picker.rowAt(y) >= 0 {
			return hoverTarget{kind: hoverPicker, row: y, level: -1}
		}
		return none
	case m.inPreview(x, y):
		return hoverTarget{kind: hoverPreview, row: y, level: -1}
	case m.onPanelEdge(x, y):
		return hoverTarget{kind: hoverPanelEdge, row: y, level: -1}
	case m.inPanel(x, y):
		return m.changesHover(x, y)
	case y >= m.top() && y < m.top()+m.view.Height:
		at := hoverTarget{row: y, level: -1}
		line := m.view.YOffset + y - m.top()
		if m.onScrollbar(x, y) {
			at.kind = hoverScrollbar
		} else if s, ok := m.starterAt(line, x); ok && m.input.Value() == "" {
			at.kind, at.from, at.to = hoverStarter, s.left, s.right
		} else if e := m.promptHeaderAt(line); e != nil && m.runBlocked() == "" {
			at.kind, at.id = hoverEdit, e.ID
		} else if e := m.foldAt(line); e != nil {
			at.kind, at.id = hoverFold, e.ID
		} else if line < len(m.lines) && x < ansi.StringWidth(m.lines[line]) && !decoration(m.lines[line]) {
			at.kind = hoverText
		}
		return at
	case y > m.statusRow() && y < m.ruleRow():
		if item, _, ok := m.queueRowAt(y); ok {
			return hoverTarget{kind: hoverQueue, row: y, id: item.ID, level: -1}
		}
		if label, ok := m.stripAt(x, y); ok {
			return hoverTarget{kind: hoverAttachment, row: y, id: label, level: -1}
		}
		return none
	case y == m.controlsRow():
		for _, c := range m.controlAt(x) {
			at := hoverTarget{kind: c.kind, row: y, level: -1, from: c.from, to: c.to}
			if c.kind == hoverEffort {
				if i := x - c.meter; i >= 0 && i < len(cockpit.ThinkingLevels) {
					at.level = i
				}
			}
			return at
		}
		return none
	case y >= m.inputTop() && y < m.inputTop()+m.input.Height():
		return hoverTarget{kind: hoverComposer, row: y, level: -1}
	}
	return none
}

// pointerShape is the pointer the terminal should show, by CSS name.
func (m *uiModel) pointerShape(h hoverTarget) string {
	switch {
	case m.press != nil && m.scrubbing:
		return "grabbing"
	case m.press != nil && m.sel != nil:
		return "text" // selecting
	}
	switch h.kind {
	case hoverNone:
		return "default"
	case hoverText, hoverComposer:
		return "text"
	case hoverPanelEdge:
		return "ew-resize"
	}
	return "pointer"
}

// hoverHint says what a click would do.
func (m *uiModel) hoverHint(h hoverTarget) string {
	switch h.kind {
	case hoverEdit:
		return "click edits this prompt and runs it again from here"
	case hoverFold:
		if e := m.tr.Entry(h.id); e != nil && m.isOpen(e) {
			return "click folds it away"
		}
		return "click shows it in full"
	case hoverStarter:
		return "click puts this task in the composer"
	case hoverQueue:
		return "click selects it — enter edits it, ctrl+x forces it in, backspace drops it"
	case hoverAttachment:
		return "click shows " + h.id + " large — the cursor goes right after its label, where ⌫ removes it"
	case hoverPreview:
		return "the image goes to the model with the prompt — moving the cursor off its label closes this"
	case hoverEffort:
		if h.level >= 0 {
			return "click sets the effort to " + cockpit.ThinkingLevels[h.level]
		}
		return "click moves to the next effort level"
	case hoverModel:
		return "click lists the models the next prompt can run with, of every provider · ctrl+p"
	case hoverRun:
		switch {
		case m.state == stopping:
			return "the run is stopping"
		case m.state != idle && strings.TrimSpace(m.input.Value()) != "":
			return "click queues the prompt for when the agent finishes · force sends it in after its running tools"
		case m.state != idle:
			return "click stops the run · esc esc"
		case m.edit != nil:
			return "click runs the edited prompt from here"
		case strings.TrimSpace(m.input.Value()) == "":
			return "write a prompt, then run it — enter does too"
		}
		return "click runs the prompt · enter"
	case hoverScrollbar:
		return "click or drag to scroll"
	case hoverChangeRow:
		if h.folder {
			return "click folds or unfolds the folder"
		}
		return "click shows the file's diff"
	case hoverPanelButton:
		if h.id == "follow" {
			return "click follows the file the run changes"
		}
		return "click closes the changes · ctrl+g"
	case hoverPanelEdge:
		if m.resizing {
			pw, _ := m.panelColumns()
			return fmt.Sprintf("the changes take %d columns · letting go keeps them", pw)
		}
		return "drag to resize the changes · a double click gives them their width back · < > in the panel"
	}
	return ""
}

// paintHover lights up what a click on a transcript row would act on.
func (m *uiModel) paintHover(h hoverTarget, row int, line string) string {
	if h.row != row {
		return line
	}
	st := m.styles
	switch h.kind {
	case hoverEdit:
		line = m.lightUp(line, 0, m.view.Width)
		label := ansi.StringWidth("✎ edit")
		return m.paint(line, m.view.Width-label, m.view.Width, st.hoverAction)
	case hoverFold:
		return m.lightUp(line, 0, m.view.Width)
	case hoverStarter:
		return m.lightUp(line, h.from, h.to)
	}
	return line
}

// paintControls lights up the control under the pointer on the controls
// row: the model, the effort and the bar a click picks, or the button.
func (m *uiModel) paintControls(h hoverTarget, row string) string {
	switch h.kind {
	case hoverModel, hoverRun:
		return m.lightUp(row, h.from, h.to)
	case hoverEffort:
		row = m.lightUp(row, h.from, h.to)
		if h.level >= 0 {
			for _, c := range m.controlAt(h.from) {
				if c.kind == hoverEffort {
					at := c.meter + h.level
					row = m.paint(row, at, at+1, m.styles.hoverAction)
				}
			}
		}
	}
	return row
}

// lightUp puts the hover background under columns [from, to) of a rendered
// line and keeps its colours. Without colours it underlines them.
func (m *uiModel) lightUp(line string, from, to int) string {
	on, _, _ := strings.Cut(m.styles.hoverBackground.Render("x"), "x")
	to = min(to, ansi.StringWidth(line))
	if on == "" {
		return m.paint(line, from, to, m.styles.hover)
	}
	if from >= to || from < 0 {
		return line
	}
	// The middle part starts with every style sequence before it; the
	// background comes back after each reset in it.
	middle := ansi.Cut(line, from, to)
	middle = strings.NewReplacer("\x1b[0m", "\x1b[0m"+on, "\x1b[m", "\x1b[m"+on).Replace(middle)
	return ansi.Truncate(line, from, "") + on + middle + "\x1b[m" + ansi.TruncateLeft(line, to, "")
}

// paint restyles columns [from, to) of a rendered line, keeping its text.
// The part after keeps every style sequence of the line, so it looks as it
// did.
func (m *uiModel) paint(line string, from, to int, style lipgloss.Style) string {
	to = min(to, ansi.StringWidth(line))
	if from >= to || from < 0 {
		return line
	}
	return ansi.Truncate(line, from, "") + "\x1b[m" + style.Render(ansi.Strip(ansi.Cut(line, from, to))) + ansi.TruncateLeft(line, to, "")
}

// ---------------------------------------------------------------- scrollbar

// onScrollbar reports whether a cell is on the transcript's scrollbar,
// which shows when the transcript is longer than its room.
func (m *uiModel) onScrollbar(x, y int) bool {
	return x >= m.view.Width && x < m.view.Width+margin+2 && !m.inPanel(x, y) &&
		y >= m.top() && y < m.top()+m.view.Height && m.view.TotalLineCount() > m.view.Height
}

// scrub scrolls the transcript to where the scrollbar was clicked or
// dragged to.
func (m *uiModel) scrub(y int) {
	total, height := m.view.TotalLineCount(), m.view.Height
	if total <= height {
		return
	}
	row := clamp(y-m.top(), 0, height-1)
	m.view.SetYOffset((total - height) * row / max(1, height-1))
	m.userScrolled()
}

// ---------------------------------------------------------------- composer

// placeCursor moves the composer's cursor to the character clicked.
func (m *uiModel) placeCursor(x, y int) {
	value := m.input.Value()
	if value == "" {
		return
	}
	rows, first, ok := m.composerRows()
	if !ok || len(rows) == 0 {
		return
	}
	r := rows[clamp(first+y-m.inputTop(), 0, len(rows)-1)]
	col := max(0, x-promptWidth)
	at, _ := runeSpan(r.runes, col, col+1)
	line := []rune(strings.Split(value, "\n")[r.line])
	// The textarea moves between lines only by rows, up or down.
	for guard := 0; m.input.Line() > r.line && guard < 100_000; guard++ {
		m.input.CursorUp()
	}
	for guard := 0; m.input.Line() < r.line && guard < 100_000; guard++ {
		m.input.CursorDown()
	}
	m.input.SetCursor(min(r.start+at, len(line)))
}
