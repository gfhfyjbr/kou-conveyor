package plugin

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
)

// CoreName is the built-in plugin that gives the agent the harness's own
// tools: Bash, ViewImage and SkillUse.
const CoreName = "core"

// Builtins are the plugins compiled into the harness.
func Builtins() []Plugin {
	return []Plugin{{
		Manifest: Manifest{
			Name:        CoreName,
			Description: "The harness's own tools: Bash runs shell commands, ViewImage shows the model images, and SkillUse loads skills.",
			Tools: []Tool{
				{Name: "Bash", Description: "Execute a shell command in background."},
				{Name: "ViewImage", Description: "View a local JPEG, PNG, BMP, TIFF, or WebP image."},
				{Name: "SkillUse", Description: "Load the instructions for a registered skill."},
			},
		},
		Source: SourceBuiltin,
	}}
}

// Environment variables of the plugin configuration.
const (
	// ConfigEnvironment is the cockpits' settings file; plugins and their
	// settings live beside it.
	ConfigEnvironment = "KOU_CONVEYOR_CONFIG"
	// TrustEnvironment set to 1 trusts the workspace's plugins for one run.
	TrustEnvironment = "KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS"
	// DisabledEnvironment lists plugins, by name and separated by commas,
	// that do not run.
	DisabledEnvironment = "KOU_CONVEYOR_DISABLED_PLUGINS"
)

// ConfigDirectory is where the user's plugins and plugins.json live: beside
// the cockpits' settings, in the user configuration directory.
func ConfigDirectory(getenv func(string) string) (string, error) {
	if path := strings.TrimSpace(getenv(ConfigEnvironment)); path != "" {
		return filepath.Dir(path), nil
	}
	directory, err := os.UserConfigDir()
	if err != nil {
		return "", fmt.Errorf("find the configuration directory: %w; set %s", err, ConfigEnvironment)
	}
	return filepath.Join(directory, "kou-conveyor"), nil
}

// WorkspaceDirectory is where a workspace keeps its plugins.
func WorkspaceDirectory(workspace string) string {
	return filepath.Join(workspace, ".harness", "plugins")
}

// Settings is plugins.json in the configuration directory: the workspaces
// whose plugins the user trusts, and the plugins the user turned off.
type Settings struct {
	Trusted  []string `json:"trusted,omitzero"`
	Disabled []string `json:"disabled,omitzero"`
}

func settingsPath(directory string) string { return filepath.Join(directory, "plugins.json") }

// SettingsPath is plugins.json in the configuration directory.
func SettingsPath(directory string) string { return settingsPath(directory) }

// UserDirectory is where the user's plugins live in the configuration
// directory.
func UserDirectory(configDirectory string) string { return filepath.Join(configDirectory, "plugins") }

// LoadSettings reads plugins.json; a missing file holds the defaults.
func LoadSettings(directory string) (Settings, error) {
	var settings Settings
	encoded, err := os.ReadFile(settingsPath(directory))
	if errors.Is(err, fs.ErrNotExist) {
		return settings, nil
	}
	if err != nil {
		return settings, fmt.Errorf("read plugin settings: %w", err)
	}
	if err := json.Unmarshal(encoded, &settings); err != nil {
		return Settings{}, fmt.Errorf("read plugin settings %s: %w", settingsPath(directory), err)
	}
	return settings, nil
}

// SaveSettings writes plugins.json, replacing it whole.
func SaveSettings(directory string, settings Settings) error {
	settings.Trusted = compact(settings.Trusted)
	settings.Disabled = compact(settings.Disabled)
	encoded, err := json.Marshal(settings, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return err
	}
	temporary := fmt.Sprintf("%s.%d.tmp", settingsPath(directory), time.Now().UnixNano())
	if err := os.WriteFile(temporary, append(encoded, '\n'), 0o600); err != nil {
		return err
	}
	if err := os.Rename(temporary, settingsPath(directory)); err != nil {
		os.Remove(temporary)
		return err
	}
	return nil
}

func compact(values []string) []string {
	values = slices.Clone(values)
	slices.Sort(values)
	return slices.Compact(values)
}

// Trusts reports whether the workspace's plugins may run.
func (settings Settings) Trusts(workspace string) bool {
	return slices.Contains(settings.Trusted, filepath.Clean(workspace))
}

// Trust records whether the workspace's plugins may run.
func (settings *Settings) Trust(workspace string, trusted bool) {
	workspace = filepath.Clean(workspace)
	settings.Trusted = slices.DeleteFunc(settings.Trusted, func(path string) bool { return path == workspace })
	if trusted {
		settings.Trusted = append(settings.Trusted, workspace)
	}
}

// Disable records whether a plugin is turned off.
func (settings *Settings) Disable(name string, disabled bool) {
	settings.Disabled = slices.DeleteFunc(settings.Disabled, func(other string) bool { return other == name })
	if disabled {
		settings.Disabled = append(settings.Disabled, name)
	}
}

// Options say which plugins to find.
type Options struct {
	// ConfigDirectory holds the user's plugins, in plugins/, and plugins.json.
	// Empty leaves the user's plugins out.
	ConfigDirectory string
	// Workspace is the folder the agent works in; its plugins are in
	// .harness/plugins.
	Workspace string
	// TrustWorkspace runs the workspace's plugins even when plugins.json does
	// not trust the workspace.
	TrustWorkspace bool
	// Disabled names plugins that do not run, besides those plugins.json
	// turns off.
	Disabled []string
	// Builtins are built-in plugins the program brings besides the
	// harness's own, such as the browser cockpit's.
	Builtins []Plugin
}

// Found is what Discover found.
type Found struct {
	// Plugins lists every plugin found, active or not, in load order.
	Plugins []Plugin
	// Errors are the plugins that could not be read.
	Errors []error
	// Trusted says whether the workspace's plugins may run.
	Trusted  bool
	Settings Settings
}

// Active are the plugins that run.
func (found Found) Active() []Plugin {
	var active []Plugin
	for _, current := range found.Plugins {
		if current.Active {
			active = append(active, current)
		}
	}
	return active
}

// Discover finds the plugins for a workspace: the built-in ones, the
// user's, then the workspace's. A plugin replaces an earlier one of the same
// name.
func Discover(options Options) Found {
	var found Found
	var settings Settings
	if options.ConfigDirectory != "" {
		var err error
		if settings, err = LoadSettings(options.ConfigDirectory); err != nil {
			found.Errors = append(found.Errors, err)
		}
	}
	found.Settings = settings
	found.Trusted = options.TrustWorkspace || options.Workspace != "" && settings.Trusts(options.Workspace)
	disabled := append(slices.Clone(settings.Disabled), options.Disabled...)

	plugins := append(Builtins(), options.Builtins...)
	if options.ConfigDirectory != "" {
		user, problems := ReadDirectory(UserDirectory(options.ConfigDirectory), SourceUser)
		plugins, found.Errors = append(plugins, user...), append(found.Errors, problems...)
	}
	if options.Workspace != "" {
		workspace, problems := ReadDirectory(WorkspaceDirectory(options.Workspace), SourceWorkspace)
		plugins, found.Errors = append(plugins, workspace...), append(found.Errors, problems...)
	}
	for index := range plugins {
		current := &plugins[index]
		current.Active = true
		switch {
		case slices.Contains(disabled, current.Name):
			current.Active, current.Reason = false, "turned off"
		case current.Source == SourceWorkspace && !found.Trusted:
			current.Active, current.Reason = false, "the workspace is not trusted"
		}
		// A later plugin of the same name that runs replaces an earlier one.
		for later := index + 1; later < len(plugins); later++ {
			replacing := plugins[later]
			if replacing.Name != current.Name || replacing.Source == SourceWorkspace && !found.Trusted {
				continue
			}
			if current.Active {
				current.Active, current.Reason = false, fmt.Sprintf("replaced by the %s plugin of the same name", replacing.Source)
			}
			break
		}
	}
	found.Errors = append(found.Errors, resolveConflicts(plugins)...)
	found.Plugins = plugins
	return found
}

// resolveConflicts leaves out the tools and commands an active plugin
// declares after another active plugin did, and reports them.
func resolveConflicts(plugins []Plugin) []error {
	var problems []error
	tools, commands := map[string]string{}, map[string]string{}
	for index := range plugins {
		current := &plugins[index]
		if !current.Active {
			continue
		}
		current.Tools = slices.DeleteFunc(slices.Clone(current.Tools), func(definition Tool) bool {
			if owner, taken := tools[definition.Name]; taken {
				problems = append(problems, fmt.Errorf("plugin %q: tool %q is %q's already, so it is left out", current.Name, definition.Name, owner))
				return true
			}
			tools[definition.Name] = current.Name
			return false
		})
		current.Commands = slices.DeleteFunc(slices.Clone(current.Commands), func(command Command) bool {
			if owner, taken := commands[command.Name]; taken {
				problems = append(problems, fmt.Errorf("plugin %q: command /%s is %q's already, so it is left out", current.Name, command.Name, owner))
				return true
			}
			commands[command.Name] = current.Name
			return false
		})
	}
	return problems
}
