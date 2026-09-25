package main

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
)

// unknownCSISequenceMsg has the name and shape of the message Bubble Tea
// sends for control sequences it does not recognize.
type unknownCSISequenceMsg []byte

func TestComposerLines(t *testing.T) {
	m := testModel(t)
	if m.input.Height() != minInputs {
		t.Fatalf("empty composer is %d rows", m.input.Height())
	}
	typeText(m, "one")
	for range 12 {
		m.Update(key("ctrl+j"))
	}
	if m.input.LineCount() != 13 {
		t.Fatalf("lines = %d", m.input.LineCount())
	}
	if m.input.Height() != maxInputs || m.view.Height < 1 {
		t.Fatalf("composer %d rows, transcript %d", m.input.Height(), m.view.Height)
	}
	if rule := ansi.Strip(m.composerRule()); !strings.Contains(rule, "line 13/13") {
		t.Fatalf("rule = %q", rule)
	}

	m.input.Reset()
	typeText(m, `first line \`)
	if m.Update(key("enter")); m.input.Value() != "first line \n" || m.state != idle {
		t.Fatalf("backslash enter: %q, state %v", m.input.Value(), m.state)
	}
	typeText(m, "second")
	m.Update(key("alt+enter"))
	m.Update(unknownCSISequenceMsg("\x1b[13;2u")) // shift+enter, as CSI u
	m.Update(unknownCSISequenceMsg("\x1b[27;2;13~"))
	m.Update(unknownCSISequenceMsg("\x1b[99;5u")) // not a line break
	if m.input.Value() != "first line \nsecond\n\n\n" {
		t.Fatalf("line breaks: %q", m.input.Value())
	}

	// Pasted Windows line breaks are one break each, and never run anything.
	m.input.Reset()
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Paste: true, Runes: []rune("alpha\r\nbeta\rgamma\n\tdelta")})
	if m.input.Value() != "alpha\nbeta\ngamma\n    delta" || m.state != idle {
		t.Fatalf("paste: %q", m.input.Value())
	}

	// Small terminals keep a transcript row.
	for _, size := range [][2]int{{24, 8}, {40, 12}} {
		m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		if lines := strings.Count(m.View(), "\n") + 1; lines != size[1] || m.view.Height < 1 {
			t.Fatalf("%dx%d: %d lines, transcript %d", size[0], size[1], lines, m.view.Height)
		}
	}
}

func TestEffortControls(t *testing.T) {
	m := testModel(t)
	for _, step := range []struct{ key, want string }{
		{"alt+up", "xhigh"}, {"alt+up", "max"}, {"alt+up", "max"}, {"alt+down", "xhigh"},
		{"shift+tab", "max"}, {"shift+tab", "low"}, {"ctrl+t", "medium"},
	} {
		m.Update(key(step.key))
		if m.thinking != step.want {
			t.Fatalf("after %s: %q, want %q", step.key, m.thinking, step.want)
		}
	}
	if controls, _ := m.controls(); !strings.Contains(ansi.Strip(controls), "EFFORT") || !strings.Contains(ansi.Strip(controls), "MEDIUM") {
		t.Fatalf("controls = %q", ansi.Strip(controls))
	}
	// Each bar of the meter picks its level; the rest of it moves on.
	row, meter := m.effortAt()
	for i, want := range []string{"low", "medium", "high", "xhigh", "max"} {
		click(m, meter+i, row)
		if m.thinking != want {
			t.Fatalf("bar %d: %q, want %q", i, m.thinking, want)
		}
	}
	click(m, meter-3, row) // the control's label moves on
	click(m, 1, row)       // the box's edge is not the control
	if m.thinking != "low" {
		t.Fatalf("after clicks beside the bars: %q", m.thinking)
	}
	m.command("/effort xhigh")
	m.command("/think high")
	if m.command("/effort huge"); m.thinking != "high" || m.note.level != "warn" {
		t.Fatalf("effort %q, notice %+v", m.thinking, m.note)
	}
}
