package cockpit

import (
	"os"
	"path/filepath"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Plugins, as the cockpits see them: the ones a run in the workspace finds,
// with the user's in the configuration directory beside the settings. The
// runner finds the same ones, as Start points it at that directory.

// PluginDirectory is the configuration directory of the cockpit's plugins:
// the directory of the settings file, else the default one.
func PluginDirectory(settingsFile string) string {
	if settingsFile != "" {
		return filepath.Dir(settingsFile)
	}
	directory, err := plugin.ConfigDirectory(os.Getenv)
	if err != nil {
		return ""
	}
	return directory
}

// Plugins finds the plugins of a workspace.
func Plugins(settingsFile, workspace string) plugin.Found {
	return PluginsWith(settingsFile, workspace, nil)
}

// PluginsWith finds the plugins of a workspace, with the built-in plugins
// the cockpit brings besides the harness's own.
func PluginsWith(settingsFile, workspace string, builtins []plugin.Plugin) plugin.Found {
	return plugin.Discover(PluginOptions(settingsFile, workspace, builtins))
}

// PluginOptions are what finding the plugins of a workspace reads.
func PluginOptions(settingsFile, workspace string, builtins []plugin.Plugin) plugin.Options {
	return plugin.Options{ConfigDirectory: PluginDirectory(settingsFile), Workspace: workspace, Builtins: builtins}
}

// TrustPlugins records whether the workspace's plugins may run.
func TrustPlugins(settingsFile, workspace string, trusted bool) error {
	directory := PluginDirectory(settingsFile)
	settings, err := plugin.LoadSettings(directory)
	if err != nil {
		return err
	}
	settings.Trust(workspace, trusted)
	return plugin.SaveSettings(directory, settings)
}

// DisablePlugin records whether a plugin is turned off.
func DisablePlugin(settingsFile, name string, disabled bool) error {
	directory := PluginDirectory(settingsFile)
	settings, err := plugin.LoadSettings(directory)
	if err != nil {
		return err
	}
	settings.Disable(name, disabled)
	return plugin.SaveSettings(directory, settings)
}

// PluginCommand is a slash command a plugin adds.
type PluginCommand struct {
	plugin.Command
	Plugin string `json:"plugin"`
}

// PluginCommands lists the slash commands of the active plugins.
func PluginCommands(found plugin.Found) []PluginCommand {
	var commands []PluginCommand
	for _, current := range found.Active() {
		for _, command := range current.Commands {
			commands = append(commands, PluginCommand{Command: command, Plugin: current.Name})
		}
	}
	return commands
}
