package main

import (
	"cmp"
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Models. Each prompt runs with a model of its own: any the connection
// reaches, of any provider — through the gateway, every model of every
// account and endpoint it has. The composer rule shows the model the next
// prompt of the session takes: the one chosen for the session (ctrl+p,
// /model), else the one its last prompt ran with, else -model's, else the
// connection's. Every prompt shows the model that answered it.

// nextModel is the model the next prompt of the session runs with; "" is
// the connection's.
func (m *uiModel) nextModel() string {
	if model := m.chosen[m.sessionID]; model != "" {
		return model
	}
	if model := m.tr.LastModel(); model != "" {
		return model
	}
	return m.opt.model
}

// chooseModel chooses the model the next prompts of the session run with.
func (m *uiModel) chooseModel(id string) tea.Cmd {
	id = strings.TrimSpace(id)
	if id == "" || cockpit.ValidModel(id) != nil {
		return m.notify("a model ID has no spaces", "warn")
	}
	if m.chosen == nil {
		m.chosen = make(map[string]string)
	}
	m.chosen[m.sessionID] = id
	text, level := "model of the next prompts: "+id, "info"
	if c := m.catalog; c != nil && c.Source != "" && len(c.Models) != 0 {
		if info, ok := c.Find(id); ok {
			text += " · " + info.ProviderName
			if info.Cooling {
				text, level = text+" — cooling down: its accounts wait out a limit", "warn"
			}
		} else {
			text, level = text+" — the connection does not list it; runs try it anyway", "warn"
		}
	}
	if m.state != idle {
		text += " · the running agent keeps its own"
	}
	return m.notify(text, level)
}

// listModels asks the connection which models it has, for the models
// picker.
func (m *uiModel) listModels() tea.Cmd {
	m.pickerGen++
	gen, ctx, o := m.pickerGen, m.ctx, m.opt
	return func() tea.Msg {
		settings, err := cockpit.LoadSettings(o.SettingsFile)
		if o.SettingsFile == "" || err != nil {
			settings = cockpit.Settings{}
		}
		catalog := cockpit.ListModels(ctx, o.Provider, o.SettingsFile, settings, cockpit.WorkspaceEnv(o.Workspace))
		if err != nil && catalog.Error == "" {
			catalog.Error = err.Error()
		}
		return modelsMsg{gen: gen, catalog: catalog}
	}
}

// modelsListed fills the models picker with what the connection listed.
func (m *uiModel) modelsListed(msg modelsMsg) tea.Cmd {
	catalog := msg.catalog
	m.catalog = &catalog
	if m.picker == nil || m.picker.kind != "models" || msg.gen != m.pickerGen {
		return nil
	}
	switch {
	case catalog.Error != "":
		m.picker.empty = "cannot list models: " + catalog.Error + " — type an ID, enter uses it"
	case catalog.Source == "":
		m.picker.empty = "this connection lists no models — type an ID, enter uses it"
	}
	m.picker.setItems(m.modelItems(catalog))
	m.picker.selectCurrent()
	return nil
}

// modelItems are the models of a catalog as the picker lists them: by
// provider, newest first, those that make media last.
func (m *uiModel) modelItems(catalog cockpit.Catalog) []pickerItem {
	current := cmp.Or(m.nextModel(), catalog.Default)
	items := make([]pickerItem, 0, len(catalog.Models))
	for _, info := range catalog.Models {
		id := info.ID
		title := id
		if info.Name != "" && info.Name != id {
			title = info.Name + "  " + id
		}
		detail := []string{info.ProviderName}
		if info.Context > 0 {
			detail = append(detail, contextSize(info.Context))
		}
		switch {
		case id == current:
			detail = append(detail, "current")
		case id == catalog.Default:
			detail = append(detail, "default")
		}
		if info.Cooling {
			detail = append(detail, "cooling")
		}
		if info.Media {
			detail = append(detail, "media")
		}
		items = append(items, pickerItem{
			title: title, detail: strings.Join(detail, " · "), current: id == current,
			search: strings.Join(append([]string{id, info.Provider, info.ProviderName}, info.Via...), " "),
			action: func(m *uiModel) tea.Cmd {
				m.closePicker()
				return m.chooseModel(id)
			},
		})
	}
	return items
}

// contextSize says how much a context window holds: 200k, 1M.
func contextSize(tokens int64) string {
	if tokens >= 1_000_000 {
		return strings.TrimSuffix(fmt.Sprintf("%.1f", float64(tokens)/1e6), ".0") + "M"
	}
	return fmt.Sprintf("%dk", (tokens+500)/1000)
}

// modelRuleWidth is the narrowest window whose composer controls show the
// model.
const modelRuleWidth = 96

// modelControl renders the model the next prompt runs with, which the
// composer's controls show before the effort meter; "" where there is no
// room.
func (m *uiModel) modelControl() string {
	if m.stageWidth() < modelRuleWidth {
		return ""
	}
	st := m.styles
	model, style := m.nextModel(), st.text
	if _, chosen := m.chosen[m.sessionID]; chosen {
		style = st.accent
	}
	if model == "" {
		model, style = orDefault(m.conn.Model, "runner default"), st.ghost
	}
	return st.label.Render("MODEL") + " " + m.modelDot(model) + style.Render(ansi.Truncate(model, 34, "…")) + " " + st.ghost.Render("▾")
}

// previousModel is the model of the prompt before the one with the given ID.
func (m *uiModel) previousModel(id string) string {
	model := ""
	for _, e := range m.tr.Entries {
		if e.ID == id {
			return model
		}
		if e.Kind == cockpit.KindUser && e.Model != "" && e.State != cockpit.Undelivered {
			model = e.Model
		}
	}
	return model
}

// runModel is the model of the run going on, as its prompt says.
func (m *uiModel) runModel() string {
	if e := m.tr.Entry("input:" + m.runMessage); e != nil {
		return e.Model
	}
	return ""
}

// runSummary says where the next prompt goes: the connection, and the model
// it runs with.
func (m *uiModel) runSummary() string {
	provider := m.conn.Provider
	if m.conn.Source == "gateway" {
		provider = "gateway"
	}
	parts := []string{orDefault(provider, "runner default")}
	if model := orDefault(m.nextModel(), m.conn.Model); model != "" {
		parts = append(parts, model)
	}
	return strings.Join(parts, " · ")
}
