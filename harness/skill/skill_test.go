package skill

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func writeSkill(t *testing.T, directory, name, description string) string {
	t.Helper()
	path := filepath.Join(directory, name, "SKILL.md")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("---\nname: "+name+"\ndescription: "+description+"\n---\nBody.\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

// places are a workspace, a home and a configuration directory, apart.
func places(t *testing.T) (workspace, home, config string) {
	root := t.TempDir()
	workspace, home, config = filepath.Join(root, "project"), filepath.Join(root, "home"), filepath.Join(root, "home", "config")
	for _, directory := range []string{workspace, home, config} {
		os.MkdirAll(directory, 0o755)
	}
	return workspace, home, config
}

func TestDiscoverFindsTheProjectsSkillsBeforeTheSystems(t *testing.T) {
	workspace, home, config := places(t)
	writeSkill(t, HarnessDirectory(workspace), "review", "The project's review.")
	writeSkill(t, AgentsDirectory(workspace), "deploy", "Deploy this project.")
	writeSkill(t, AgentsDirectory(workspace), "review", "A shared review.")
	writeSkill(t, UserDirectory(config), "release", "kou-conveyor's release.")
	writeSkill(t, UserDirectory(config), "deploy", "Deploy anything.")
	writeSkill(t, AgentsDirectory(home), "commit", "Commit.")
	writeSkill(t, AgentsDirectory(home), "release", "Another release.")
	// Plugins' skills: a workspace's is the project's, a user's the
	// system's, and one that does not run brings none.
	pluginOf := func(name string, source plugin.Source, active bool) plugin.Plugin {
		directory := filepath.Join(t.TempDir(), name)
		writeSkill(t, filepath.Join(directory, "skills"), name+"-skill", "From "+name+".")
		return plugin.Plugin{Manifest: plugin.Manifest{Name: name, Skills: "skills"}, Source: source, Directory: directory, Active: active}
	}
	found := Discover(Options{Workspace: workspace, Home: home, ConfigDirectory: config, Plugins: []plugin.Plugin{
		pluginOf("mine", plugin.SourceUser, true), pluginOf("local", plugin.SourceWorkspace, true), pluginOf("off", plugin.SourceWorkspace, false),
	}})
	if len(found.Errors) != 0 {
		t.Fatalf("errors = %v", found.Errors)
	}
	var labels []string
	for _, directory := range found.Directories {
		labels = append(labels, string(directory.Scope)+":"+directory.Label)
	}
	if got := strings.Join(labels, " "); got != "project:.harness/skills project:.agents/skills project:plugin local system:~/config/skills system:~/.agents/skills system:plugin mine" {
		t.Fatalf("directories = %s", got)
	}
	var skills []string
	for _, current := range found.Skills {
		line := current.Name + "@" + current.Directory.Label
		if !current.Active {
			line += " (" + current.Reason + ")"
		}
		skills = append(skills, line)
	}
	want := []string{
		"review@.harness/skills",
		"deploy@.agents/skills", "review@.agents/skills (replaced by the skill of the same name in .harness/skills)",
		"local-skill@plugin local",
		"deploy@~/config/skills (replaced by the skill of the same name in .agents/skills)", "release@~/config/skills",
		"commit@~/.agents/skills", "release@~/.agents/skills (replaced by the skill of the same name in ~/config/skills)",
		"mine-skill@plugin mine",
	}
	if got := strings.Join(skills, "\n"); got != strings.Join(want, "\n") {
		t.Fatalf("skills:\n%s\nwant:\n%s", got, strings.Join(want, "\n"))
	}
	var active []string
	for _, current := range found.Active() {
		active = append(active, current.Name)
	}
	if got := strings.Join(active, " "); got != "review deploy local-skill release commit mine-skill" {
		t.Fatalf("active = %s", got)
	}
	if found.Count(ScopeProject) != 3 || found.Count(ScopeSystem) != 3 {
		t.Fatalf("counts: %d project, %d system", found.Count(ScopeProject), found.Count(ScopeSystem))
	}
}

// A directory reached twice — the workspace is the home directory — is
// read once, in its first place.
func TestDiscoverReadsADirectoryOnce(t *testing.T) {
	home := t.TempDir()
	writeSkill(t, AgentsDirectory(home), "commit", "Commit.")
	found := Discover(Options{Workspace: home, Home: home})
	if len(found.Directories) != 2 || len(found.Skills) != 1 || !found.Skills[0].Active || found.Skills[0].Directory.Scope != ScopeProject {
		t.Fatalf("found = %+v", found)
	}
}

func TestDiscoverTiesProblemsToTheirDirectory(t *testing.T) {
	workspace, home, _ := places(t)
	broken := filepath.Join(AgentsDirectory(home), "broken", "SKILL.md")
	os.MkdirAll(filepath.Dir(broken), 0o755)
	os.WriteFile(broken, []byte("no frontmatter"), 0o644)
	found := Discover(Options{Workspace: workspace, Home: home, Plugins: []plugin.Plugin{
		{Manifest: plugin.Manifest{Name: "compiled", Skills: "skills"}, Source: plugin.SourceBuiltin, Active: true},
	}})
	if len(found.Errors) != 2 || len(found.Problems) != 1 || found.Problems[0].Directory.Label != "~/.agents/skills" || !strings.Contains(found.Problems[0].Err.Error(), broken) {
		t.Fatalf("errors %v, problems %+v", found.Errors, found.Problems)
	}
}

// The fingerprint follows the skills — one added, edited or removed — and
// not the other files of a skill, which the agent reads when it uses it.
func TestFingerprintFollowsTheSkills(t *testing.T) {
	workspace, home, config := places(t)
	options := Options{Workspace: workspace, Home: home, ConfigDirectory: config}
	last := Fingerprint(options)
	changed := func(what string) {
		t.Helper()
		now := Fingerprint(options)
		if now == last {
			t.Fatalf("the fingerprint did not change when %s", what)
		}
		last = now
	}
	path := writeSkill(t, AgentsDirectory(home), "commit", "Commit.")
	changed("a skill came")
	os.WriteFile(filepath.Join(filepath.Dir(path), "notes.md"), []byte("more"), 0o644)
	os.MkdirAll(filepath.Join(filepath.Dir(path), "scripts"), 0o755)
	if Fingerprint(options) != last {
		t.Fatal("the fingerprint changed with a file the skill does not list")
	}
	later := time.Now().Add(time.Minute)
	os.WriteFile(path, []byte("---\nname: commit\ndescription: Commit well.\n---\n"), 0o644)
	os.Chtimes(path, later, later)
	changed("a SKILL.md was written")
	writeSkill(t, HarnessDirectory(workspace), "review", "Review.")
	changed("a project skill came")
	os.RemoveAll(filepath.Dir(path))
	changed("a skill went")
}
