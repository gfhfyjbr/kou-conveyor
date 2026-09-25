package main

import (
	"encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestMain(m *testing.M) {
	cockpittest.Main()
	os.Exit(m.Run())
}

func testModel(t *testing.T) *uiModel {
	t.Helper()
	workspace := t.TempDir()
	o := options{thinking: "high", historyFile: filepath.Join(workspace, "history.json")}
	o.Workspace, o.SessionDir, o.Heartbeat = workspace, filepath.Join(workspace, "sessions"), time.Minute
	o.Runner = cockpittest.Runner(t)
	// Plugins and their settings stay in the test's directory.
	t.Setenv(plugin.ConfigEnvironment, filepath.Join(workspace, "config", "settings.json"))
	m := newModel(t.Context(), o)
	// The last snapshot of a run is taken after it: the test waits for it.
	t.Cleanup(m.shutdown)
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	return m
}

// drive plays the Bubble Tea runtime: it runs commands and feeds their
// messages back to the model until done reports true.
func drive(t *testing.T, m *uiModel, cmd tea.Cmd, done func() bool) {
	t.Helper()
	msgs := make(chan tea.Msg, 256)
	var run func(tea.Cmd)
	run = func(c tea.Cmd) {
		if c == nil {
			return
		}
		go func() {
			msg := c()
			if batch, ok := msg.(tea.BatchMsg); ok {
				for _, c := range batch {
					run(c)
				}
				return
			}
			select {
			case msgs <- msg:
			case <-t.Context().Done():
			}
		}()
	}
	run(cmd)
	timeout := time.After(15 * time.Second)
	for !done() {
		select {
		case msg := <-msgs:
			switch msg.(type) {
			case tickMsg, noticeMsg, toastMsg:
				continue // timers are irrelevant here and would keep the loop busy
			}
			_, next := m.Update(msg)
			run(next)
		case <-timeout:
			t.Fatal("timed out waiting for the model")
		}
	}
}

func key(s string) tea.KeyMsg {
	switch s {
	case "enter":
		return tea.KeyMsg{Type: tea.KeyEnter}
	case "esc":
		return tea.KeyMsg{Type: tea.KeyEsc}
	case "up":
		return tea.KeyMsg{Type: tea.KeyUp}
	case "down":
		return tea.KeyMsg{Type: tea.KeyDown}
	case "ctrl+c":
		return tea.KeyMsg{Type: tea.KeyCtrlC}
	case "ctrl+o":
		return tea.KeyMsg{Type: tea.KeyCtrlO}
	case "left":
		return tea.KeyMsg{Type: tea.KeyLeft}
	case "right":
		return tea.KeyMsg{Type: tea.KeyRight}
	case "tab":
		return tea.KeyMsg{Type: tea.KeyTab}
	case "ctrl+b":
		return tea.KeyMsg{Type: tea.KeyCtrlB}
	case "ctrl+d":
		return tea.KeyMsg{Type: tea.KeyCtrlD}
	case "ctrl+e":
		return tea.KeyMsg{Type: tea.KeyCtrlE}
	case "ctrl+t":
		return tea.KeyMsg{Type: tea.KeyCtrlT}
	case "ctrl+j":
		return tea.KeyMsg{Type: tea.KeyCtrlJ}
	case "alt+enter":
		return tea.KeyMsg{Type: tea.KeyEnter, Alt: true}
	case "alt+esc":
		return tea.KeyMsg{Type: tea.KeyEsc, Alt: true}
	case "alt+up":
		return tea.KeyMsg{Type: tea.KeyUp, Alt: true}
	case "alt+down":
		return tea.KeyMsg{Type: tea.KeyDown, Alt: true}
	case "shift+tab":
		return tea.KeyMsg{Type: tea.KeyShiftTab}
	case "shift+left":
		return tea.KeyMsg{Type: tea.KeyShiftLeft}
	case "shift+right":
		return tea.KeyMsg{Type: tea.KeyShiftRight}
	}
	return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)}
}

func kinds(tr *cockpit.Transcript) string {
	var out []string
	for _, e := range tr.Entries {
		k := e.Kind
		if e.Tool != nil {
			k += ":" + e.Tool.State
		}
		if e.State != "" {
			k += ":" + e.State
		}
		out = append(out, k)
	}
	return strings.Join(out, ",")
}

func TestRunLifecycle(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("use a tool")
	_, cmd := m.Update(key("enter"))
	if m.state != running || m.input.Value() != "" {
		t.Fatalf("state %v, input %q", m.state, m.input.Value())
	}
	drive(t, m, cmd, func() bool { return m.state == idle })

	if got := kinds(m.tr); got != "user,tool:done,assistant" {
		t.Fatalf("transcript = %s", got)
	}
	if m.fresh || m.last.kind != "done" {
		t.Fatalf("fresh %v, outcome %q", m.fresh, m.last.kind)
	}
	if history, _ := loadHistory(m.opt.historyFile); len(history) != 1 || history[0].SessionID != m.sessionID {
		t.Fatalf("history = %#v", history)
	}

	// A click on the tool row expands its output.
	m.view.GotoTop()
	for _, s := range m.spans {
		if s.id == "tool:call-1" {
			click(m, 20, m.top()+s.start-m.view.YOffset)
		}
	}
	if !m.expanded["tool:call-1"] || !strings.Contains(m.view.View(), "OUTPUT ·") {
		t.Fatalf("tool was not expanded:\n%s", m.view.View())
	}
}

func TestStopInterruptsRun(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("wait for a signal")
	_, cmd := m.Update(key("enter"))
	// Stop once the runner has persisted the prompt.
	drive(t, m, cmd, func() bool { return len(m.tr.Entries) > 0 && m.tr.Entries[0].State == "" })
	stop := press(m, key("esc"))
	if m.state != running {
		t.Fatal("a single esc stopped the run")
	}
	stop = press(m, key("esc"))
	if m.state != stopping {
		t.Fatalf("state = %v", m.state)
	}
	drive(t, m, tea.Batch(stop, waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
	if m.last.kind != "stopped" || !strings.HasSuffix(kinds(m.tr), "notice") {
		t.Fatalf("outcome %q, transcript %s", m.last.kind, kinds(m.tr))
	}
}

func TestUndeliveredPromptReturnsToComposer(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("crash right away")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if got := kinds(m.tr); got != "user:undelivered,error" {
		t.Fatalf("transcript = %s", got)
	}
	if m.input.Value() != "crash right away" || m.last.kind != "failed" {
		t.Fatalf("input %q, outcome %q", m.input.Value(), m.last.kind)
	}
}

func TestStaleMessagesAreDropped(t *testing.T) {
	m := testModel(t)
	m.jobGen = 3
	m.Update(linesMsg{gen: 2, lines: []cockpit.Line{{Text: `{"type":"error","message":"late"}`}}})
	if len(m.tr.Entries) != 0 {
		t.Fatalf("a late runner line reached the transcript: %s", kinds(m.tr))
	}
	if _, cmd := m.Update(tickMsg{gen: 2}); cmd != nil {
		t.Fatal("a stale tick kept ticking")
	}

	first, second := m.load("first-session"), m.load("second-session")
	m.Update(second())
	m.Update(first()) // arrives last but belongs to the older request
	if m.sessionID != "second-session" || m.loading {
		t.Fatalf("session %q, loading %v", m.sessionID, m.loading)
	}
}

func TestSessionSwitchWaitsForRun(t *testing.T) {
	m := testModel(t)
	m.state = running
	before := m.loadGen
	if m.openSession("other-session"); m.loadGen != before || m.note.level != "warn" {
		t.Fatalf("switched sessions during a run: load gen %d → %d", before, m.loadGen)
	}
	pending := m.tr.Submit("0b7d5e7c-4a0f-4a4e-9a55-7f6d1f3c2b10", "keep me", time.Now())
	if m.command("/clear"); m.tr.Entry(pending.ID) == nil {
		t.Fatal("/clear dropped the prompt of a running run")
	}
	m.state = idle
	m.loading = true
	m.input.SetValue("hello")
	m.submit()
	if m.job != nil || m.input.Value() != "hello" {
		t.Fatal("submitted while the session was loading")
	}
}

func TestHistoryBrowsing(t *testing.T) {
	m := testModel(t)
	m.history = appendHistory(appendHistory(nil, "first", "s", ""), "second", "s", "")
	m.input.SetValue("draft")
	for _, step := range []struct{ key, want string }{
		{"up", "second"}, {"up", "first"}, {"up", "first"}, {"down", "second"}, {"down", "draft"},
	} {
		m.Update(key(step.key))
		if m.input.Value() != step.want {
			t.Fatalf("after %s: %q, want %q", step.key, m.input.Value(), step.want)
		}
	}
}

func TestCommandsAndQuit(t *testing.T) {
	m := testModel(t)
	m.command("/think max")
	m.command("/model gpt-test")
	if m.thinking != "max" || m.nextModel() != "gpt-test" {
		t.Fatalf("thinking %q, model %q", m.thinking, m.nextModel())
	}
	m.command("/nope")
	if m.note.level != "warn" {
		t.Fatalf("notice = %#v", m.note)
	}
	m.input.SetValue("/ef")
	m.Update(tea.KeyMsg{Type: tea.KeyTab})
	if m.input.Value() != "/effort " {
		t.Fatalf("completion = %q", m.input.Value())
	}
	m.input.Reset()
	// The first ctrl+c only arms quitting; its command is a notice timer.
	if m.Update(key("ctrl+c")); m.quitArmed.IsZero() {
		t.Fatal("the first ctrl+c did not ask for confirmation")
	}
	if _, cmd := m.Update(key("ctrl+c")); !isQuit(cmd) {
		t.Fatal("the second ctrl+c did not quit")
	}
}

func isQuit(cmd tea.Cmd) bool {
	if cmd == nil {
		return false
	}
	_, ok := cmd().(tea.QuitMsg)
	return ok
}

func item(t *testing.T, sequence int, kind sessionstore.ItemKind, data any) string {
	t.Helper()
	line, err := json.Marshal(sessionstore.Item{Sequence: sessionstore.Sequence(sequence), RecordedAt: time.Now(), Kind: kind, Data: data})
	if err != nil {
		t.Fatal(err)
	}
	return string(line)
}

func TestViewFitsTerminal(t *testing.T) {
	m := testModel(t)
	m.tr.Submit("7c5d3f8e-0f7a-4d38-9d8f-2f1c3a7e9b10", strings.Repeat("a long prompt that wraps ", 12), time.Now())
	failed, _ := json.Marshal(operation.ShellState{Result: &operation.ShellResult{Out: "line\n" + strings.Repeat("x", 300), ExitCode: 2}})
	for _, line := range []string{
		item(t, 1, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "t", Response: llm.Response{Output: []llm.Item{
			{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"**Planning** the work"}}},
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "c1", Name: "Bash", Arguments: `{"command":"` + strings.Repeat("echo wide ", 30) + `"}`}},
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "# Result\n\n- **bold** item with `code`\n\n```go\nfunc main() {}\n```\n\n" + strings.Repeat("word ", 80)}},
		}}}),
		item(t, 2, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "t", CallID: "c1", Status: tool.CallStatus{WaitingFor: []operation.ID{"o"}},
			Operations: []operation.Operation{{ID: "o", Type: operation.TypeShell, Version: operation.VersionShell, Status: operation.StatusCompleted, State: failed}}}),
		`{"type":"error","message":"` + strings.Repeat("boom ", 40) + `"}`,
	} {
		if _, err := m.tr.Apply([]byte(line)); err != nil {
			t.Fatal(err)
		}
	}
	m.expandAll = true
	for _, size := range [][2]int{{24, 8}, {40, 12}, {80, 24}, {160, 50}} {
		m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		for _, overlay := range []string{"", "palette", "help"} {
			m.picker = nil
			if overlay != "" {
				m.openPicker(overlay)
			}
			lines := strings.Split(m.View(), "\n")
			if len(lines) != size[1] {
				t.Fatalf("%dx%d %s: %d lines", size[0], size[1], overlay, len(lines))
			}
			for i, line := range lines {
				if w := ansi.StringWidth(line); w > size[0] {
					t.Fatalf("%dx%d %s: line %d is %d wide: %q", size[0], size[1], overlay, i, w, ansi.Strip(line))
				}
			}
		}
	}
}

func TestPickerSurvivesResizeAndClipping(t *testing.T) {
	m := testModel(t)
	for i := range 30 {
		m.history = appendHistory(m.history, fmt.Sprintf("prompt %02d", i), "s", "")
	}
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 20})
	m.openPicker("history")
	m.picker.move(len(m.picker.shown))
	m.View()
	// Growing the terminal while scrolled to the end used to overrun the list.
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 40})
	m.View()

	// In a terminal too short for the whole box, rows that were clipped away
	// must not be clickable.
	m.closePicker()
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 10})
	m.openPicker("palette")
	m.View()
	sessionBefore := m.sessionID
	for y := 0; y < 10; y++ {
		if i := m.picker.rowAt(y); i >= 0 && y >= m.top()+m.view.Height {
			t.Fatalf("row %d outside the transcript area maps to item %d", y, i)
		}
	}
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, Y: 9})
	if m.picker == nil || m.sessionID != sessionBefore {
		t.Fatal("a click on the key hints ran a palette command")
	}
}

func TestWatchesSessionRunElsewhere(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("hello there")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	id, entries := m.sessionID, len(m.tr.Entries)

	// Another process takes the session and appends to it.
	unlock, err := cockpit.LockSession(m.opt.SessionDir, id)
	if err != nil {
		t.Fatal(err)
	}
	load := m.load(id)
	m.Update(load())
	if !m.external || !strings.Contains(ansi.Strip(m.header()), "IN USE") {
		t.Fatalf("external %v, header %q", m.external, ansi.Strip(m.header()))
	}
	m.input.SetValue("me too")
	if m.submit(); m.job != nil {
		t.Fatal("started a run in a session another process is running")
	}
	path := cockpit.SessionPath(m.opt.SessionDir, id)
	record, _ := json.Marshal(map[string]any{"type": "item", "data": map[string]any{"Item": sessionstore.Item{
		Sequence: 99, RecordedAt: time.Now(), Kind: sessionstore.ItemModelResponse,
		Data: sessionstore.ModelResponse{TurnID: "t", Response: llm.Response{Output: []llm.Item{
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "from elsewhere"}},
		}}},
	}}})
	file, _ := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	file.Write(append(record, '\n'))
	file.Close()
	info, _ := os.Stat(path)

	_, read := m.Update(watchMsg{gen: m.loadGen, size: info.Size(), busy: true})
	m.Update(read())
	if len(m.tr.Entries) != entries+1 || !m.external {
		t.Fatalf("entries %d → %d, external %v", entries, len(m.tr.Entries), m.external)
	}
	unlock()
	m.Update(watchMsg{gen: m.loadGen, size: info.Size(), busy: false})
	if m.external {
		t.Fatal("still following after the other run ended")
	}
}
