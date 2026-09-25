package main

import (
	"io"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func move(m *uiModel, x, y int) {
	m.Update(tea.MouseMsg{Action: tea.MouseActionMotion, Button: tea.MouseButtonNone, X: x, Y: y})
}

func TestThePointerShowsWhatAClickDoes(t *testing.T) {
	m := testModel(t)
	if strings.Contains(m.View(), "\x1b]22;") {
		t.Fatal("set the pointer before it came by")
	}
	runPrompt(t, m, "use a tool")
	m.view.GotoTop()
	status := func() string { return ansi.Strip(strings.Split(m.View(), "\n")[m.statusRow()]) }

	header := headerRow(t, m, "use a tool")
	for _, c := range []struct {
		x, y  int
		kind  hoverKind
		shape string
		hint  string
	}{
		{30, header, hoverEdit, "pointer", "click edits this prompt"},
		{10, header + 2, hoverText, "text", ""},
		{90, header + 2, hoverNone, "default", ""},
		{10, header + 1, hoverNone, "default", ""}, // the prompt's card, not its text
		{30, find(t, m, "BASH").row, hoverFold, "pointer", "click shows it in full"},
		{5, m.inputTop() + 1, hoverComposer, "text", ""},
	} {
		move(m, c.x, c.y)
		h := m.hover()
		if h.kind != c.kind || m.pointerShape(h) != c.shape || !strings.Contains(m.View(), ansi.SetPointerShape(c.shape)) {
			t.Fatalf("at %d,%d: hover %+v, shape %q", c.x, c.y, h, m.pointerShape(h))
		}
		if c.hint != "" && !strings.Contains(status(), c.hint) {
			t.Fatalf("at %d,%d: status %q", c.x, c.y, status())
		}
	}

	// The effort control names the level each bar picks.
	row, meter := m.effortAt()
	move(m, meter+4, row)
	if !strings.Contains(status(), "click sets the effort to max") {
		t.Fatalf("status = %q", status())
	}

	// Selecting shows the I-beam wherever the pointer goes.
	at := find(t, m, "echo: use a tool")
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: at.col, Y: at.row})
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: 5, Y: m.inputTop()})
	if shape := m.pointerShape(m.hover()); shape != "text" {
		t.Fatalf("selecting: %q", shape)
	}
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: 5, Y: m.inputTop()})

	// A window without focus has no pointer over it.
	m.Update(tea.BlurMsg{})
	if m.hover().kind != hoverNone || strings.Contains(m.View(), "\x1b]22;") {
		t.Fatal("the pointer stayed after the window lost focus")
	}

	// Lists follow the pointer.
	m.openPicker("palette")
	m.View()
	move(m, 20, m.picker.listTop+2)
	if m.picker.cursor != 2 || m.hover().kind != hoverPicker {
		t.Fatalf("list cursor %d, hover %+v", m.picker.cursor, m.hover())
	}
}

func TestClicksPlaceTheCursorAndFoldBlocks(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "use a tool")
	m.input.SetValue("first line\nsecond line here")
	m.resize()
	m.View()
	click(m, promptWidth+7, m.inputTop()+1)
	typeText(m, "X")
	if m.input.Value() != "first line\nsecond Xline here" {
		t.Fatalf("typed at the click: %q", m.input.Value())
	}
	click(m, promptWidth+50, m.inputTop())
	typeText(m, "!")
	if m.input.Value() != "first line!\nsecond Xline here" {
		t.Fatalf("typed past the end of a line: %q", m.input.Value())
	}
	// A wrapped line takes the click on its second row.
	m.input.SetValue(strings.Repeat("abcd ", 30) + "\nnext")
	m.resize()
	m.View()
	rows, _, _ := m.composerRows()
	click(m, promptWidth+2, m.inputTop()+1)
	typeText(m, "#")
	if want := len(rows[0].runes) + 2; []rune(m.input.Value())[want] != '#' {
		t.Fatalf("typed at %d? %q", want, m.input.Value())
	}

	// An open block folds from its first line; its text is for selecting.
	m.view.GotoTop()
	tool := find(t, m, "BASH")
	click(m, 30, tool.row)
	if !m.expanded["tool:call-1"] {
		t.Fatal("the tool did not open")
	}
	output := find(t, m, "OUTPUT ·")
	click(m, 20, output.row)
	if !m.expanded["tool:call-1"] {
		t.Fatal("a click in the output folded the tool")
	}
	click(m, 30, find(t, m, "BASH").row)
	if m.expanded["tool:call-1"] {
		t.Fatal("the header did not fold the tool")
	}
}

func TestTheScrollbarScrolls(t *testing.T) {
	m := testModel(t)
	for range 8 {
		runPrompt(t, m, "use a tool")
	}
	total, height := m.view.TotalLineCount(), m.view.Height
	if total <= height {
		t.Fatalf("%d lines fit in %d", total, height)
	}
	bar := m.width - 1
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: bar, Y: m.top()})
	if m.view.YOffset != 0 || m.pointerShape(m.hover()) != "grabbing" {
		t.Fatalf("offset %d at the top", m.view.YOffset)
	}
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: bar - 30, Y: m.top() + height})
	if m.view.YOffset != total-height {
		t.Fatalf("offset %d at the bottom, want %d", m.view.YOffset, total-height)
	}
	_, cmd := m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: bar - 30, Y: m.top() + height})
	if cmd != nil || m.scrubbing || m.press != nil {
		t.Fatal("letting go of the scrollbar did more than stop scrolling")
	}
}

func TestEffortIsSharedWithTheWebCockpit(t *testing.T) {
	dir := t.TempDir()
	settings := filepath.Join(dir, "config", "settings.json")
	prefs := cockpit.PreferencesPath(settings)
	if err := cockpit.SaveEffort(prefs, "xhigh"); err != nil {
		t.Fatal(err)
	}
	for args, want := range map[string]string{
		"":                    "xhigh",
		"-thinking-level low": "low",
	} {
		o, err := parseOptions(append([]string{"-workspace", dir, "-config", settings}, strings.Fields(args)...), io.Discard)
		if err != nil || o.thinking != want || o.preferences != prefs {
			t.Fatalf("%q: effort %q, preferences %q, %v", args, o.thinking, o.preferences, err)
		}
	}

	m := testModel(t)
	m.opt.preferences = prefs
	m.Init()
	m.Update(key("alt+up"))
	m.Update(key("alt+up"))
	if m.thinking != "max" {
		t.Fatalf("effort = %q", m.thinking)
	}
	if saved := cockpit.LoadPreferences(prefs).Effort; saved != m.thinking {
		t.Fatalf("saved %q, shown %q", saved, m.thinking)
	}
	// Its own choice is no news.
	m.note = notice{}
	if m.Update(prefsMsg{}); m.note.text != "" {
		t.Fatalf("notice = %q", m.note.text)
	}
	// The web cockpit chooses another level.
	if err := cockpit.SaveEffort(prefs, "low"); err != nil {
		t.Fatal(err)
	}
	m.Update(prefsMsg{})
	if m.thinking != "low" || !strings.Contains(m.note.text, "another window") {
		t.Fatalf("effort %q, notice %q", m.thinking, m.note.text)
	}
	if controls, _ := m.controls(); !strings.Contains(ansi.Strip(controls), "LOW") {
		t.Fatalf("controls = %q", ansi.Strip(controls))
	}
}
