package main

import (
	"math"
	"strings"
	"time"
	"unicode"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
)

// Dragging with the left button selects text, and letting go copies it. A
// selection that starts in the composer stays in the composer and copies the
// text as typed; one in the transcript copies what the screen shows, without
// the gutter of timestamps and rails when it starts past it.

type selArea int

const (
	inTranscript selArea = iota + 1
	inComposer
	inDiff // the diff in the changes panel
)

// cell is a position: on the screen, or in an area of it.
type cell struct{ row, col int }

type selection struct {
	area selArea
	// Transcript: rendered line and column. Composer: visible row and column
	// of its text.
	anchor, head cell
	moved        bool // dragged away from where it started
	// gutter is the columns of the area's gutter, of times or of line
	// numbers: rows after the first leave it out when the selection starts
	// past it.
	gutter int
}

type (
	selectionCopiedMsg struct {
		text string
		err  error
	}
	toastMsg struct{ gen int }
)

// promptWidth is the width of the "❯ " before each composer row.
const promptWidth = 2

// Screen rows below the transcript.
func (m *uiModel) statusRow() int { return transcriptTop + m.view.Height }
func (m *uiModel) ruleRow() int   { return m.statusRow() + 1 + m.queueRows() + m.stripRows() }
func (m *uiModel) inputTop() int  { return m.ruleRow() + 1 }

func clamp(v, lo, hi int) int { return max(lo, min(hi, v)) }

// selectionAt starts a selection where the button went down, if that is in
// text that can be selected.
func (m *uiModel) selectionAt(x, y int) *selection {
	if m.inPanel(x, y) {
		g, _ := m.panelGeometry()
		lines, gutter, note := m.diffView(g.inner)
		if y < g.diffTop || y >= g.diffTop+g.diffRows || note != "" || lines == nil {
			return nil
		}
		at := cell{m.changes.scroll + y - g.diffTop, clamp(x-g.content, 0, g.inner-1)}
		return &selection{area: inDiff, anchor: at, head: at, gutter: gutter}
	}
	switch {
	case y >= transcriptTop && y < transcriptTop+m.view.Height:
		at := cell{m.view.YOffset + y - transcriptTop, clamp(x, 0, m.view.Width-1)}
		return &selection{area: inTranscript, anchor: at, head: at, gutter: gutter}
	case y >= m.inputTop() && y < m.inputTop()+m.input.Height():
		at := cell{y - m.inputTop(), clamp(x-promptWidth, 0, m.input.Width()-1)}
		return &selection{area: inComposer, anchor: at, head: at}
	}
	return nil
}

// drag moves the selection's end to the pointer. The composer's selection
// stops at the composer's edges; the transcript's scrolls when the pointer
// leaves it above or below.
func (m *uiModel) drag(x, y int) {
	s := m.sel
	if s == nil {
		return
	}
	var head cell
	switch s.area {
	case inTranscript:
		row := y - transcriptTop
		switch {
		case row < 0:
			m.view.ScrollUp(1)
			m.userScrolled()
			row = 0
		case row >= m.view.Height:
			m.view.ScrollDown(1)
			m.userScrolled()
			row = m.view.Height - 1
		}
		m.scrolled()
		head = cell{m.view.YOffset + row, clamp(x, 0, m.view.Width-1)}
	case inDiff:
		g, ok := m.panelGeometry()
		if !ok || g.diffRows == 0 {
			return
		}
		row := y - g.diffTop
		switch {
		case row < 0:
			m.scrollDiff(-1)
			row = 0
		case row >= g.diffRows:
			m.scrollDiff(1)
			row = g.diffRows - 1
		}
		head = cell{m.changes.scroll + row, clamp(x-g.content, 0, g.inner-1)}
	case inComposer:
		top, rows := m.inputTop(), m.input.Height()
		switch {
		case y < top:
			head = cell{0, 0}
		case y >= top+rows:
			head = cell{rows - 1, m.input.Width() - 1}
		default:
			head = cell{y - top, clamp(x-promptWidth, 0, m.input.Width()-1)}
		}
	}
	if head != s.anchor {
		s.moved = true
	}
	s.head = head
}

// bounds returns the selection's first and last cell, in reading order.
func (s *selection) bounds() (cell, cell) {
	a, b := s.anchor, s.head
	if b.row < a.row || b.row == a.row && b.col < a.col {
		a, b = b, a
	}
	return a, b
}

// columns returns the columns of a row the selection covers, [from, to).
func (s *selection) columns(row int) (from, to int, ok bool) {
	start, end := s.bounds()
	if row < start.row || row > end.row {
		return 0, 0, false
	}
	// Rows after the first leave out the gutter when the selection started
	// in the text beside it.
	if s.gutter > 0 && start.col >= s.gutter {
		from = s.gutter
	}
	to = math.MaxInt
	if row == start.row {
		from = start.col
	}
	if row == end.row {
		to = end.col + 1
	}
	return from, to, true
}

// selected highlights the selection on one row of an area.
func (m *uiModel) selected(area selArea, row int, line string, offset int) string {
	s := m.sel
	if s == nil || !s.moved || s.area != area {
		return line
	}
	from, to, ok := s.columns(row)
	// A picture is no text, and repainted it would show no picture.
	if !ok || area == inTranscript && m.pictureLine(row) {
		return line
	}
	if to != math.MaxInt {
		to += offset
	}
	return m.paint(line, from+offset, to, m.styles.selection)
}

// selectedText is what a selection copies.
func (m *uiModel) selectedText(s *selection) string {
	start, end := s.bounds()
	if s.area == inComposer {
		return m.composerText(s, start, end)
	}
	source := m.lines
	if s.area == inDiff {
		source = m.changes.drawn.lines
	}
	var lines []string
	for row := start.row; row <= end.row && row < len(source); row++ {
		if s.area == inTranscript && m.pictureLine(row) {
			continue // a picture copies as nothing
		}
		from, to, _ := s.columns(row)
		plain := ansi.Strip(source[row])
		lines = append(lines, strings.TrimRight(ansi.Cut(plain, from, min(to, ansi.StringWidth(plain))), " "))
	}
	return strings.TrimRight(strings.Join(lines, "\n"), "\n")
}

func (m *uiModel) copySelection(s *selection) tea.Cmd {
	text := m.selectedText(s)
	if strings.TrimSpace(text) == "" {
		return nil
	}
	return func() tea.Msg { return selectionCopiedMsg{text: text, err: writeClipboard(text)} }
}

// showToast shows a short message at the bottom of the screen.
func (m *uiModel) showToast(text string) tea.Cmd {
	m.toastGen++
	m.toast = text
	gen := m.toastGen
	return tea.Tick(2500*time.Millisecond, func(time.Time) tea.Msg { return toastMsg{gen} })
}

// ---------------------------------------------------------------- composer text

// wrappedRow is one row of the composer: a part of one line of its text.
type wrappedRow struct {
	line, start int // the line, and the rune of it the row starts with
	runes       []rune
}

// composerText returns the text a selection in the composer covers, as
// typed: rows the composer wrapped join without a line break.
func (m *uiModel) composerText(s *selection, start, end cell) string {
	value := m.input.Value()
	if value == "" {
		return "" // the placeholder is not text
	}
	rows, first, ok := m.composerRows()
	if !ok {
		// The rows could not be matched to the text; copy what they show.
		shown := strings.Split(ansi.Strip(m.input.View()), "\n")
		var lines []string
		for row := start.row; row <= end.row && row < len(shown); row++ {
			from, to, _ := s.columns(row)
			text := ansi.TruncateLeft(shown[row], promptWidth, "")
			lines = append(lines, strings.TrimRight(ansi.Cut(text, from, min(to, ansi.StringWidth(text))), " "))
		}
		return strings.Join(lines, "\n")
	}
	lines := strings.Split(value, "\n")
	var b strings.Builder
	previous := -1
	for row := start.row; row <= end.row; row++ {
		i := first + row
		if i >= len(rows) {
			break
		}
		r := rows[i]
		from, to, _ := s.columns(row)
		a, z := runeSpan(r.runes, from, to)
		text := []rune(lines[r.line])
		lo, hi := min(r.start+a, len(text)), min(r.start+z, len(text))
		if previous >= 0 && r.line != previous {
			b.WriteByte('\n')
		}
		b.WriteString(string(text[lo:hi]))
		previous = r.line
	}
	return b.String()
}

// composerRows wraps the composer's text as the textarea does and finds the
// first row it shows, which it scrolls by itself: the rows on screen must
// read like the wrapped ones from there.
func (m *uiModel) composerRows() (rows []wrappedRow, first int, ok bool) {
	width := m.input.Width()
	for i, line := range strings.Split(m.input.Value(), "\n") {
		start := 0
		for _, part := range wrapLine([]rune(line), width) {
			rows = append(rows, wrappedRow{line: i, start: start, runes: part})
			start += len(part)
		}
	}
	shown := strings.Split(ansi.Strip(m.input.View()), "\n")
	height := m.input.Height()
	matches := func(first int) bool {
		for i := range height {
			got, want := "", ""
			if i < len(shown) {
				got = strings.TrimRight(ansi.TruncateLeft(shown[i], promptWidth, ""), " ")
			}
			if first+i < len(rows) {
				want = strings.TrimRight(string(rows[first+i].runes), " ")
			}
			if got != want {
				return false
			}
		}
		return true
	}
	// Among equal-looking positions, the one that shows the cursor.
	cursor := m.input.LineInfo().RowOffset
	for i := range m.input.Line() {
		cursor += len(wrapLine([]rune(strings.Split(m.input.Value(), "\n")[i]), width))
	}
	found := -1
	for first := 0; first <= len(rows); first++ {
		if !matches(first) {
			continue
		}
		if cursor >= first && cursor < first+height {
			return rows, first, true
		}
		if found < 0 {
			found = first
		}
	}
	return rows, max(0, found), found >= 0
}

// runeSpan returns the runes of a row that start in columns [from, to).
func runeSpan(runes []rune, from, to int) (int, int) {
	a, z := -1, len(runes)
	col := 0
	for i, r := range runes {
		if a < 0 && col >= from {
			a = i
		}
		if col >= to {
			z = i
			break
		}
		col += ansi.StringWidth(string(r))
	}
	if a < 0 {
		a = len(runes)
	}
	return a, max(a, z)
}

// wrapLine wraps one line of the composer's text into rows exactly as the
// textarea does (wrap in github.com/charmbracelet/bubbles/textarea), so that
// positions on the screen map back to the text. Rows keep the spaces they
// end with, and the last one gets one more, as there.
func wrapLine(runes []rune, width int) [][]rune {
	var (
		lines  = [][]rune{{}}
		word   = []rune{}
		row    int
		spaces int
	)
	for _, r := range runes {
		if unicode.IsSpace(r) {
			spaces++
		} else {
			word = append(word, r)
		}
		if spaces > 0 {
			if ansi.StringWidth(string(lines[row]))+ansi.StringWidth(string(word))+spaces > width {
				row++
				lines = append(lines, []rune{})
			}
			lines[row] = append(lines[row], word...)
			lines[row] = append(lines[row], []rune(strings.Repeat(" ", spaces))...)
			spaces = 0
			word = nil
		} else {
			// A word as wide as the row goes on a row of its own.
			last := ansi.StringWidthWc(string(word[len(word)-1]))
			if ansi.StringWidth(string(word))+last > width {
				if len(lines[row]) > 0 {
					row++
					lines = append(lines, []rune{})
				}
				lines[row] = append(lines[row], word...)
				word = nil
			}
		}
	}
	if ansi.StringWidth(string(lines[row]))+ansi.StringWidth(string(word))+spaces >= width {
		lines = append(lines, []rune{})
		lines[row+1] = append(lines[row+1], word...)
		spaces++
		lines[row+1] = append(lines[row+1], []rune(strings.Repeat(" ", spaces))...)
	} else {
		lines[row] = append(lines[row], word...)
		spaces++
		lines[row] = append(lines[row], []rune(strings.Repeat(" ", spaces))...)
	}
	return lines
}
