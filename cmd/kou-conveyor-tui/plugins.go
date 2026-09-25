package main

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Plugins add slash commands to the terminal cockpit too: those their
// manifests declare, which send a prompt. Their tools, skills and
// instructions reach the agent through the runner, which follows them as
// they change; their web part is the browser cockpit's. The cockpit follows
// them too: every couple of seconds it looks at where they come from, and
// takes up their commands again when that changed.

// loadPlugins finds the workspace's plugins and takes up their commands.
func (m *uiModel) loadPlugins() plugin.Found {
	m.pluginsSeen = m.pluginFingerprint()
	found := cockpit.Plugins(m.opt.SettingsFile, m.opt.Workspace)
	m.pluginCommands = cockpit.PluginCommands(found)
	return found
}

func (m *uiModel) pluginFingerprint() string {
	return plugin.Fingerprint(plugin.Sources(cockpit.PluginOptions(m.opt.SettingsFile, m.opt.Workspace, nil))...)
}

// pluginsChanged takes up plugins that came, changed or went since they
// were last read, and says so when their commands changed.
func (m *uiModel) pluginsChanged() tea.Cmd {
	if m.pluginFingerprint() == m.pluginsSeen {
		return nil
	}
	before := pluginCommandNames(m.pluginCommands)
	found := m.loadPlugins()
	after := pluginCommandNames(m.pluginCommands)
	if before == after {
		return nil
	}
	if after == "" {
		after = "none"
	}
	return m.notify(fmt.Sprintf("plugins changed: %d active · commands %s", len(found.Active()), after), "info")
}

func pluginCommandNames(commands []cockpit.PluginCommand) string {
	names := make([]string, 0, len(commands))
	for _, command := range commands {
		names = append(names, "/"+command.Name)
	}
	return strings.Join(names, " ")
}

// pluginCommand returns the plugin command of that name.
func (m *uiModel) pluginCommand(name string) (cockpit.PluginCommand, bool) {
	for _, command := range m.pluginCommands {
		if "/"+command.Name == name {
			return command, true
		}
	}
	return cockpit.PluginCommand{}, false
}

// plugins handles /plugins: the list, or trusting the workspace's plugins.
func (m *uiModel) plugins(arg string) tea.Cmd {
	switch arg {
	case "":
		return m.openPlugins()
	case "trust", "untrust":
		trusted := arg == "trust"
		if err := cockpit.TrustPlugins(m.opt.SettingsFile, m.opt.Workspace, trusted); err != nil {
			return m.notify("cannot change the trust: "+err.Error(), "error")
		}
		found := m.loadPlugins()
		if trusted {
			return m.notify(fmt.Sprintf("the workspace's plugins run from the next prompt: %d active", len(found.Active())), "info")
		}
		return m.notify("the workspace's plugins are off", "info")
	case "reload":
		found := m.loadPlugins()
		return m.notify(fmt.Sprintf("%d plugins active, %d commands", len(found.Active()), len(m.pluginCommands)), "info")
	}
	return m.notify("/plugins takes trust, untrust or reload", "warn")
}

// openPlugins lists the plugins of the workspace.
func (m *uiModel) openPlugins() tea.Cmd {
	found := m.loadPlugins()
	m.closePicker()
	m.picker = newPicker("plugins", "plugins", "no plugins", m.styles)
	var items []pickerItem
	workspacePlugins := 0
	for _, current := range found.Plugins {
		if current.Source == plugin.SourceWorkspace {
			workspacePlugins++
		}
	}
	switch {
	case workspacePlugins > 0 && !found.Trusted:
		items = append(items, pickerItem{
			title: fmt.Sprintf("Trust this workspace: run its %d %s", workspacePlugins, orPlural(workspacePlugins, "plugin", "plugins")), detail: "/plugins trust",
			action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.plugins("trust") },
		})
	case workspacePlugins > 0:
		items = append(items, pickerItem{
			title: "Stop trusting this workspace's plugins", detail: "/plugins untrust",
			action: func(m *uiModel) tea.Cmd { m.closePicker(); return m.plugins("untrust") },
		})
	}
	for _, current := range found.Plugins {
		var adds []string
		if n := len(current.Tools); n > 0 {
			adds = append(adds, fmt.Sprintf("%d %s", n, orPlural(n, "tool", "tools")))
		}
		for _, command := range current.Commands {
			adds = append(adds, "/"+command.Name)
		}
		if current.Skills != "" {
			adds = append(adds, "skills")
		}
		if current.Instructions != "" {
			adds = append(adds, "instructions")
		}
		if current.Web != nil {
			adds = append(adds, "web")
		}
		state := strings.Join(adds, " · ")
		if !current.Active {
			state = current.Reason
		}
		title := current.Name
		if current.Version != "" {
			title += " " + current.Version
		}
		items = append(items, pickerItem{
			title: title + " (" + string(current.Source) + ")", detail: state,
			search: current.Description + " " + current.Directory, current: current.Active,
		})
	}
	for _, err := range found.Errors {
		items = append(items, pickerItem{title: "error: " + err.Error()})
	}
	m.picker.setItems(items)
	m.input.Blur()
	return nil
}

func orPlural(n int, one, many string) string {
	if n == 1 {
		return one
	}
	return many
}
