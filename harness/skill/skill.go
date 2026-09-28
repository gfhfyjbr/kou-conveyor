// Package skill finds the skills an agent can load. A skill is a directory
// with a SKILL.md, whose frontmatter names it and says when it helps; the
// agent's system prompt lists the skills, and SkillUse loads one's file when
// a task matches it. Skills come in two scopes:
//
//   - The project's, which the agent has only while it works in the
//     workspace that holds them: .harness/skills, kou-conveyor's own, and
//     .agents/skills, the directory other agents share, in the workspace;
//     and the skills of the workspace's plugins.
//   - The system's, for every workspace: skills/ in kou-conveyor's
//     configuration directory (beside plugins/), ~/.agents/skills, and the
//     skills of the user's and the built-in plugins.
//
// A project's skill replaces a system-wide one of the same name, and within
// a scope kou-conveyor's own directory comes first, then the shared one,
// then the plugins'. Directories are read again as they change (see
// Fingerprint), so skills added, removed or edited reach a running agent at
// its next turn.
package skill

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// Scope says whose a skill is.
type Scope string

const (
	// ScopeProject skills are a workspace's: the agent has them only there.
	ScopeProject Scope = "project"
	// ScopeSystem skills are the user's, in every workspace.
	ScopeSystem Scope = "system"
)

// Kinds of the directories skills come from.
const (
	KindHarness = "harness" // .harness/skills in the workspace
	KindAgents  = "agents"  // .agents/skills, in the workspace or the home directory
	KindConfig  = "config"  // skills/ in kou-conveyor's configuration directory
	KindPlugin  = "plugin"  // the skills directory of a plugin
)

// HarnessDirectory is where a workspace keeps kou-conveyor's skills.
func HarnessDirectory(workspace string) string {
	return filepath.Join(workspace, ".harness", "skills")
}

// AgentsDirectory is the skills directory the agents that follow the
// .agents convention share, in a workspace or in the home directory.
func AgentsDirectory(base string) string { return filepath.Join(base, ".agents", "skills") }

// UserDirectory is where the user keeps skills for kou-conveyor alone: in
// its configuration directory, beside the user's plugins.
func UserDirectory(configDirectory string) string {
	return filepath.Join(configDirectory, "skills")
}

// Options say where to find skills; an empty place is left out.
type Options struct {
	// Workspace holds the project's skills, in .harness/skills and
	// .agents/skills.
	Workspace string
	// Home is the user's home directory, with ~/.agents/skills.
	Home string
	// ConfigDirectory is kou-conveyor's configuration directory, with
	// skills/.
	ConfigDirectory string
	// Plugins are the plugins of the workspace; the active ones bring their
	// skills, a workspace's plugin to the project and the others to the
	// system.
	Plugins []plugin.Plugin
}

// Directory is a place skills come from.
type Directory struct {
	Path  string
	Scope Scope
	Kind  string
	// Plugin names the plugin whose skills the directory holds.
	Plugin string
	// Label is how the cockpits name the directory: .harness/skills,
	// ~/.agents/skills, plugin git-glance.
	Label string
}

// Skill is a skill as found.
type Skill struct {
	tool.Skill
	Directory Directory
	// Active is set for a skill the agent has; Reason says why another one
	// is not: a skill of the same name replaces it.
	Active bool
	Reason string
}

// Problem is a skill of a directory that could not be read.
type Problem struct {
	Directory Directory
	Err       error
}

// Found is what Discover found.
type Found struct {
	// Directories are where skills were looked for, the one that wins a
	// name first.
	Directories []Directory
	// Skills lists every skill found, by directory in that order.
	Skills []Skill
	// Errors are the skills that could not be read, and the plugins whose
	// skills could not be found.
	Errors []error
	// Problems are those of Errors that are a directory's.
	Problems []Problem
}

// Active lists the skills the agent has: the project's first.
func (found Found) Active() []tool.Skill {
	var skills []tool.Skill
	for _, current := range found.Skills {
		if current.Active {
			skills = append(skills, current.Skill)
		}
	}
	return skills
}

// Count tells how many of a scope's skills the agent has.
func (found Found) Count(scope Scope) int {
	count := 0
	for _, current := range found.Skills {
		if current.Active && current.Directory.Scope == scope {
			count++
		}
	}
	return count
}

// Directories lists where skills are looked for, the one that wins a name
// first: the project's before the system's. A directory reached twice, as
// the workspace's .agents/skills is when the workspace is the home
// directory, is looked in once, in its first place.
func Directories(options Options) ([]Directory, []error) {
	var directories []Directory
	var problems []error
	seen := map[string]bool{}
	add := func(directory Directory) {
		key := directory.Path
		if resolved, err := filepath.EvalSymlinks(directory.Path); err == nil {
			key = resolved
		}
		if seen[key] {
			return
		}
		seen[key] = true
		directories = append(directories, directory)
	}
	addPlugins := func(scope Scope) {
		for _, current := range options.Plugins {
			if !current.Active || current.Skills == "" || (current.Source == plugin.SourceWorkspace) != (scope == ScopeProject) {
				continue
			}
			path, err := current.Resolve(current.Skills)
			if err != nil {
				problems = append(problems, fmt.Errorf("plugin %q: skills: %w", current.Name, err))
				continue
			}
			add(Directory{Path: path, Scope: scope, Kind: KindPlugin, Plugin: current.Name, Label: "plugin " + current.Name})
		}
	}
	if options.Workspace != "" {
		add(Directory{Path: HarnessDirectory(options.Workspace), Scope: ScopeProject, Kind: KindHarness, Label: ".harness/skills"})
		add(Directory{Path: AgentsDirectory(options.Workspace), Scope: ScopeProject, Kind: KindAgents, Label: ".agents/skills"})
	}
	addPlugins(ScopeProject)
	if options.ConfigDirectory != "" {
		path := UserDirectory(options.ConfigDirectory)
		add(Directory{Path: path, Scope: ScopeSystem, Kind: KindConfig, Label: tilde(path, options.Home)})
	}
	if options.Home != "" {
		add(Directory{Path: AgentsDirectory(options.Home), Scope: ScopeSystem, Kind: KindAgents, Label: "~/.agents/skills"})
	}
	addPlugins(ScopeSystem)
	return directories, problems
}

// Discover finds the skills options name. Of the skills that share a name,
// the one of the first directory is the agent's and the others are listed
// as replaced; two of one directory are an error.
func Discover(options Options) Found {
	var found Found
	found.Directories, found.Errors = Directories(options)
	winners := map[string]Directory{}
	for _, directory := range found.Directories {
		skills, problems := tool.DiscoverSkills(directory.Path)
		found.Errors = append(found.Errors, problems...)
		for _, problem := range problems {
			found.Problems = append(found.Problems, Problem{Directory: directory, Err: problem})
		}
		for _, current := range skills {
			entry := Skill{Skill: current, Directory: directory, Active: true}
			if winner, taken := winners[current.Name]; taken {
				entry.Active, entry.Reason = false, "replaced by the skill of the same name in "+winner.Label
			} else {
				winners[current.Name] = directory
			}
			found.Skills = append(found.Skills, entry)
		}
	}
	return found
}

// Fingerprint summarizes what Discover reads of the directories of options
// — the entries of each and their SKILL.md files — and changes when a skill
// is added, removed or its SKILL.md written: a stat per skill, so it is
// cheap to take before every turn. The plugins' skills are left to
// plugin.Fingerprint, which follows the plugins' files.
func Fingerprint(options Options) string {
	options.Plugins = nil
	directories, _ := Directories(options)
	digest := sha256.New()
	for _, directory := range directories {
		fmt.Fprintf(digest, "root %s\n", directory.Path)
		entries, err := os.ReadDir(directory.Path)
		if err != nil {
			fmt.Fprintf(digest, "missing\n")
			continue
		}
		for _, entry := range entries {
			info, err := os.Stat(filepath.Join(directory.Path, entry.Name(), "SKILL.md"))
			if err != nil {
				fmt.Fprintf(digest, "%s -\n", entry.Name())
				continue
			}
			fmt.Fprintf(digest, "%s %d %d\n", entry.Name(), info.Size(), info.ModTime().UnixNano())
		}
	}
	return hex.EncodeToString(digest.Sum(nil))[:20]
}

// tilde writes a path under the home directory with ~ for it.
func tilde(path, home string) string {
	if home == "" {
		return path
	}
	if rest, ok := strings.CutPrefix(path, strings.TrimSuffix(home, string(filepath.Separator))+string(filepath.Separator)); ok {
		return "~/" + filepath.ToSlash(rest)
	}
	return path
}
