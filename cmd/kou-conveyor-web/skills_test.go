package main

import (
	"encoding/json/v2"
	"path/filepath"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

func skillFile(name, description string) map[string]string {
	return map[string]string{name + "/SKILL.md": "---\nname: " + name + "\ndescription: " + description + "\n---\nBody.\n"}
}

// skillsOf is the skills of a listing, by name@directory.
func skillsOf(t *testing.T, listing map[string]any) map[string]map[string]any {
	t.Helper()
	skills := map[string]map[string]any{}
	for _, value := range listing["skills"].(map[string]any)["skills"].([]any) {
		current := value.(map[string]any)
		skills[current["name"].(string)+"@"+current["directory"].(string)] = current
	}
	return skills
}

// The listing says which skills a run in the workspace has: the project's
// and the system-wide ones, and those a skill of the same name replaces;
// and it comes anew when a skill does.
func TestPluginListingHasTheSkills(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	h := newHarness(t)
	h.server.watch.interval = 20 * time.Millisecond
	ws := h.server.workspaces.startup()
	writeFiles(t, skill.HarnessDirectory(ws.Path), skillFile("commit", "The project's commits."))
	writeFiles(t, skill.AgentsDirectory(ws.Path), skillFile("deploy", "Deploy it."))
	writeFiles(t, skill.AgentsDirectory(home), skillFile("commit", "Any commits."))
	writeFiles(t, skill.UserDirectory(filepath.Dir(h.server.opt.SettingsFile)), skillFile("release", "Release."))

	_, _, body := h.get("/api/plugins")
	var listing map[string]any
	if err := json.Unmarshal([]byte(body), &listing); err != nil {
		t.Fatal(err)
	}
	var labels []any
	for _, value := range listing["skills"].(map[string]any)["directories"].([]any) {
		directory := value.(map[string]any)
		labels = append(labels, directory["scope"].(string)+":"+directory["kind"].(string)+":"+directory["label"].(string))
	}
	if len(labels) != 4 || labels[0] != "project:harness:.harness/skills" || labels[1] != "project:agents:.agents/skills" || labels[3] != "system:agents:~/.agents/skills" {
		t.Fatalf("directories = %v", labels)
	}
	skills := skillsOf(t, listing)
	project, replaced, release := skills["commit@.harness/skills"], skills["commit@~/.agents/skills"], skills["release@"+labels[2].(string)[len("system:config:"):]]
	if project == nil || project["scope"] != "project" || project["active"] != true || project["file"] != ".harness/skills/commit/SKILL.md" {
		t.Fatalf("the project's commit = %v", project)
	}
	if replaced == nil || replaced["scope"] != "system" || replaced["active"] != false || replaced["reason"] != "replaced by the skill of the same name in .harness/skills" || replaced["file"] != nil {
		t.Fatalf("the system's commit = %v", replaced)
	}
	if release == nil || release["active"] != true || skills["deploy@.agents/skills"] == nil {
		t.Fatalf("skills = %v", skills)
	}

	stream := h.pluginEvents("/api/plugins/events")
	stream.next("plugins")
	writeFiles(t, skill.AgentsDirectory(home), skillFile("later", "Later."))
	stream.until(func(listing map[string]any) bool { return skillsOf(t, listing)["later@~/.agents/skills"] != nil })
}
