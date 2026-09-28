package main

import (
	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

// /skills lists the skills a run in the workspace has, as the runner finds
// them: the project's, from the workspace's .harness/skills and
// .agents/skills and its plugins, then the system-wide ones, from skills/
// beside the user's plugins, ~/.agents/skills and the other plugins. A
// skill that a skill of the same name replaces is listed as replaced.

// openSkills lists the skills of the workspace.
func (m *uiModel) openSkills() tea.Cmd {
	found := cockpit.Skills(m.opt.SettingsFile, m.opt.Workspace, m.loadPlugins())
	m.closePicker()
	m.picker = newPicker("skills", "skills", "no skills: add them to .harness/skills or .agents/skills here, or to ~/.agents/skills", m.styles)
	var items []pickerItem
	for _, current := range found.Skills {
		items = append(items, pickerItem{
			title: current.Name + " — " + current.Description, detail: skillDetail(current),
			search: current.Directory.Label + " " + current.Path + " " + current.Reason, current: current.Active,
		})
	}
	for _, err := range found.Errors {
		items = append(items, pickerItem{title: "error: " + err.Error()})
	}
	m.picker.setItems(items)
	m.input.Blur()
	return nil
}

// skillDetail says where a skill comes from, or that another replaces it.
func skillDetail(current skill.Skill) string {
	scope := "system"
	if current.Directory.Scope == skill.ScopeProject {
		scope = "project"
	}
	from := current.Directory.Label
	if current.Directory.Kind == skill.KindConfig {
		from = "kou-conveyor"
	}
	switch {
	case !current.Active:
		return "replaced · " + scope
	case current.Manual:
		return "on request · " + scope + " · " + from
	}
	return scope + " · " + from
}
