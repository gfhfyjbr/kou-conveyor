package main

import (
	"io"
	"os"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func TestCompactCommandCompactsTheConversation(t *testing.T) {
	m := testModel(t)
	if m.command("/compact"); m.job != nil || !strings.Contains(m.note.text, "nothing to compact yet") {
		t.Fatalf("compacted a new session: %+v", m.note)
	}
	runPrompt(t, m, "first question")
	if !paletteHas(m, "Compact the context") {
		t.Fatal("the palette does not offer compaction")
	}

	drive(t, m, m.command("/compact the release notes"), func() bool { return m.state == idle && m.job == nil })
	last := m.tr.Entries[len(m.tr.Entries)-1]
	if last.Kind != cockpit.KindNotice || last.Text != "Context compacted from 152k tokens" ||
		last.Detail != "Summary of the session, focused on the release notes" {
		t.Fatalf("last entry = %#v", last)
	}
	if !strings.HasPrefix(m.note.text, "context compacted") || m.last.kind != "done" || m.tr.Interrupted() {
		t.Fatalf("note %+v, outcome %q, interrupted %v", m.note, m.last.kind, m.tr.Interrupted())
	}
	if kinds(m.tr) != "user,assistant,notice" {
		t.Fatalf("transcript = %s", kinds(m.tr))
	}

	// Nothing new to summarize until the agent answers again.
	if m.command("/compact"); m.job != nil || !strings.Contains(m.note.text, "has not answered since the last compaction") {
		t.Fatalf("compacted the summary: %+v", m.note)
	}
	runPrompt(t, m, "second question")
	if last := m.tr.Entries[len(m.tr.Entries)-1]; last.Text != "echo: second question" || !m.tr.Compactable() {
		t.Fatalf("after the compaction: %s", kinds(m.tr))
	}
}

func TestCompactCommandReportsAnEmptySummary(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	drive(t, m, m.command("/compact nothing"), func() bool { return m.state == idle && m.job == nil })
	if m.note.level != "warn" || m.note.text != "Context not compacted: the model wrote no summary" {
		t.Fatalf("note = %+v", m.note)
	}
}

func TestCompactionCanBeStopped(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	cmd := m.command("/compact wait")
	if m.state != running || m.tr.Activity != "Compacting context" || !strings.Contains(m.statusLine(), "Compacting context") {
		t.Fatalf("state %v, activity %q", m.state, m.tr.Activity)
	}
	path := cockpit.SessionPath(m.opt.SessionDir, m.sessionID)
	drive(t, m, cmd, func() bool {
		data, _ := os.ReadFile(path)
		return strings.Contains(string(data), `"Type":"compaction"`)
	})
	drive(t, m, tea.Batch(m.stop(), waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
	if m.last.kind != "stopped" || m.tr.Interrupted() || !m.tr.Compactable() {
		t.Fatalf("outcome %q, interrupted %v", m.last.kind, m.tr.Interrupted())
	}
	// The conversation was not compacted: it can be, still.
	drive(t, m, m.command("/compact"), func() bool { return m.state == idle && m.job == nil })
	if last := m.tr.Entries[len(m.tr.Entries)-1]; last.Text != "Context compacted from 152k tokens" {
		t.Fatalf("last entry = %#v", last)
	}
}

func TestInlineCommandSwitchesLayout(t *testing.T) {
	m := testModel(t)
	m.command("/inline")
	if !m.compact || m.note.text != "inline · ^F switches back" {
		t.Fatalf("compact %v, note %q", m.compact, m.note.text)
	}
	m.command("/fullscreen")
	if m.compact {
		t.Fatal("still inline")
	}
}

func paletteHas(m *uiModel, title string) bool {
	for _, item := range m.paletteItems() {
		if item.title == title {
			return true
		}
	}
	return false
}

func TestHeadlessCompaction(t *testing.T) {
	workspace := t.TempDir()
	o := options{thinking: "high", historyFile: workspace + "/history.json"}
	o.Workspace, o.SessionDir, o.Heartbeat, o.Runner = workspace, workspace+"/sessions", time.Minute, cockpittest.Runner(t)

	o.prompt = "/compact"
	if err := runHeadless(t.Context(), o, io.Discard, io.Discard); err == nil || !strings.Contains(err.Error(), "-session") {
		t.Fatalf("compacted without a session: %v", err)
	}
	o.session = "headless"
	if err := runHeadless(t.Context(), o, io.Discard, io.Discard); err == nil || !strings.Contains(err.Error(), "does not exist") {
		t.Fatalf("compacted a missing session: %v", err)
	}

	o.prompt = "first question"
	var out strings.Builder
	if err := runHeadless(t.Context(), o, &out, io.Discard); err != nil || out.String() != "echo: first question\n" {
		t.Fatalf("prompt: %q, %v", out.String(), err)
	}
	o.prompt = "/compact the API"
	out.Reset()
	var diagnostics strings.Builder
	if err := runHeadless(t.Context(), o, &out, &diagnostics); err != nil {
		t.Fatal(err)
	}
	if out.String() != "Summary of the session, focused on the API\n" || diagnostics.String() != "Context compacted from 152k tokens\n" {
		t.Fatalf("out %q, diagnostics %q", out.String(), diagnostics.String())
	}
	history, _ := loadHistory(o.historyFile)
	if len(history) != 1 || history[0].Prompt != "first question" {
		t.Fatalf("history = %+v", history)
	}
}

func TestCompactCommandParsing(t *testing.T) {
	for prompt, want := range map[string]string{"/compact": "", " /compact  the API ": "the API", "/compact\tlogs": "logs"} {
		if focus, ok := compactCommand(prompt); !ok || focus != want {
			t.Errorf("compactCommand(%q) = %q, %v", prompt, focus, ok)
		}
	}
	for _, prompt := range []string{"/compactify", "please /compact", "compact", ""} {
		if _, ok := compactCommand(prompt); ok {
			t.Errorf("compactCommand(%q) matched", prompt)
		}
	}
}
