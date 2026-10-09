package cockpit

import (
	"os"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

// Skills, as the cockpits see them: those a run in the workspace has — the
// project's, in its .harness/skills and .agents/skills, and the
// system-wide ones, in ~/.agents/skills and skills/ beside the user's
// plugins — found as the runner finds them, which Start points at the same
// configuration directory.

// SkillOptions are where finding the skills of a workspace looks, besides
// its plugins.
func SkillOptions(settingsFile, workspace string) skill.Options {
	home, err := os.UserHomeDir()
	if err != nil {
		home = ""
	}
	return skill.Options{Workspace: workspace, Home: home, ConfigDirectory: PluginDirectory(settingsFile)}
}

// Skills finds the skills of a workspace, with those of the active plugins
// found for it.
func Skills(settingsFile, workspace string, plugins plugin.Found) skill.Found {
	options := SkillOptions(settingsFile, workspace)
	options.Plugins = plugins.Active()
	return skill.Discover(options)
}
