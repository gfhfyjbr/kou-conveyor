package main

import (
	"maps"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The transcript is drawn for the stage's width entry by entry, and a long
// one takes longer to draw anew than the pointer takes to move the changes
// panel's edge a column, or the terminal to send its next size while it is
// resized. Drawn anew at every step, it fell further and further behind
// them, and the edge with it. Where drawing it all takes long, a step draws
// anew only what is on screen: the entries in view, from the one at the top
// of the view or up from the bottom, and the lines of the diff in view. The
// rest is drawn a slice at a time once the width rests or the edge is let
// go, between whatever else comes, and what needs the lines as they are
// laid out, a click, the wheel or a key, has them finished first.

const (
	// reflowQuick is the longest drawing everything anew may take for a
	// step to do it at once: less than the pointer leaves between motions.
	reflowQuick = 4 * time.Millisecond
	// reflowSlice is the longest a slice of the rest takes.
	reflowSlice = 4 * time.Millisecond
	// reflowRest is how long the width stays before the rest is drawn.
	reflowRest = 100 * time.Millisecond
)

// reflowState is the transcript being drawn anew for the stage's width.
type reflowState struct {
	// pending is set while the view keeps the lines drawn for another
	// width than the stage gives the transcript, width: what is on screen
	// is drawn for it at each frame, the entries into next, which the
	// slices fill.
	pending bool
	width   int
	next    map[string]block
	gen     int // the rest's and the slices'
	// cost is what drawing every entry anew took, as last measured.
	cost time.Duration
}

// reflowMsg is the width at rest, or the next slice to draw.
type reflowMsg struct{ gen int }

// reflowCheap reports whether the transcript and the diff are drawn anew
// for another width quicker than a step of the width may take.
func (m *uiModel) reflowCheap() bool {
	return m.reflow.cost+m.changes.drawn.cost < reflowQuick
}

// reflowLater fits the stage to the width it was given: the composer and
// the rows at once, the transcript's lines the entries on screen first.
func (m *uiModel) reflowLater() tea.Cmd {
	r := &m.reflow
	r.pending = true
	m.layout() // the view's lines keep their width
	width := m.transcriptColumns()
	if width == m.view.Width {
		// Back at the width the lines were drawn for.
		m.relayout()
		return nil
	}
	if width != r.width || r.next == nil {
		r.width, r.next = width, make(map[string]block)
	}
	r.gen++
	gen := r.gen
	return tea.Tick(reflowRest, func(time.Time) tea.Msg { return reflowMsg{gen} })
}

// reflowNow draws the rest without waiting for the width to rest.
func (m *uiModel) reflowNow() tea.Cmd {
	r := &m.reflow
	if !r.pending {
		return nil
	}
	r.gen++
	return nextSlice(r.gen)
}

func nextSlice(gen int) tea.Cmd { return func() tea.Msg { return reflowMsg{gen} } }

// reflowed draws a slice of the entries not yet drawn for the width, and
// once none is left lays the transcript out with them.
func (m *uiModel) reflowed(msg reflowMsg) tea.Cmd {
	r := &m.reflow
	if msg.gen != r.gen || !r.pending {
		return nil
	}
	start := time.Now()
	index, fading := 0, false
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser {
			index++
		}
		if _, ok := m.cachedBlock(r.next, e, r.width, fading); !ok {
			m.drawBlock(r.next, e, r.width, index, fading)
			if time.Since(start) >= reflowSlice {
				return nextSlice(r.gen)
			}
		}
		if m.edit != nil && m.edit.id == e.ID {
			fading = true
		}
	}
	m.relayout()
	return nil
}

// settleReflow ends the drawing anew: what was drawn for the width the
// stage gives the transcript is the transcript's, and what is left is
// drawn with the next render.
func (m *uiModel) settleReflow() {
	r := &m.reflow
	if !r.pending {
		return
	}
	if r.width == m.transcriptColumns() {
		maps.Copy(m.cache, r.next)
	}
	r.pending, r.next = false, nil
	r.gen++
}

// dropReflow forgets the drawing anew, for a transcript drawn from the
// start.
func (m *uiModel) dropReflow() {
	r := &m.reflow
	if r.pending {
		r.pending, r.next = false, nil
		r.gen++
		m.layout()
	}
}

// needsLayout reports a message that acts on the transcript or the diff as
// they are laid out, which the rest of their lines must be drawn for first:
// a key, a click, the wheel, a selection's drag. The panel's edge goes on
// without, dragged or moved by its keys, and so does the pointer passing
// over: nothing lights up under it until the rest is drawn.
func (m *uiModel) needsLayout(msg tea.Msg) bool {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		stepsEdge := m.picker == nil && m.form == nil && m.queueFocus < 0 && m.changes.focused &&
			m.changesShown() && panelStepKey(msg.String())
		return !stepsEdge
	case tea.MouseMsg:
		switch {
		case m.resizing:
			return false
		case msg.Action == tea.MouseActionMotion:
			return m.press != nil
		case msg.Action == tea.MouseActionPress && msg.Button == tea.MouseButtonLeft:
			return !m.onPanelEdge(msg.X, msg.Y)
		}
		return true
	}
	return false
}

// preview is the transcript's rows while the view keeps lines drawn for
// another width: the entries on screen drawn for the stage's, from the
// entry at the top of the view, or up from the bottom where the view is at
// the bottom, as the lines will be laid out once the rest is drawn.
func (m *uiModel) preview() []string {
	r := &m.reflow
	entries := m.tr.Entries
	if len(entries) == 0 {
		return m.view.Visible()
	}
	// The prompts' numbers and what fades, as render has them.
	indexes := make([]int, len(entries))
	faded := make([]bool, len(entries))
	index, fading := 0, false
	for i, e := range entries {
		if e.Kind == cockpit.KindUser {
			index++
		}
		indexes[i], faded[i] = index, fading
		if m.edit != nil && m.edit.id == e.ID {
			fading = true
		}
	}
	air := func(i int) bool { return i > 0 && !(dense(entries[i-1]) && dense(entries[i])) }
	draw := func(i int) []string {
		body := m.drawBlock(r.next, entries[i], r.width, indexes[i], faded[i])
		if air(i) {
			return append([]string{m.timeline(entries[i])}, body...)
		}
		return body
	}
	first, into := -1, 0
	if !m.view.AtBottom() && !m.follow {
		for i, s := range m.spans {
			if s.end <= m.view.YOffset {
				continue
			}
			if i < len(entries) && entries[i].ID == s.id {
				first, into = i, max(0, m.view.YOffset-s.start)
				if air(i) {
					into++ // the row of air above it is above the view
				}
			}
			break
		}
	}
	return window(len(entries), first, into, m.view.Height, draw)
}

// window is the rows a view of height shows of blocks drawn one at a time:
// into rows into block first and on, or, where first is -1 or the blocks
// end before the view does, the last rows.
func window(blocks, first, into, height int, draw func(int) []string) []string {
	if first >= 0 {
		var rows []string
		for i := first; i < blocks && len(rows) < into+height; i++ {
			block := draw(i)
			if i == first {
				into = clamp(into, 0, max(0, len(block)-1))
			}
			rows = append(rows, block...)
		}
		if len(rows) >= into+height {
			return rows[into : into+height]
		}
	}
	var parts [][]string
	total := 0
	for i := blocks - 1; i >= 0 && total < height; i-- {
		block := draw(i)
		parts = append(parts, block)
		total += len(block)
	}
	rows := make([]string, 0, total)
	for i := len(parts) - 1; i >= 0; i-- {
		rows = append(rows, parts[i]...)
	}
	return rows[max(0, len(rows)-height):]
}
