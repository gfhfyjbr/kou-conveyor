package main

import (
	"reflect"
	"slices"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// ---------------------------------------------------------------- lines

// newline breaks the composer's line at the cursor.
func (m *uiModel) newline() {
	m.input.InsertString("\n")
	m.resize()
}

// backslashBeforeCursor reports a backslash right before the cursor: enter
// then breaks the line instead of running the prompt, which works in
// terminals that cannot tell shift+enter from enter.
func (m *uiModel) backslashBeforeCursor() bool {
	lines := strings.Split(m.input.Value(), "\n")
	row := m.input.Line()
	if row < 0 || row >= len(lines) {
		return false
	}
	info := m.input.LineInfo()
	col := info.StartColumn + info.ColumnOffset
	runes := []rune(lines[row])
	return col > 0 && col <= len(runes) && runes[col-1] == '\\'
}

// paste inserts pasted text as it was copied. The textarea would read the
// CR and LF of a Windows line break as two breaks, so line breaks are made
// uniform first.
func (m *uiModel) paste(text string) {
	text = strings.NewReplacer("\r\n", "\n", "\r", "\n").Replace(text)
	m.input.InsertString(text)
	m.resize()
}

// shiftEnter lists how terminals that report modifiers send shift+enter:
// CSI u (kitty, iTerm2, WezTerm, foot and others with the option on) and
// xterm's modifyOtherKeys. Terminals configured to send ESC CR arrive as
// alt+enter instead.
var shiftEnter = []string{"\x1b[13;2u", "\x1b[27;2;13~", "\x1b[13;2~"}

// csi handles escape sequences Bubble Tea does not know.
func (m *uiModel) csi(seq string) tea.Cmd {
	switch {
	case m.picker != nil || m.form != nil:
	case slices.Contains(shiftEnter, seq):
		m.newline()
	case slices.Contains(forceKeys, seq):
		return m.forceComposer()
	}
	return nil
}

// csiSequence returns the bytes of a control sequence Bubble Tea reported
// without recognizing it. The message type is unexported, so it is known by
// its name and shape.
func csiSequence(msg tea.Msg) (string, bool) {
	v := reflect.ValueOf(msg)
	if !v.IsValid() || v.Kind() != reflect.Slice || v.Type().Elem().Kind() != reflect.Uint8 || v.Type().Name() != "unknownCSISequenceMsg" {
		return "", false
	}
	return string(v.Bytes()), true
}

// ---------------------------------------------------------------- effort

// setEffort chooses the effort for the next runs, here and in the web
// cockpit, which reads it from the preferences beside the settings.
func (m *uiModel) setEffort(level string) tea.Cmd {
	m.thinking = level
	if m.opt.preferences == "" {
		return m.notify("effort: "+level, "info")
	}
	if err := cockpit.SaveEffort(m.opt.preferences, level); err != nil {
		return m.notify("effort: "+level+" here only — "+err.Error(), "warn")
	}
	m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	return m.notify("effort: "+level, "info")
}

// watchPreferences looks for an effort chosen elsewhere every few seconds;
// the check is a stat of a small file.
func watchPreferences() tea.Cmd {
	return tea.Tick(2*time.Second, func(time.Time) tea.Msg { return prefsMsg{} })
}

// preferencesChanged takes up an effort chosen in the web cockpit or
// another terminal.
func (m *uiModel) preferencesChanged() tea.Cmd {
	if m.opt.preferences == "" {
		return nil
	}
	info, changed := cockpit.PreferencesChanged(m.opt.preferences, m.prefsSeen)
	if !changed {
		return nil
	}
	m.prefsSeen = info
	effort := cockpit.LoadPreferences(m.opt.preferences).Effort
	if effort == "" || effort == m.thinking {
		return nil
	}
	m.thinking = effort
	return m.notify("effort: "+effort+", as chosen in another window", "info")
}

// stepEffort raises or lowers the effort by one level, stopping at the ends.
func (m *uiModel) stepEffort(step int) tea.Cmd {
	levels := cockpit.ThinkingLevels
	at := slices.Index(levels, m.thinking)
	if at < 0 {
		at = slices.Index(levels, "high")
	}
	next := max(0, min(len(levels)-1, at+step))
	if next == at {
		end := "highest"
		if step < 0 {
			end = "lowest"
		}
		return m.notify("effort: "+m.thinking+" is the "+end+" level", "info")
	}
	return m.setEffort(levels[next])
}

// effortControl renders the effort meter that ends the composer rule, and
// the column of its first bar within it.
func (m *uiModel) effortControl() (string, int) {
	st := m.styles
	bars := []string{"▁", "▃", "▄", "▆", "█"}
	level := max(0, slices.Index(cockpit.ThinkingLevels, m.thinking))
	meter := ""
	for i := range cockpit.ThinkingLevels {
		bar := bars[min(i, len(bars)-1)]
		switch {
		case i <= level:
			meter += st.accent.Render(bar)
		case st.noColor:
			meter += "·"
		default:
			meter += st.rule.Render(bar)
		}
	}
	// The rule after the name makes up for shorter names, so the bars stay
	// where they are whatever the level.
	name := strings.ToUpper(m.thinking)
	longest := 0
	for _, level := range cockpit.ThinkingLevels {
		longest = max(longest, len(level))
	}
	lead := " " + st.label.Render("EFFORT") + " "
	tail := st.rule.Render(" " + strings.Repeat("─", 2+longest-len(name)))
	return lead + meter + " " + st.muted.Render(name) + tail, ansi.StringWidth(lead)
}

// clickEffort picks the level of the bar clicked; a click elsewhere on the
// control moves to the next level.
func (m *uiModel) clickEffort(x int) tea.Cmd {
	control, meter := m.effortControl()
	start := m.width - ansi.StringWidth(control)
	if x < start {
		return nil
	}
	if i := x - start - meter; i >= 0 && i < len(cockpit.ThinkingLevels) {
		return m.setEffort(cockpit.ThinkingLevels[i])
	}
	return m.setEffort(nextLevel(m.thinking))
}
