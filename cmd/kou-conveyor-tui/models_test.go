package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// promptModels are the models the transcript's prompts show, in order.
func promptModels(tr *cockpit.Transcript) string {
	var models []string
	for _, e := range tr.Entries {
		if e.Kind == cockpit.KindUser {
			models = append(models, e.Model)
		}
	}
	return strings.Join(models, ",")
}

// A session's prompts run with the model chosen for it; one queued while the
// agent works keeps the model chosen when it was queued. Each prompt shows
// the model that answered it, and the composer rule the model of the next.
func TestPromptsRunWithTheSessionsModel(t *testing.T) {
	t.Setenv("KOU_CONVEYOR_LLM_MODEL", "")
	m := testModel(t)
	m.command("/model claude-opus-5-5")
	m.input.SetValue("hold at the gate")
	_, cmd := m.Update(key("enter"))
	queued := false
	drive(t, m, cmd, func() bool {
		if !queued && promptPersisted(m) {
			queued = true
			m.command("/model grok-4.7")
			m.input.SetValue("then grok")
			m.Update(key("enter"))
			if item := m.queue().Items[0]; item.Model != "grok-4.7" {
				t.Errorf("queued with %q", item.Model)
			}
			os.WriteFile(filepath.Join(m.opt.Workspace, "gate"), nil, 0o600)
		}
		return m.state == idle && strings.Contains(texts(m.tr), "echo: then grok")
	})
	if got := promptModels(m.tr); got != "claude-opus-5-5,grok-4.7" {
		t.Fatalf("prompts' models = %q", got)
	}
	screen := ansi.Strip(m.View())
	for _, want := range []string{"MODEL grok-4.7", "⇄ grok-4.7", "· claude-opus-5-5"} {
		if !strings.Contains(screen, want) {
			t.Errorf("the screen lacks %q:\n%s", want, screen)
		}
	}
}

// The next prompt runs with the model chosen for the session, else the one
// its last prompt ran with, else -model's, else the connection's.
func TestNextModel(t *testing.T) {
	m := testModel(t)
	if m.nextModel() != "" {
		t.Fatalf("a new session without -model runs %q", m.nextModel())
	}
	m.opt.model = "flag-model"
	if m.nextModel() != "flag-model" {
		t.Fatalf("with -model: %q", m.nextModel())
	}
	m.tr.SubmitModel("11111111-1111-4111-8111-111111111111", "hello", "session-model", time.Now())
	if m.nextModel() != "session-model" {
		t.Fatalf("after a prompt: %q", m.nextModel())
	}
	m.chooseModel("chosen-model")
	if m.nextModel() != "chosen-model" {
		t.Fatalf("chosen: %q", m.nextModel())
	}
	m.chooseModel("two words")
	if m.nextModel() != "chosen-model" {
		t.Fatalf("an ID with a space was chosen: %q", m.nextModel())
	}
	m.sessionID = "another"
	if m.nextModel() != "session-model" {
		t.Fatalf("a choice is the session's alone: %q", m.nextModel())
	}
}

func TestModelsPicker(t *testing.T) {
	m := testModel(t)
	catalog := cockpit.Catalog{Source: "gateway", Default: "claude-opus-5-5", Models: []cockpit.ModelInfo{
		{ID: "claude-opus-5-5", Name: "Claude Opus 5.5", Provider: "anthropic", ProviderName: "Anthropic", Context: 1_000_000},
		{ID: "grok-4.7", Provider: "xai", ProviderName: "xAI", Context: 500_000, Cooling: true},
		{ID: "grok-imagine-video", Provider: "xai", ProviderName: "xAI", Media: true},
	}}
	open := func() {
		t.Helper()
		m.command("/model")
		if m.picker == nil || m.picker.kind != "models" || !m.picker.loading {
			t.Fatalf("picker = %+v", m.picker)
		}
		m.Update(modelsMsg{gen: m.pickerGen, catalog: catalog})
	}
	open()
	if len(m.picker.items) != 3 || m.picker.selected().title != "Claude Opus 5.5  claude-opus-5-5" {
		t.Fatalf("items = %+v, selected %+v", m.picker.items, m.picker.selected())
	}
	if detail := m.picker.items[1].detail; detail != "xAI · 500k · cooling" {
		t.Errorf("grok's detail = %q", detail)
	}
	if detail := m.picker.items[0].detail; detail != "Anthropic · 1M · current" {
		t.Errorf("claude's detail = %q", detail)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("grok-4")})
	m.Update(key("enter"))
	if m.picker != nil || m.nextModel() != "grok-4.7" || m.note.level != "warn" {
		t.Fatalf("picked %q, notice %+v", m.nextModel(), m.note)
	}
	// An ID the connection does not list is typed and taken.
	open()
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("local-model")})
	m.Update(key("enter"))
	if m.picker != nil || m.nextModel() != "local-model" {
		t.Fatalf("typed %q", m.nextModel())
	}
	// A late list for a closed picker changes nothing.
	m.Update(modelsMsg{gen: m.pickerGen - 1, catalog: cockpit.Catalog{}})
	if m.picker != nil {
		t.Fatal("a late list opened the picker")
	}
}

// An edited prompt runs again with the model it ran with, unless a model
// was chosen for the session.
func TestEditedPromptKeepsItsModel(t *testing.T) {
	t.Setenv("KOU_CONVEYOR_LLM_MODEL", "")
	m := testModel(t)
	m.chooseModel("claude-opus-5-5")
	runPrompt(t, m, "first question")
	m.chooseModel("grok-4.7")
	runPrompt(t, m, "second question")
	delete(m.chosen, m.sessionID)
	m.command("/edit 1")
	if m.edit == nil {
		t.Fatal("no edit")
	}
	m.input.SetValue("first question, edited")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle && strings.Contains(texts(m.tr), "echo: first question, edited") })
	if got := promptModels(m.tr); got != "claude-opus-5-5" {
		t.Fatalf("prompts' models after the edit = %q", got)
	}
	// A model chosen for the session wins.
	m.chooseModel("gpt-6-astra")
	m.command("/edit 1")
	m.input.SetValue("first question, again")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle && strings.Contains(texts(m.tr), "echo: first question, again") })
	if got := promptModels(m.tr); got != "gpt-6-astra" {
		t.Fatalf("prompts' models after the second edit = %q", got)
	}
}
