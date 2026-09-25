package main

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

// settle lets a key held back in case it starts a cut mouse report go on,
// as the timer would, and returns what it did.
func settle(m *uiModel) tea.Cmd {
	if m.debris.held == nil {
		return nil
	}
	_, cmd := m.Update(releaseKeyMsg{m.debris.gen})
	return cmd
}

// press delivers a key and lets it go through if it was held back.
func press(m *uiModel, msg tea.KeyMsg) tea.Cmd {
	_, cmd := m.Update(msg)
	if m.debris.held != nil {
		return settle(m)
	}
	return cmd
}

func runes(s string) tea.KeyMsg { return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)} }

// A mouse report cut between two reads reaches the model as keys: never
// text in the composer, and never an esc.
// The first report of a scroll can be cut too, before any other came.
func TestTheFirstCutReportIsNotKeys(t *testing.T) {
	m := testModel(t)
	for _, piece := range []tea.KeyMsg{{Type: tea.KeyRunes, Runes: []rune("["), Alt: true}, runes("<64"), runes(";16;5M")} {
		m.Update(piece)
	}
	m.Update(key("esc"))
	m.Update(runes("[<64;16;5M"))
	settle(m)
	if m.input.Value() != "" || !m.escArmed.IsZero() {
		t.Fatalf("composer %q, esc armed %v", m.input.Value(), !m.escArmed.IsZero())
	}
}

func TestCutMouseReportsAreNotKeys(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	wheel := func() {
		m.Update(tea.MouseMsg{Button: tea.MouseButtonWheelUp, Action: tea.MouseActionPress, X: 40, Y: 9})
	}
	for name, pieces := range map[string][]tea.KeyMsg{
		"after esc":           {key("esc"), runes("[<64;40;10M")},
		"after esc, in three": {key("esc"), runes("[<64;4"), runes("0;10M")},
		"after alt+[":         {{Type: tea.KeyRunes, Runes: []rune("["), Alt: true}, runes("<64;4"), runes("0;10M")},
		"before the last":     {{Type: tea.KeyRunes, Runes: []rune("["), Alt: true}, runes("<64;40;10"), runes("M")},
		"tail alone":          {runes("4;40;10M")},
		"focus report":        {key("esc"), runes("[I")},
	} {
		for _, piece := range pieces {
			wheel()
			m.Update(piece)
		}
		settle(m)
		if m.input.Value() != "" || m.note.text != "" || !m.escArmed.IsZero() || m.edit != nil {
			t.Fatalf("%s: composer %q, notice %q, esc armed %v", name, m.input.Value(), m.note.text, !m.escArmed.IsZero())
		}
	}

	// Keys of their own still count, a moment later at most.
	wheel()
	m.Update(key("esc"))
	if !m.escArmed.IsZero() {
		t.Fatal("an esc right after the mouse counted before its moment")
	}
	m.Update(runes("x")) // not the rest of a report: the esc goes first
	if !strings.Contains(m.note.text, "esc again") || m.input.Value() != "x" {
		t.Fatalf("notice %q, composer %q", m.note.text, m.input.Value())
	}
	m.input.Reset()
	wheel()
	m.Update(key("esc"))
	m.Update(key("esc"))
	settle(m)
	if m.edit == nil {
		t.Fatal("esc esc right after the mouse did not edit")
	}
	m.Update(key("esc"))
	settle(m)
	// One key at a time, letters, digits and ; type as they are.
	for _, r := range "m5;M[" {
		wheel()
		m.Update(runes(string(r)))
	}
	if m.input.Value() != "m5;M[" {
		t.Fatalf("typed %q", m.input.Value())
	}
	// Compact mode leaves the mouse to the terminal: nothing waits.
	m.input.Reset()
	m.compact = true
	m.Update(key("esc"))
	if m.escArmed.IsZero() || m.debris.held != nil {
		t.Fatal("an esc waited in compact mode")
	}
	m.compact = false
	if strings.Contains(m.View(), "[<64") {
		t.Fatal("a report shows on screen")
	}
}
