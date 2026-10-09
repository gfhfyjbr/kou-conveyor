package main

import (
	"encoding/json/v2"
	"fmt"
	"io"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// printLineMessage has the name of the message Bubble Tea's Println sends,
// which the model waits for before it prints more.
type printLineMessage struct{ body string }

// compactModel starts a model in compact mode. scrollback returns what it
// has printed above itself, without styles.
func compactModel(t *testing.T) (m *uiModel, scrollback func() string) {
	t.Helper()
	var mu sync.Mutex
	var printed []string
	saved := printAbove
	printAbove = func(args ...any) tea.Cmd {
		text := fmt.Sprint(args...)
		return func() tea.Msg {
			mu.Lock()
			printed = append(printed, text)
			mu.Unlock()
			return printLineMessage{text}
		}
	}
	t.Cleanup(func() { printAbove = saved })
	workspace := t.TempDir()
	o := options{thinking: "high", historyFile: filepath.Join(workspace, "history.json"), compact: true}
	o.Workspace, o.SessionDir, o.Heartbeat = workspace, filepath.Join(workspace, "sessions"), time.Minute
	o.Runner = cockpittest.Runner(t)
	m = newModel(t.Context(), o)
	t.Cleanup(m.shutdown)
	_, cmd := m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	drive(t, m, cmd, printedAll(m))
	return m, func() string {
		mu.Lock()
		defer mu.Unlock()
		return ansi.Strip(strings.Join(printed, "\n"))
	}
}

func printedAll(m *uiModel) func() bool {
	return func() bool { return !m.scroll.busy && len(m.scroll.queue) == 0 }
}

// runCompact runs a prompt and waits for it and for its printing.
func runCompact(t *testing.T, m *uiModel, prompt string) {
	t.Helper()
	m.input.SetValue(prompt)
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle && printedAll(m)() })
}

func viewLines(m *uiModel) []string { return strings.Split(ansi.Strip(m.View()), "\n") }

func TestCompactPrintsTheTranscriptAsItSettles(t *testing.T) {
	m, scrollback := compactModel(t)
	if got := scrollback(); !strings.Contains(got, "KOU-CONVEYOR") || !strings.Contains(got, "new session") {
		t.Fatalf("header = %q", got)
	}
	// The screen holds only the status line, the composer in its box and the
	// key hints.
	lines := viewLines(m)
	chrome := 3 + len(m.dock(hoverTarget{level: -1}))
	if last := lines[len(lines)-1]; len(lines) != chrome || lines[0] != "" || !strings.Contains(last, "^F") || !strings.Contains(last, "fullscreen") {
		t.Fatalf("view = %q", lines)
	}

	runCompact(t, m, "use a tool")
	got := scrollback()
	order := []string{"YOU", "use a tool", "BASH", "echo hi", "AGENT", "echo: use a tool"}
	at := 0
	for _, want := range order {
		i := strings.Index(got[at:], want)
		if i < 0 {
			t.Fatalf("%q is missing or out of order in:\n%s", want, got)
		}
		at += i + len(want)
	}
	if strings.Contains(got, "✎ edit") {
		t.Fatal("the scrollback offers a click it cannot take")
	}
	if lines := viewLines(m); len(lines) != chrome {
		t.Fatalf("the finished run stayed on screen: %q", lines)
	}

	// Entries print once: another prompt adds only its own.
	runCompact(t, m, "second question")
	if got := scrollback(); strings.Count(got, "echo: use a tool") != 1 || !strings.Contains(got, "echo: second question") {
		t.Fatalf("scrollback:\n%s", got)
	}
}

func TestCompactKeepsWhatIsStillChangingOnScreen(t *testing.T) {
	m, scrollback := compactModel(t)
	m.tr.Submit("7c5d3f8e-0f7a-4d38-9d8f-2f1c3a7e9b10", "build it", time.Now())
	m.state = running
	running, _ := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "make build"}})
	for _, line := range []string{
		`{"Sequence":1,"RecordedAt":"2026-01-01T00:00:00Z","Kind":"input","Data":{"ID":"7c5d3f8e-0f7a-4d38-9d8f-2f1c3a7e9b10","Kind":"external","Payload":"\"build it\""}}`,
		item(t, 2, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "t", Response: llm.Response{Output: []llm.Item{
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "c1", Name: "Bash", Arguments: `{"command":"make build"}`}},
		}}}),
		item(t, 3, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "t", CallID: "c1", Status: tool.CallStatus{WaitingFor: []operation.ID{"o"}},
			Operations: []operation.Operation{{ID: "o", Type: operation.TypeShell, Version: operation.VersionShell, Status: operation.StatusAwaiting, State: running}}}),
	} {
		if _, err := m.tr.Apply([]byte(line)); err != nil {
			t.Fatal(err)
		}
	}
	_, cmd := m.Update(tickMsg{gen: m.jobGen})
	drive(t, m, cmd, printedAll(m))
	// The prompt settled; the running command has not.
	if got := scrollback(); !strings.Contains(got, "build it") || strings.Contains(got, "make build") {
		t.Fatalf("scrollback:\n%s", got)
	}
	if view := strings.Join(viewLines(m), "\n"); !strings.Contains(view, "make build") || strings.Contains(view, "build it\n") {
		t.Fatalf("screen:\n%s", view)
	}
}

func TestCompactEditMarksTheRewind(t *testing.T) {
	m, scrollback := compactModel(t)
	runCompact(t, m, "first question")
	runCompact(t, m, "second question")
	m.Update(key("alt+esc"))
	if m.edit == nil || m.input.Value() != "second question" {
		t.Fatalf("edit %v, composer %q", m.edit, m.input.Value())
	}
	runCompact(t, m, "second question, edited")
	got := scrollback()
	marker := strings.Index(got, "↶ prompt 02 edited")
	if marker < 0 || strings.Index(got, "echo: second question\n") > marker || strings.LastIndex(got, "echo: second question, edited") < marker {
		t.Fatalf("scrollback:\n%s", got)
	}
	if strings.Count(got, "echo: first question") != 1 {
		t.Fatalf("what the edit kept was printed again:\n%s", got)
	}
}

func TestSwitchingLayouts(t *testing.T) {
	m, scrollback := compactModel(t)
	m.opt.preferences = filepath.Join(t.TempDir(), "preferences.json")
	runCompact(t, m, "first question")

	_, cmd := m.Update(tea.KeyMsg{Type: tea.KeyCtrlF})
	if m.compact || !m.switching || cmd == nil {
		t.Fatalf("compact %v, switching %v", m.compact, m.switching)
	}
	m.Update(tea.EnterAltScreen())
	if m.switching || cockpit.LoadPreferences(m.opt.preferences).Layout != cockpit.LayoutFullscreen {
		t.Fatal("the switch to fullscreen did not finish, or was not kept")
	}
	if lines := viewLines(m); len(lines) != m.height || !strings.Contains(strings.Join(lines, "\n"), "echo: first question") {
		t.Fatalf("fullscreen shows %d lines", len(lines))
	}
	// A run in fullscreen prints nothing; its entries wait.
	m.input.SetValue("second question")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	before := scrollback()
	if strings.Contains(before, "second question") {
		t.Fatal("printed into the alternate screen")
	}

	m.Update(tea.KeyMsg{Type: tea.KeyCtrlF})
	if !m.compact || !m.switching {
		t.Fatal("did not switch back")
	}
	// Nothing prints before the terminal has left the alternate screen.
	if _, cmd := m.Update(tea.WindowSizeMsg{Width: 100, Height: 30}); cmd != nil || m.scroll.busy {
		t.Fatal("printed while switching screens")
	}
	_, cmd = m.Update(tea.ExitAltScreen())
	drive(t, m, cmd, printedAll(m))
	got := scrollback()
	if strings.Count(got, "echo: first question") != 1 || strings.Count(got, "echo: second question") != 1 {
		t.Fatalf("scrollback:\n%s", got)
	}
	if cockpit.LoadPreferences(m.opt.preferences).Layout != cockpit.LayoutCompact {
		t.Fatal("the layout was not kept")
	}
}

func TestCompactQuitLeavesTheScrollback(t *testing.T) {
	m, _ := compactModel(t)
	if _, cmd := m.Update(key("ctrl+d")); !isQuit(cmd) || m.View() != "" {
		t.Fatalf("quit: last frame %q", m.View())
	}
}

func TestCompactViewFitsTerminal(t *testing.T) {
	m, _ := compactModel(t)
	runCompact(t, m, "use a tool")
	for _, size := range [][2]int{{24, 8}, {40, 12}, {80, 24}, {160, 50}} {
		m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		for _, overlay := range []string{"", "palette", "help"} {
			m.picker = nil
			if overlay != "" {
				m.openPicker(overlay)
			}
			lines := strings.Split(m.View(), "\n")
			if len(lines) > size[1] {
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

func TestLayoutOptions(t *testing.T) {
	dir := t.TempDir()
	settings := filepath.Join(dir, "config", "settings.json")
	parse := func(args ...string) (options, error) {
		return parseOptions(append([]string{"-workspace", dir, "-config", settings}, args...), io.Discard)
	}
	if o, _ := parse(); o.compact {
		t.Fatal("compact without asking")
	}
	if err := cockpit.SaveLayout(cockpit.PreferencesPath(settings), cockpit.LayoutCompact); err != nil {
		t.Fatal(err)
	}
	for args, want := range map[string]bool{"": true, "-fullscreen": false, "-inline": true, "-compact": true} {
		if o, err := parse(strings.Fields(args)...); err != nil || o.compact != want {
			t.Fatalf("%q: compact %v, %v", args, o.compact, err)
		}
	}
	for _, both := range [][]string{{"-inline", "-fullscreen"}, {"-compact", "-fullscreen"}} {
		if _, err := parse(both...); err == nil {
			t.Fatalf("took both layouts: %v", both)
		}
	}
}
