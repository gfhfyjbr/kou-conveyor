package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
)

// controlOf returns the column where a control of the composer starts.
func controlOf(t *testing.T, m *uiModel, kind hoverKind) int {
	t.Helper()
	_, hits := m.controls()
	for _, c := range hits {
		if c.kind == kind {
			return c.from
		}
	}
	t.Fatalf("no control %d among %+v", kind, hits)
	return 0
}

func TestTheRunButtonRunsQueuesForcesAndStops(t *testing.T) {
	m := testModel(t)
	// With nothing written the button does nothing, and says so.
	row := m.controlsRow()
	button := controlOf(t, m, hoverRun)
	if cmd := click(m, button, row); cmd != nil || m.state != idle {
		t.Fatal("an empty prompt ran")
	}
	move(m, button, row)
	if h := m.hover(); h.kind != hoverRun || !strings.Contains(m.hoverHint(h), "write a prompt") {
		t.Fatalf("hover %+v", h)
	}
	if !strings.Contains(ansi.Strip(m.View()), "RUN ↵") {
		t.Fatal("the button is not on screen")
	}

	// Written, a click runs it.
	m.input.SetValue("hold at the gate")
	m.View()
	cmd := click(m, controlOf(t, m, hoverRun), row)
	if m.state != running || m.input.Value() != "" {
		t.Fatalf("state %v, composer %q", m.state, m.input.Value())
	}
	stage := 0
	drive(t, m, cmd, func() bool {
		switch {
		case stage == 0 && promptPersisted(m):
			stage = 1
			// While it runs, a written prompt queues, and beside the queue
			// button the force chip forces it in.
			m.input.SetValue("then say hi")
			m.View()
			if !strings.Contains(ansi.Strip(m.View()), "QUEUE ↵") || !strings.Contains(ansi.Strip(m.View()), "FORCE") {
				t.Errorf("the buttons of a run: %q", ansi.Strip(strings.Split(m.View(), "\n")[m.controlsRow()]))
			}
			from := controlOf(t, m, hoverRun)
			click(m, from+ansi.StringWidth(m.runButton())-2, m.controlsRow())
			if q := m.queue(); q.Queued() != 1 || m.input.Value() != "" {
				t.Errorf("queue %q, composer %q", queueTexts(q), m.input.Value())
			}
			// Empty, the button stops the run.
			m.View()
			if !strings.Contains(ansi.Strip(m.View()), "STOP") {
				t.Errorf("no stop button: %q", ansi.Strip(strings.Split(m.View(), "\n")[m.controlsRow()]))
			}
			os.WriteFile(filepath.Join(m.opt.Workspace, "gate"), nil, 0o600)
		}
		return m.state == idle && strings.Contains(texts(m.tr), "echo: then say hi")
	})
	if got := texts(m.tr); got != "hold at the gate|echo: hold at the gate|then say hi|echo: then say hi" {
		t.Fatalf("transcript = %s", got)
	}

	// A click on stop stops a run.
	m.input.SetValue("wait for a signal")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return promptPersisted(m) })
	m.View()
	stop := click(m, controlOf(t, m, hoverRun), m.controlsRow())
	if m.state != stopping {
		t.Fatalf("state = %v", m.state)
	}
	drive(t, m, tea.Batch(stop, waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
	if m.last.kind != "stopped" {
		t.Fatalf("outcome = %q", m.last.kind)
	}
}

func TestTheWelcomeScreenLaysTasksOutInAGrid(t *testing.T) {
	m := testModel(t)
	m.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
	if len(m.starters) != len(starters) || m.starters[1].left <= m.starters[0].left || m.starters[1].top != m.starters[0].top {
		t.Fatalf("two columns: %+v", m.starters)
	}
	if m.starters[2].top <= m.starters[0].bottom {
		t.Fatalf("two rows: %+v", m.starters)
	}
	shown := strings.Join(screen(t, m), "\n")
	for _, want := range []string{"SESSION · NEW", "01 SURVEY", "02 VERIFY", "03 REVIEW", "04 PROFILE", wordmark[0]} {
		if !strings.Contains(shown, want) {
			t.Fatalf("the welcome screen lacks %q:\n%s", want, shown)
		}
	}
	// The pointer over a task lights that task up, not the row; a click on
	// the second column takes its task.
	second := m.starters[1]
	x, y := second.left+3, m.top()+second.top+1
	move(m, x, y)
	if h := m.hover(); h.kind != hoverStarter || h.from != second.left || h.to != second.right {
		t.Fatalf("hover = %+v", h)
	}
	click(m, x, y)
	if m.input.Value() != starters[1].prompt {
		t.Fatalf("composer = %q", m.input.Value())
	}
	// Narrow, the tasks go one under another.
	m.input.SetValue("")
	m.Update(tea.WindowSizeMsg{Width: 60, Height: 40})
	if m.starters[1].left != m.starters[0].left || m.starters[1].top <= m.starters[0].top {
		t.Fatalf("one column: %+v", m.starters)
	}
}

func TestCardEdgesAreNeitherTextNorCopied(t *testing.T) {
	copied := fakeClipboard(t)
	m := testModel(t)
	runPrompt(t, m, "use a tool")
	m.view.GotoTop()
	// The prompt's card: its edges around its text.
	header := headerRow(t, m, "use a tool")
	lines := screen(t, m)
	if edge := lines[header+1]; !strings.Contains(edge, "┎") || strings.Contains(edge, "use a tool") {
		t.Fatalf("the card's top edge = %q", edge)
	}
	if text := lines[header+2]; !strings.Contains(text, "┃ use a tool") {
		t.Fatalf("the card's text = %q", text)
	}
	move(m, 20, header+1)
	if h := m.hover(); h.kind != hoverNone {
		t.Fatalf("an edge is text: %+v", h)
	}
	// Dragged from the prompt through the edges to the answer, the copy
	// holds the text alone.
	prompt := find(t, m, "use a tool")
	answer := find(t, m, "echo: use a tool")
	dragAndCopy(t, m, prompt, cell{answer.row, answer.col + 3})
	if got := (*copied)[0]; !strings.HasPrefix(got, "use a tool\n") || strings.ContainsAny(got, "┎┖─│┃") {
		t.Fatalf("copied %q", got)
	}
	// An open tool call is a card with its output's heading on its edge.
	click(m, 30, find(t, m, "BASH").row)
	shown := strings.Join(screen(t, m), "\n")
	if !strings.Contains(shown, "┌─ OUTPUT · 1 LINE ─") || !strings.Contains(shown, "│ hi") {
		t.Fatalf("the open call:\n%s", shown)
	}
}

func TestShortTerminalsPutTheControlsOnTheEdge(t *testing.T) {
	m := testModel(t)
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 18})
	if m.roomy() || m.top() != 2 {
		t.Fatalf("roomy %v, top %d", m.roomy(), m.top())
	}
	lines := screen(t, m)
	edge := lines[m.controlsRow()]
	if !strings.HasPrefix(strings.TrimSpace(edge), "└") || !strings.HasSuffix(edge, "┘") || !strings.Contains(edge, "EFFORT") || !strings.Contains(edge, "RUN ↵") {
		t.Fatalf("the bottom edge = %q", edge)
	}
	// Its controls still take clicks.
	row, meter := m.effortAt()
	click(m, meter+1, row)
	if m.thinking != "medium" || row != m.controlsRow() {
		t.Fatalf("effort %q from row %d", m.thinking, row)
	}
	// Roomy again, the controls have a row of their own under the text,
	// and the box a bottom edge.
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	lines = screen(t, m)
	if controls, bottom := lines[m.controlsRow()], lines[m.controlsRow()+1]; !strings.HasPrefix(strings.TrimSpace(controls), "│") || !strings.HasPrefix(strings.TrimSpace(bottom), "└") {
		t.Fatalf("controls %q, bottom %q", controls, bottom)
	}
}

func TestWideTerminalsKeepTheMeasure(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "use a tool")
	m.Update(tea.WindowSizeMsg{Width: 200, Height: 30})
	if m.width != maxMeasure || m.left() != (200-maxMeasure)/2 || m.view.Width != maxMeasure-margin-2 {
		t.Fatalf("width %d, left %d, transcript %d", m.width, m.left(), m.view.Width)
	}
	lines := strings.Split(m.View(), "\n")
	if len(lines) != m.height {
		t.Fatalf("%d lines for %d rows", len(lines), m.height)
	}
	pad := strings.Repeat(" ", m.left())
	for i, line := range lines {
		if w := ansi.StringWidth(line); w > 200 {
			t.Fatalf("line %d is %d wide", i, w)
		}
		// Every row but the bar's rule starts at the cockpit's column.
		if i != 1 && !strings.HasPrefix(ansi.Strip(line), pad) {
			t.Fatalf("line %d is not in the middle: %q", i, ansi.Strip(line))
		}
	}
	if rule := ansi.Strip(lines[1]); ansi.StringWidth(rule) != 200 || strings.TrimLeft(rule, "─") != "" {
		t.Fatalf("the rule = %q", rule)
	}
	// The pointer's columns are the cockpit's: the prompt's header edits it
	// from the middle of the terminal, not from its left edge.
	m.view.GotoTop()
	header := headerRow(t, m, "use a tool")
	move(m, 20, header)
	if h := m.hover(); h.kind != hoverNone {
		t.Fatalf("hover at the edge = %+v", h)
	}
	move(m, m.left()+20, header)
	if h := m.hover(); h.kind != hoverEdit {
		t.Fatalf("hover in the middle = %+v", h)
	}
	// Beside the changes panel the cockpit takes the whole width.
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlG})
	if m.width != 200 || m.left() != 0 {
		t.Fatalf("with the panel: width %d, left %d", m.width, m.left())
	}
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlG})
	if m.width != maxMeasure {
		t.Fatalf("without the panel: width %d", m.width)
	}
}
