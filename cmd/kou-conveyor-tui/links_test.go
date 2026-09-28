package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// linkModel is a cockpit whose workspace has files to link.
func linkModel(t *testing.T) *uiModel {
	t.Helper()
	m := testModel(t)
	for name, content := range map[string]string{
		"README.md": "# Title\n", "cmd/main.go": "package main\n\nfunc main() {}\n", "my notes.txt": "a note\n",
	} {
		path := filepath.Join(m.opt.Workspace, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return m
}

// typeLink types text and waits for the list of what the reference at the
// cursor completes to.
func typeLink(t *testing.T, m *uiModel, text string) {
	t.Helper()
	_, cmd := m.Update(runes(text))
	drive(t, m, cmd, func() bool { return m.linksShown() })
}

// pressKey delivers a key and runs what it asks for until done.
func pressKey(t *testing.T, m *uiModel, k string, done func() bool) {
	t.Helper()
	_, cmd := m.Update(key(k))
	if done != nil {
		drive(t, m, cmd, done)
	}
}

func listed(m *uiModel) []string {
	var paths []string
	for _, item := range m.links.items {
		paths = append(paths, item.Path)
	}
	return paths
}

func TestDollarListsTheFilesOfTheWorkspace(t *testing.T) {
	m := linkModel(t)
	typeLink(t, m, "look at $READ")
	if got := listed(m); len(got) == 0 || got[0] != "README.md" {
		t.Fatalf("listed %q", got)
	}
	// It shows at the transcript's foot, over the composer's columns.
	shown := frame(t, m)
	if !strings.Contains(shown, "┌─ FILES 1") || !strings.Contains(shown, "▌▤ README.md") || !strings.Contains(shown, "tab completes") {
		t.Fatalf("frame:\n%s", shown)
	}
	// Tab completes; the tray says what the text links.
	pressKey(t, m, "tab", nil)
	if m.input.Value() != "look at $README.md " || m.linksShown() {
		t.Fatalf("composer %q, list shown %v", m.input.Value(), m.linksShown())
	}
	if shown := frame(t, m); !strings.Contains(shown, "FILES 1 ▤ $README.md 8 B") {
		t.Fatalf("frame:\n%s", shown)
	}
}

func TestAFolderCompletedListsWhatIsInIt(t *testing.T) {
	m := linkModel(t)
	typeLink(t, m, "$cm")
	_, cmd := m.Update(key("tab"))
	drive(t, m, cmd, func() bool { return m.linksShown() && len(m.links.items) == 1 })
	if m.input.Value() != "$cmd/" || listed(m)[0] != "cmd/main.go" {
		t.Fatalf("composer %q, listed %q", m.input.Value(), listed(m))
	}
	pressKey(t, m, "tab", nil)
	if m.input.Value() != "$cmd/main.go " {
		t.Fatalf("composer %q", m.input.Value())
	}
}

func TestAPathWithSpacesCompletesQuoted(t *testing.T) {
	m := linkModel(t)
	typeLink(t, m, "read $my")
	pressKey(t, m, "tab", nil)
	if m.input.Value() != `read $"my notes.txt" ` {
		t.Fatalf("composer %q", m.input.Value())
	}
	if links := m.composerLinks(); len(links) != 1 || links[0].Path != "my notes.txt" {
		t.Fatalf("links = %+v", links)
	}
}

func TestEnterRunsThePromptUnlessAChoiceWasMade(t *testing.T) {
	m := linkModel(t)
	typeLink(t, m, "see $READ")
	// Down and up: a choice, which enter completes.
	pressKey(t, m, "down", nil)
	pressKey(t, m, "up", nil)
	pressKey(t, m, "enter", nil)
	if m.input.Value() != "see $README.md " || m.state != idle {
		t.Fatalf("composer %q, state %v", m.input.Value(), m.state)
	}
	// Typed out, the prompt runs as it is with the list showing.
	typeLink(t, m, "and $cmd/main.g")
	_, cmd := m.Update(key("enter"))
	if m.state != running || m.input.Value() != "" {
		t.Fatalf("state %v, composer %q", m.state, m.input.Value())
	}
	drive(t, m, cmd, func() bool { return m.state == idle })
	// The list went with the text: ↑ walks the history again.
	pressKey(t, m, "up", nil)
	if m.input.Value() != "see $README.md and $cmd/main.g" {
		t.Fatalf("composer %q", m.input.Value())
	}
}

func TestEscClosesTheListAndNothingElse(t *testing.T) {
	m := linkModel(t)
	typeLink(t, m, "$READ")
	press(m, key("esc")) // a lone esc is held back a moment
	if m.linksShown() || !m.escArmed.IsZero() || m.input.Value() != "$READ" {
		t.Fatalf("list shown %v, esc armed %v, composer %q", m.linksShown(), !m.escArmed.IsZero(), m.input.Value())
	}
	// A $ that names nothing lists nothing.
	m.input.SetValue("")
	_, cmd := m.Update(runes("echo $HOME"))
	drive(t, m, cmd, func() bool { return m.links.seq > 0 })
	if m.linksShown() || len(m.composerLinks()) != 0 {
		t.Fatalf("listed %q", listed(m))
	}
}

func TestPromptsShowWhatTheModelSawOfTheirFiles(t *testing.T) {
	m := linkModel(t)
	runPrompt(t, m, "summarize $cmd/main.go:3 and $README.md")
	var prompt *cockpit.Entry
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser {
			prompt = e
		}
	}
	if prompt == nil || len(prompt.Files) != 2 {
		t.Fatalf("prompt = %+v", prompt)
	}
	shown := frame(t, m)
	if !strings.Contains(shown, "▤ $cmd/main.go:3 line 3 of 3") || !strings.Contains(shown, "3 · ▤ $README.md all 1 line ▸") {
		t.Fatalf("frame:\n%s", shown)
	}
	// A click on the files shows what the model saw of them.
	m.view.GotoTop()
	var span entrySpan
	for _, s := range m.spans {
		if s.id == prompt.ID {
			span = s
		}
	}
	if span.files == 0 {
		t.Fatalf("span = %+v", span)
	}
	cmd := click(m, 20, m.top()+span.start+span.files-m.view.YOffset)
	drive(t, m, cmd, func() bool { _, ok := m.linked[m.linkedKey(prompt)]; return ok })
	shown = frame(t, m)
	if !m.isOpen(prompt) || !strings.Contains(shown, "▤ cmd/main.go · as the model saw it · line 3 of 3") || !strings.Contains(shown, "3  func main() {}") {
		t.Fatalf("frame:\n%s", shown)
	}
	// The prompt's header still edits it.
	if e := m.promptHeaderAt(span.start); e != prompt {
		t.Fatalf("the header is %+v", e)
	}
	_ = tea.Quit
}

func TestCtrlOShowsWhatTheModelSawOfPromptsToCome(t *testing.T) {
	m := linkModel(t)
	pressKey(t, m, "ctrl+o", nil)
	m.input.SetValue("read $README.md")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool {
		for _, e := range m.tr.Entries {
			if _, ok := m.linked[m.linkedKey(e)]; ok && e.Kind == cockpit.KindUser {
				return m.state == idle
			}
		}
		return false
	})
	if shown := frame(t, m); !strings.Contains(shown, "▤ README.md · as the model saw it · all 1 line") || !strings.Contains(shown, "1  # Title") {
		t.Fatalf("frame:\n%s", shown)
	}
}
