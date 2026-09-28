package main

import (
	"strings"

	tea "github.com/charmbracelet/bubbletea"
)

// lineView is the transcript's window onto its lines: what the cockpit used
// of bubbles' viewport, without what that cost. The viewport takes its
// content as one string, splits it into lines again and measures each of
// them whenever it is set, and pads and wraps what it shows at every frame;
// a long transcript paid for all of it at every refresh. The lines here are
// kept as rendered, and the rows in view are taken from them as they are.
type lineView struct {
	Width, Height int
	// YOffset is the first line in view.
	YOffset int
	lines   []string
}

// wheelLines is how many lines a turn of the wheel scrolls.
const wheelLines = 3

// SetLines gives the view its lines; there is always one, if empty.
func (v *lineView) SetLines(lines []string) {
	if len(lines) == 0 {
		lines = []string{""}
	}
	v.lines = lines
	if v.YOffset > len(lines)-1 {
		v.GotoBottom()
	}
}

// SetContent gives the view text, a line of it a line of the view.
func (v *lineView) SetContent(s string) {
	v.SetLines(strings.Split(strings.ReplaceAll(s, "\r\n", "\n"), "\n"))
}

// TotalLineCount is how many lines the view has, in view or not.
func (v lineView) TotalLineCount() int { return len(v.lines) }

func (v lineView) maxYOffset() int { return max(0, len(v.lines)-v.Height) }

func (v lineView) AtTop() bool    { return v.YOffset <= 0 }
func (v lineView) AtBottom() bool { return v.YOffset >= v.maxYOffset() }

func (v *lineView) SetYOffset(n int) { v.YOffset = clamp(n, 0, v.maxYOffset()) }
func (v *lineView) GotoTop()         { v.SetYOffset(0) }
func (v *lineView) GotoBottom()      { v.SetYOffset(v.maxYOffset()) }

// ScrollDown and ScrollUp move the view by n lines, as far as there are.
func (v *lineView) ScrollDown(n int) {
	if !v.AtBottom() && n > 0 {
		v.SetYOffset(v.YOffset + n)
	}
}

func (v *lineView) ScrollUp(n int) {
	if !v.AtTop() && n > 0 {
		v.SetYOffset(v.YOffset - n)
	}
}

func (v *lineView) HalfPageDown() { v.ScrollDown(v.Height / 2) }
func (v *lineView) HalfPageUp()   { v.ScrollUp(v.Height / 2) }

// Visible is the lines in view, at most Height of them, as they were
// rendered: they are not padded to the width.
func (v lineView) Visible() []string {
	top := clamp(v.YOffset, 0, len(v.lines))
	return v.lines[top:min(len(v.lines), top+v.Height)]
}

// View is the lines in view, one a row.
func (v lineView) View() string { return strings.Join(v.Visible(), "\n") }

// Update scrolls the view by the wheel.
func (v lineView) Update(msg tea.Msg) (lineView, tea.Cmd) {
	if msg, ok := msg.(tea.MouseMsg); ok && msg.Action == tea.MouseActionPress && !msg.Shift {
		switch msg.Button {
		case tea.MouseButtonWheelUp:
			v.ScrollUp(wheelLines)
		case tea.MouseButtonWheelDown:
			v.ScrollDown(wheelLines)
		}
	}
	return v, nil
}
