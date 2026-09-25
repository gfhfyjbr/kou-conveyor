package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func TestPluginCommandsNeedATrustedWorkspace(t *testing.T) {
	m := testModel(t)
	directory := filepath.Join(plugin.WorkspaceDirectory(m.opt.Workspace), "shout")
	os.MkdirAll(directory, 0o755)
	os.WriteFile(filepath.Join(directory, plugin.ManifestName), []byte(`{"name": "shout", "commands": [{"name": "shout", "args": "[text]", "description": "Shout it", "prompt": "Shout {{args}}"}]}`), 0o644)
	m.loadPlugins()
	if m.command("/shout x"); !strings.Contains(m.note.text, "unknown command") {
		t.Fatalf("an untrusted plugin's command ran: %+v", m.note)
	}

	m.command("/plugins")
	if m.picker == nil || m.picker.kind != "plugins" || !strings.HasPrefix(m.picker.items[0].title, "Trust this workspace: run its 1 plugin") {
		t.Fatalf("picker = %+v", m.picker)
	}
	var shout pickerItem
	for _, item := range m.picker.items {
		if strings.HasPrefix(item.title, "shout") {
			shout = item
		}
	}
	if shout.detail != "the workspace is not trusted" || shout.current {
		t.Fatalf("shout = %+v", shout)
	}
	m.Update(key("enter")) // trusts the workspace
	if m.picker != nil || len(m.pluginCommands) != 1 || !strings.Contains(m.note.text, "run from the next prompt") {
		t.Fatalf("after trusting: picker %v, commands %v, note %q", m.picker, m.pluginCommands, m.note.text)
	}

	typeText(m, "/sh")
	m.Update(key("tab"))
	if m.input.Value() != "/shout " {
		t.Fatalf("completion = %q", m.input.Value())
	}
	m.input.Reset()
	drive(t, m, m.command("/shout at the moon"), func() bool { return m.state == idle && m.job == nil })
	if last := m.tr.Entries[len(m.tr.Entries)-1]; last.Text != "echo: Shout at the moon" {
		t.Fatalf("last entry = %#v", last)
	}

	m.command("/plugins untrust")
	if len(m.pluginCommands) != 0 {
		t.Fatal("an untrusted plugin's command stayed")
	}
}

// The cockpit takes up a plugin that comes while it runs: its commands are
// there without a restart.
func TestPluginCommandsFollowChanges(t *testing.T) {
	m := testModel(t)
	if _, ok := m.pluginCommand("/later"); ok {
		t.Fatal("a command before its plugin")
	}
	if cmd := m.pluginsChanged(); cmd != nil {
		t.Fatal("a change reported with nothing changed")
	}
	directory := filepath.Join(plugin.UserDirectory(cockpit.PluginDirectory(m.opt.SettingsFile)), "later")
	os.MkdirAll(directory, 0o755)
	os.WriteFile(filepath.Join(directory, plugin.ManifestName), []byte(`{"name": "later", "commands": [{"name": "later", "description": "Later", "prompt": "Do it later"}]}`), 0o644)
	if cmd := m.pluginsChanged(); cmd == nil {
		t.Fatal("the new plugin went unnoticed")
	}
	if command, ok := m.pluginCommand("/later"); !ok || command.Plugin != "later" {
		t.Fatalf("command = %+v, %v", command, ok)
	}
	os.RemoveAll(directory)
	m.pluginsChanged()
	if _, ok := m.pluginCommand("/later"); ok {
		t.Fatal("the command outlived its plugin")
	}
}
