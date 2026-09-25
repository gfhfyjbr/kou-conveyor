package agentrunner

import (
	"encoding/json/v2"
	"fmt"
	"io"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// discoverPlugins finds the plugins of a run: the built-in ones, the user's
// when the runner has a configuration directory, and the workspace's when
// the workspace is trusted there or KOU_CONVEYOR_TRUST_WORKSPACE_PLUGINS is
// 1. KOU_CONVEYOR_DISABLED_PLUGINS turns plugins off by name.
func discoverPlugins(config Config, getenv func(string) string, workspace string) plugin.Found {
	options, problems := pluginOptions(config, getenv, workspace)
	found := plugin.Discover(options)
	found.Errors = append(problems, found.Errors...)
	return found
}

// pluginOptions says where a run finds its plugins.
func pluginOptions(config Config, getenv func(string) string, workspace string) (plugin.Options, []error) {
	options := plugin.Options{
		Workspace:      workspace,
		TrustWorkspace: strings.TrimSpace(getenv(plugin.TrustEnvironment)) == "1",
	}
	for _, name := range strings.Split(getenv(plugin.DisabledEnvironment), ",") {
		if name = strings.TrimSpace(name); name != "" {
			options.Disabled = append(options.Disabled, name)
		}
	}
	var problems []error
	if config.PluginConfigDirectory != nil {
		directory, err := config.PluginConfigDirectory(getenv)
		if err != nil {
			problems = append(problems, err)
		}
		options.ConfigDirectory = directory
	}
	return options, problems
}

// activePlugin reports whether a plugin of that name runs.
func activePlugin(found plugin.Found, name string) bool {
	return slices.ContainsFunc(found.Active(), func(current plugin.Plugin) bool { return current.Name == name })
}

// pluginSkills lists the skills the active plugins bring.
func pluginSkills(found plugin.Found) ([]tool.Skill, []error) {
	var skills []tool.Skill
	var problems []error
	for _, current := range found.Active() {
		if current.Skills == "" {
			continue
		}
		directory, err := current.Resolve(current.Skills)
		if err != nil {
			problems = append(problems, fmt.Errorf("plugin %q: %w", current.Name, err))
			continue
		}
		found, errors := tool.DiscoverSkills(directory)
		skills, problems = append(skills, found...), append(problems, errors...)
	}
	return skills, problems
}

// pluginInstructions is what the active plugins add to the system prompt.
func pluginInstructions(found plugin.Found) string {
	var parts []string
	for _, current := range found.Active() {
		if current.Instructions != "" {
			parts = append(parts, "## "+current.Name+" plugin\n\n"+current.Instructions)
		}
	}
	return strings.Join(parts, "\n\n")
}

// listedPlugin is a line of -list-plugins.
type listedPlugin struct {
	Name        string   `json:"name"`
	Version     string   `json:"version,omitzero"`
	Description string   `json:"description,omitzero"`
	Source      string   `json:"source"`
	Directory   string   `json:"directory,omitzero"`
	Active      bool     `json:"active"`
	Reason      string   `json:"reason,omitzero"`
	Tools       []string `json:"tools,omitzero"`
	Commands    []string `json:"commands,omitzero"`
}

// listPlugins prints the plugins a run in the workspace would find, one JSON
// object per line, and then those that could not be read.
func listPlugins(output io.Writer, found plugin.Found) error {
	for _, current := range found.Plugins {
		line := listedPlugin{
			Name: current.Name, Version: current.Version, Description: current.Description, Source: string(current.Source),
			Directory: current.Directory, Active: current.Active, Reason: current.Reason,
		}
		for _, definition := range current.Tools {
			line.Tools = append(line.Tools, definition.Name)
		}
		for _, command := range current.Commands {
			line.Commands = append(line.Commands, "/"+command.Name)
		}
		if err := json.MarshalWrite(output, line); err != nil {
			return err
		}
		if _, err := fmt.Fprintln(output); err != nil {
			return err
		}
	}
	for _, problem := range found.Errors {
		if err := json.MarshalWrite(output, map[string]string{"error": problem.Error()}); err != nil {
			return err
		}
		if _, err := fmt.Fprintln(output); err != nil {
			return err
		}
	}
	return nil
}
