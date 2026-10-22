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

// setEffort chooses the effort of the session's next runs, and of new
// sessions here and in the web cockpit, which reads it from the preferences
// beside the settings. Other sessions keep theirs.
func (m *uiModel) setEffort(level string) tea.Cmd {
	m.thinking, m.lastEffort, m.ownEffort, m.prefs.Effort = level, level, true, level
	text, kind := "effort: "+level, "info"
	// A session that has started keeps it, for the web cockpit to show.
	if !m.fresh || m.state != idle {
		if err := cockpit.SetSessionEffort(m.opt.SessionDir, m.sessionID, level); err != nil {
			text, kind = text+" — not kept with the session: "+err.Error(), "warn"
		} else {
			m.meta.Effort = level
		}
	}
	if m.opt.preferences == "" {
		return m.notify(text, kind)
	}
	if err := cockpit.SaveEffort(m.opt.preferences, level); err != nil {
		return m.notify("effort: "+level+", for new sessions here only — "+err.Error(), "warn")
	}
	m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	return m.notify(text, kind)
}

// watchPreferences looks for a choice made elsewhere every few seconds; the
// check is a stat of a small file.
func watchPreferences() tea.Cmd {
	return tea.Tick(2*time.Second, func(time.Time) tea.Msg { return prefsMsg{} })
}

// readPreferences takes up the effort and the model a composer of the web
// cockpit or another terminal chose, for new sessions. It reports whether
// the effort is another.
func (m *uiModel) readPreferences() (effort bool) {
	if m.opt.preferences == "" {
		return false
	}
	info, changed := cockpit.PreferencesChanged(m.opt.preferences, m.prefsSeen)
	if !changed {
		return false
	}
	m.prefsSeen = info
	p := cockpit.LoadPreferences(m.opt.preferences)
	// Only what changed: -model and -thinking-level hold until a choice.
	if p.LastModel != "" && p.LastModel != m.prefs.LastModel {
		m.lastModel = p.LastModel
	}
	if p.Effort != "" && p.Effort != m.prefs.Effort {
		m.lastEffort, effort = p.Effort, true
	}
	// The look chosen there is this window's too.
	if applyAppearance(p.Appearance()) {
		m.restyle()
	}
	m.prefs = p
	return effort
}

// preferencesChanged takes up a choice made in the web cockpit or another
// terminal: new sessions start with it, and so does the session in view
// while it is new and nothing was chosen in it. Every other session keeps
// its own effort and model.
func (m *uiModel) preferencesChanged() tea.Cmd {
	if !m.readPreferences() || m.ownEffort || m.state != idle || m.loading || m.thinking == m.lastEffort {
		return nil
	}
	m.thinking = m.lastEffort
	return m.notify("effort of the new session: "+m.thinking+", as chosen in another window", "info")
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

// effortControl renders the effort meter of the composer's controls, and
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
			meter += st.rule2.Render(bar)
		}
	}
	lead := st.label.Render("EFFORT") + " "
	return lead + meter + " " + st.muted.Render(strings.ToUpper(m.thinking)), ansi.StringWidth(lead)
}

// clickEffort picks the level of the bar clicked; a click elsewhere on the
// control moves to the next level.
func (m *uiModel) clickEffort(x int) tea.Cmd {
	for _, c := range m.controlAt(x) {
		if c.kind != hoverEffort {
			continue
		}
		if i := x - c.meter; i >= 0 && i < len(cockpit.ThinkingLevels) {
			return m.setEffort(cockpit.ThinkingLevels[i])
		}
		return m.setEffort(nextLevel(m.thinking))
	}
	return nil
}
