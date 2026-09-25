package main

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// click presses and lets go of the left button in one place.
func click(m *uiModel, x, y int) tea.Cmd {
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: x, Y: y})
	_, cmd := m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: x, Y: y})
	return cmd
}

func texts(tr *cockpit.Transcript) string {
	var out []string
	for _, e := range tr.Entries {
		out = append(out, e.Text)
	}
	return strings.Join(out, "|")
}

// headerRow returns the screen row of a prompt's header, scrolled into view.
func headerRow(t *testing.T, m *uiModel, text string) int {
	t.Helper()
	for _, s := range m.spans {
		if e := m.tr.Entry(s.id); e != nil && e.Kind == cockpit.KindUser && e.Text == text {
			m.view.SetYOffset(s.start)
			return m.top() + s.start - m.view.YOffset
		}
	}
	t.Fatalf("no prompt %q", text)
	return 0
}

func TestEscEscEditsTheLastPrompt(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	runPrompt(t, m, "second question")
	m.input.SetValue("a draft")

	if press(m, key("esc")); m.edit != nil || !strings.Contains(m.note.text, "esc again to edit") {
		t.Fatalf("one esc: edit %v, notice %q", m.edit, m.note.text)
	}
	press(m, key("esc"))
	if m.edit == nil || m.input.Value() != "second question" {
		t.Fatalf("esc esc: edit %v, composer %q", m.edit, m.input.Value())
	}
	if rule, status := ansi.Strip(m.composerRule()), ansi.Strip(m.statusLine()); !strings.Contains(rule, "EDIT 02") ||
		!strings.Contains(status, "replacing the 1 entry below") {
		t.Fatalf("rule %q, status %q", rule, status)
	}
	if !strings.Contains(ansi.Strip(strings.Join(m.lines, "\n")), "✎ EDITING") {
		t.Fatal("the prompt being edited is not marked")
	}

	m.input.SetValue("second question, edited")
	_, cmd := m.Update(key("enter"))
	if m.edit != nil || m.input.Value() != "a draft" {
		t.Fatalf("after running the edit: edit %v, composer %q", m.edit, m.input.Value())
	}
	drive(t, m, cmd, func() bool { return m.state == idle })
	want := "first question|echo: first question|second question, edited|echo: second question, edited"
	if got := texts(m.tr); got != want {
		t.Fatalf("transcript = %s", got)
	}
	saved, err := cockpit.LoadSession(m.opt.SessionDir, m.sessionID)
	if err != nil || texts(saved) != want {
		t.Fatalf("session = %s, %v", texts(saved), err)
	}
	// The edited prompt went into the history like any other.
	if last := m.history[len(m.history)-1]; last.Prompt != "second question, edited" {
		t.Fatalf("history ends with %q", last.Prompt)
	}
}

func TestEscEscWhileStoppingEditsOnceStopped(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	m.input.SetValue("wait for a signal")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return len(m.tr.Entries) > 2 && m.tr.Entries[2].State == "" })

	// Two escapes that arrive together are one esc esc.
	_, stop := m.Update(key("alt+esc"))
	if m.state != stopping {
		t.Fatalf("state = %v", m.state)
	}
	press(m, key("esc"))
	press(m, key("esc"))
	if !m.editAfterStop || m.edit != nil {
		t.Fatalf("esc esc while stopping: edit after stop %v, edit %v", m.editAfterStop, m.edit)
	}
	drive(t, m, tea.Batch(stop, waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
	if m.edit == nil || m.input.Value() != "wait for a signal" || m.editAfterStop {
		t.Fatalf("once stopped: edit %v, composer %q", m.edit, m.input.Value())
	}

	m.input.SetValue("second question")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if got := texts(m.tr); got != "first question|echo: first question|second question|echo: second question" {
		t.Fatalf("transcript = %s", got)
	}
	if m.tr.Interrupted() {
		t.Fatal("the stopped run is still there")
	}
}

func TestClickingAPromptHeaderEditsIt(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	runPrompt(t, m, "second question")
	m.input.SetValue("a draft")

	click(m, 30, headerRow(t, m, "first question"))
	if m.edit == nil || m.input.Value() != "first question" || m.replaced() != 3 {
		t.Fatalf("edit %v, composer %q", m.edit, m.input.Value())
	}
	// Another prompt's header moves the edit there; the draft still waits.
	click(m, 30, headerRow(t, m, "second question"))
	if m.input.Value() != "second question" || m.edit.draft != "a draft" {
		t.Fatalf("composer %q, draft %q", m.input.Value(), m.edit.draft)
	}
	// A click in a prompt's text is not a click on its header. (Right after
	// the mouse, esc waits a moment in case it starts a cut mouse report.)
	press(m, key("esc"))
	if m.edit != nil || m.input.Value() != "a draft" {
		t.Fatalf("esc: edit %v, composer %q", m.edit, m.input.Value())
	}
	click(m, 30, headerRow(t, m, "first question")+1)
	if m.edit != nil {
		t.Fatal("a click in the prompt's text started an edit")
	}

	// During a run, headers do not edit.
	m.input.SetValue("wait for a signal")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return len(m.tr.Entries) > 4 && m.tr.Entries[4].State == "" })
	click(m, 30, headerRow(t, m, "first question"))
	if m.edit != nil {
		t.Fatal("started an edit during a run")
	}
	if strings.Contains(ansi.Strip(strings.Join(m.lines, "\n")), "✎ edit") {
		t.Fatal("prompts offer an edit during a run")
	}
	_, stop := m.Update(key("alt+esc"))
	drive(t, m, tea.Batch(stop, waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
}

func TestEditingAPromptTheRunnerNeverGot(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("crash right away")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if m.input.Value() != "crash right away" {
		t.Fatalf("composer = %q", m.input.Value())
	}
	m.Update(key("alt+esc"))
	if m.edit == nil || m.edit.draft != "" {
		t.Fatalf("edit = %+v", m.edit)
	}
	m.input.SetValue("hello")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if got := texts(m.tr); got != "hello|echo: hello" || m.input.Value() != "" {
		t.Fatalf("transcript %s, composer %q", got, m.input.Value())
	}
}

func TestEditingAPromptAnotherWindowRewound(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	runPrompt(t, m, "second question")
	m.Update(key("alt+esc"))
	second := strings.TrimPrefix(m.edit.id, "input:")
	if err := cockpit.RewindSession(m.opt.SessionDir, m.sessionID, second); err != nil {
		t.Fatal(err)
	}
	m.input.SetValue("second question, edited")
	_, cmd := m.Update(key("enter"))
	if m.edit != nil || m.state != idle || m.input.Value() != "second question, edited" {
		t.Fatalf("edit %v, state %v, composer %q", m.edit, m.state, m.input.Value())
	}
	drive(t, m, cmd, func() bool { return !m.loading })
	if got := texts(m.tr); got != "first question|echo: first question" {
		t.Fatalf("transcript = %s", got)
	}
}

func TestEditCommandAndCommandsOverAnEdit(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	runPrompt(t, m, "second question")
	m.input.SetValue("a draft")
	m.command("/edit 1")
	if m.edit == nil || m.input.Value() != "first question" {
		t.Fatalf("edit %v, composer %q", m.edit, m.input.Value())
	}
	// A command typed over the edit ends it.
	m.input.SetValue("/effort low")
	m.Update(key("enter"))
	if m.edit != nil || m.thinking != "low" || m.input.Value() != "a draft" {
		t.Fatalf("edit %v, effort %q, composer %q", m.edit, m.thinking, m.input.Value())
	}
	if m.command("/edit 7"); m.note.level != "warn" || m.edit != nil {
		t.Fatal("edited a prompt that does not exist")
	}
	// Starting another session ends an edit too.
	m.command("/edit")
	m.newSession()
	if m.edit != nil || m.input.Value() != "a draft" {
		t.Fatalf("new session: edit %v, composer %q", m.edit, m.input.Value())
	}
}
