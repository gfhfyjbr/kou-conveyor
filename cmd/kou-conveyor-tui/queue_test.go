package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

var ctrlX = tea.KeyMsg{Type: tea.KeyCtrlX}

func queueTexts(q *cockpit.Queue) string {
	var parts []string
	for _, item := range q.Items {
		text := item.Text
		if item.Forced {
			text = "!" + text
		}
		parts = append(parts, text)
	}
	if q.Paused {
		parts = append(parts, "(paused)")
	}
	return strings.Join(parts, " ")
}

// persisted reports that the runner recorded the run's prompt.
func promptPersisted(m *uiModel) bool {
	return len(m.tr.Entries) > 0 && m.tr.Entries[0].State == ""
}

func TestQueuedPromptRunsWhenTheRunEnds(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("hold at the gate")
	_, cmd := m.Update(key("enter"))
	queued := false
	drive(t, m, cmd, func() bool {
		if !queued && promptPersisted(m) {
			queued = true
			m.input.SetValue("then say hi")
			m.Update(key("enter"))
			q := m.queue()
			if q.Queued() != 1 || m.input.Value() != "" || len(m.tr.Entries) != 1 {
				t.Errorf("queue %q, composer %q, transcript %s", queueTexts(q), m.input.Value(), kinds(m.tr))
			}
			screen := ansi.Strip(m.View())
			for _, want := range []string{"QUEUE 1", "runs when the agent finishes", "then say hi", "next"} {
				if !strings.Contains(screen, want) {
					t.Errorf("the screen lacks %q:\n%s", want, screen)
				}
			}
			if lines := strings.Count(m.View(), "\n") + 1; lines != m.height {
				t.Errorf("the screen has %d lines, the terminal %d", lines, m.height)
			}
			os.WriteFile(filepath.Join(m.opt.Workspace, "gate"), nil, 0o600)
		}
		return m.state == idle && strings.Contains(texts(m.tr), "echo: then say hi")
	})
	if got := texts(m.tr); got != "hold at the gate|echo: hold at the gate|then say hi|echo: then say hi" {
		t.Fatalf("transcript = %s", got)
	}
	if q := m.queue(); len(q.Items) != 0 || m.queueRows() != 0 {
		t.Fatalf("queue = %q", queueTexts(q))
	}
	if history, _ := loadHistory(m.opt.historyFile); len(history) != 2 {
		t.Fatalf("history = %#v", history)
	}
}

func TestForcedPromptReachesTheRunningAgent(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("steer the tests")
	_, cmd := m.Update(key("enter"))
	forced := false
	drive(t, m, cmd, func() bool {
		if running := m.tr.Running(); !forced && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
			forced = true
			m.input.SetValue("use pnpm")
			m.Update(ctrlX)
			if q := m.queue(); queueTexts(q) != "!use pnpm" {
				t.Errorf("queue = %q", queueTexts(q))
			}
			screen := ansi.Strip(m.View())
			for _, want := range []string{"⚡ goes in after the running tools", "use pnpm", "after make test"} {
				if !strings.Contains(screen, want) {
					t.Errorf("the screen lacks %q:\n%s", want, screen)
				}
			}
		}
		return m.state == idle
	})
	if got := kinds(m.tr); got != "user,tool:done,user,assistant" {
		t.Fatalf("transcript = %s", got)
	}
	if e := m.tr.Entries[2]; !e.Forced || e.Text != "use pnpm" || m.tr.Entries[3].Text != "steered: use pnpm" {
		t.Fatalf("forced entry %+v, answer %q", e, m.tr.Entries[3].Text)
	}
	if q := m.queue(); len(q.Items) != 0 {
		t.Fatalf("queue = %q", queueTexts(q))
	}
	if !strings.Contains(ansi.Strip(strings.Join(m.lines, "\n")), "⚡ forced in") {
		t.Fatal("the transcript does not mark the forced prompt")
	}
}

func TestStoppedRunPausesTheQueue(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("wait for a signal")
	_, cmd := m.Update(key("enter"))
	stopped := false
	drive(t, m, cmd, func() bool {
		if !stopped && promptPersisted(m) {
			stopped = true
			m.input.SetValue("second")
			m.Update(key("enter"))
			// The agent never gets to read this one: the run is stopped.
			m.input.SetValue("first")
			m.Update(ctrlX)
			if q := m.queue(); queueTexts(q) != "!first second" {
				t.Errorf("queue = %q", queueTexts(q))
			}
			m.stop()
		}
		return stopped && m.state == idle
	})
	q := m.queue()
	if got := queueTexts(q); got != "first second (paused)" {
		t.Fatalf("queue = %q", got)
	}
	if screen := ansi.Strip(m.View()); !strings.Contains(screen, "PAUSED") || !strings.Contains(screen, "send the next queued") {
		t.Fatalf("the screen does not say the queue is paused:\n%s", screen)
	}
	// Enter on an empty prompt goes on with the queue, one prompt at a time.
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle && len(q.Items) == 0 })
	got := texts(m.tr)
	if !strings.HasSuffix(got, "first|echo: first|second|echo: second") {
		t.Fatalf("transcript = %s", got)
	}
}

func TestQueueTakesTheKeys(t *testing.T) {
	m := testModel(t)
	m.state = running // a run whose output does not matter here
	defer func() { m.state = idle }()
	for _, text := range []string{"one", "two", "three"} {
		m.input.SetValue(text)
		m.Update(key("enter"))
	}
	q := m.queue()
	if queueTexts(q) != "one two three" {
		t.Fatalf("queue = %q", queueTexts(q))
	}
	// ↑ over the empty composer selects the last prompt, and again the one
	// before it.
	m.Update(key("up"))
	m.Update(key("up"))
	if m.queueFocus != 1 || !strings.Contains(ansi.Strip(m.footer()), "force in") {
		t.Fatalf("focus %d, footer %q", m.queueFocus, ansi.Strip(m.footer()))
	}
	// Enter takes it into the composer; enter puts it back where it was.
	m.Update(key("enter"))
	if m.input.Value() != "two" || m.queueEdit == nil || queueTexts(q) != "one three" {
		t.Fatalf("composer %q, queue %q", m.input.Value(), queueTexts(q))
	}
	if screen := ansi.Strip(m.View()); !strings.Contains(screen, "QUEUED 2") || !strings.Contains(screen, "editing in the composer") {
		t.Fatalf("the screen does not show the edit:\n%s", screen)
	}
	m.input.SetValue("TWO")
	m.Update(key("enter"))
	if queueTexts(q) != "one TWO three" || m.input.Value() != "" {
		t.Fatalf("queue %q, composer %q", queueTexts(q), m.input.Value())
	}
	// Esc leaves an edit as it was.
	m.Update(key("up"))
	m.Update(key("enter"))
	m.input.SetValue("gone")
	press(m, key("esc"))
	if queueTexts(q) != "one TWO three" {
		t.Fatalf("esc changed the queue: %q", queueTexts(q))
	}
	// Shift+↑ moves the selected prompt; backspace drops it.
	m.Update(key("up"))
	m.Update(tea.KeyMsg{Type: tea.KeyShiftUp})
	m.Update(tea.KeyMsg{Type: tea.KeyShiftUp})
	if queueTexts(q) != "three one TWO" || m.queueFocus != 0 {
		t.Fatalf("queue %q, focus %d", queueTexts(q), m.queueFocus)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyBackspace})
	if queueTexts(q) != "one TWO" {
		t.Fatalf("queue %q", queueTexts(q))
	}
	// Typing goes back to the composer.
	m.Update(runes("n"))
	if m.queueFocus != -1 || m.input.Value() != "n" {
		t.Fatalf("focus %d, composer %q", m.queueFocus, m.input.Value())
	}
	m.input.Reset()

	// The queue sits between the status line and the composer, and a click
	// on a prompt selects it.
	if m.ruleRow() != m.statusRow()+1+3 || m.queueRows() != 3 {
		t.Fatalf("rule row %d, status row %d, queue rows %d", m.ruleRow(), m.statusRow(), m.queueRows())
	}
	if lines := strings.Count(m.View(), "\n") + 1; lines != m.height {
		t.Fatalf("the screen has %d lines, the terminal %d", lines, m.height)
	}
	click(m, 10, m.queueTop()+2)
	if m.queueFocus != 1 {
		t.Fatalf("a click selected %d", m.queueFocus)
	}
	press(m, key("esc"))
	if m.queueFocus != -1 {
		t.Fatal("esc kept the queue focused")
	}
	m.command("/queue clear")
	if len(q.Items) != 0 || m.queueRows() != 0 {
		t.Fatalf("queue %q", queueTexts(q))
	}
}

func TestOlderRunnerKeepsForcedPromptsQueued(t *testing.T) {
	t.Setenv(cockpittest.NoSteer, "1")
	m := testModel(t)
	// A runner of its own, which is asked afresh what it can do.
	runner := filepath.Join(t.TempDir(), "kou-conveyor-runner")
	if err := os.Symlink(m.opt.Runner, runner); err != nil {
		t.Fatal(err)
	}
	m.opt.Runner = runner
	m.input.SetValue("hold at the gate")
	_, cmd := m.Update(key("enter"))
	forced := false
	drive(t, m, cmd, func() bool {
		if !forced && promptPersisted(m) {
			forced = true
			m.input.SetValue("use pnpm")
			m.Update(ctrlX)
			if q := m.queue(); queueTexts(q) != "use pnpm" || !strings.Contains(m.note.text, "takes no messages") {
				t.Errorf("queue %q, note %q", queueTexts(q), m.note.text)
			}
			os.WriteFile(filepath.Join(m.opt.Workspace, "gate"), nil, 0o600)
		}
		return m.state == idle && strings.Contains(texts(m.tr), "echo: use pnpm")
	})
}
