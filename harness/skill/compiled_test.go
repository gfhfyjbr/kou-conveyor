package skill

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// compiledPlugin is a built-in plugin whose skills are compiled in.
func compiledPlugin(name string, files fstest.MapFS) plugin.Plugin {
	return plugin.Plugin{Manifest: plugin.Manifest{Name: name, Skills: "skills"}, Source: plugin.SourceBuiltin, Files: files, Active: true}
}

// The skills of a plugin compiled into the program are written out, the
// files beside SKILL.md with it, for the agent to read; and written again
// when they went or changed.
func TestDiscoverWritesTheSkillsCompiledIn(t *testing.T) {
	config := t.TempDir()
	files := fstest.MapFS{
		"plugin.json":                    {Data: []byte(`{"name": "handbook", "skills": "skills"}`)},
		"skills/howto/SKILL.md":          {Data: []byte("---\nname: howto\ndescription: How to.\n---\nSee reference/more.md.\n")},
		"skills/howto/reference/more.md": {Data: []byte("More.\n")},
	}
	found := Discover(Options{ConfigDirectory: config, Plugins: []plugin.Plugin{compiledPlugin("handbook", files)}})
	if len(found.Errors) != 0 || len(found.Skills) != 1 {
		t.Fatalf("found = %+v", found)
	}
	current := found.Skills[0]
	directory := current.Directory
	if directory.Scope != ScopeSystem || directory.Kind != KindBuiltin || directory.Label != "built in · handbook" || directory.Plugin != "handbook" ||
		!strings.HasPrefix(directory.Path, filepath.Join(config, "builtin", "handbook-")) || current.Path != filepath.Join(directory.Path, "howto", "SKILL.md") {
		t.Fatalf("skill = %+v", current)
	}
	if more, err := os.ReadFile(filepath.Join(directory.Path, "howto", "reference", "more.md")); err != nil || string(more) != "More.\n" {
		t.Fatalf("the file beside SKILL.md: %q, %v", more, err)
	}

	// Edited, it is written again by the next process to look.
	os.WriteFile(current.Path, []byte("tampered"), 0o644)
	checked.Delete(directory.Path)
	Discover(Options{ConfigDirectory: config, Plugins: []plugin.Plugin{compiledPlugin("handbook", files)}})
	if content, _ := os.ReadFile(current.Path); !strings.Contains(string(content), "name: howto") {
		t.Fatalf("SKILL.md was not written again: %q", content)
	}
	// Gone, it is written again at once.
	os.RemoveAll(directory.Path)
	Discover(Options{ConfigDirectory: config, Plugins: []plugin.Plugin{compiledPlugin("handbook", files)}})
	if _, err := os.Stat(current.Path); err != nil {
		t.Fatalf("SKILL.md was not written again: %v", err)
	}

	// Other content goes to a directory of its own.
	changed := fstest.MapFS{"skills/howto/SKILL.md": {Data: []byte("---\nname: howto\ndescription: How to, better.\n---\n")}}
	again := Discover(Options{ConfigDirectory: config, Plugins: []plugin.Plugin{compiledPlugin("handbook", changed)}})
	if len(again.Skills) != 1 || again.Skills[0].Directory.Path == directory.Path || again.Skills[0].Description != "How to, better." {
		t.Fatalf("changed = %+v", again.Skills)
	}

	// Without a configuration directory there is nowhere to write them.
	if none := Discover(Options{Plugins: []plugin.Plugin{compiledPlugin("handbook", files)}}); len(none.Skills) != 0 || len(none.Errors) != 0 || len(none.Directories) != 0 {
		t.Fatalf("without a configuration directory: %+v", none)
	}
}

// A user's skill replaces a built-in one of the same name, whether it is in
// a skills directory or a plugin of the user's.
func TestTheUsersSkillsReplaceTheBuiltInOnes(t *testing.T) {
	config := t.TempDir()
	builtin := compiledPlugin("handbook", fstest.MapFS{"skills/howto/SKILL.md": {Data: []byte("---\nname: howto\ndescription: Built in.\n---\n")}})
	mine := filepath.Join(t.TempDir(), "mine")
	writeSkill(t, filepath.Join(mine, "skills"), "howto", "Mine.")
	user := plugin.Plugin{Manifest: plugin.Manifest{Name: "mine", Skills: "skills"}, Source: plugin.SourceUser, Directory: mine, Active: true}
	found := Discover(Options{ConfigDirectory: config, Plugins: []plugin.Plugin{builtin, user}})
	if len(found.Skills) != 2 || !found.Skills[0].Active || found.Skills[0].Description != "Mine." ||
		found.Skills[1].Active || found.Skills[1].Reason != "replaced by the skill of the same name in plugin mine" {
		t.Fatalf("skills = %+v", found.Skills)
	}
}

// The guide plugin's skill, which tells the agent how to write plugins, is
// there for every workspace, with the reference it names beside it.
func TestTheGuideSkillIsBuiltIn(t *testing.T) {
	config := t.TempDir()
	found := Discover(Options{Workspace: t.TempDir(), ConfigDirectory: config, Plugins: plugin.Discover(plugin.Options{}).Active()})
	if len(found.Errors) != 0 {
		t.Fatalf("errors = %v", found.Errors)
	}
	var guide *Skill
	for index := range found.Skills {
		if found.Skills[index].Name == "kou-conveyor-plugins" {
			guide = &found.Skills[index]
		}
	}
	if guide == nil || !guide.Active || guide.Manual || guide.Directory.Scope != ScopeSystem || guide.Directory.Label != "built in · guide" {
		t.Fatalf("guide = %+v", guide)
	}
	content, err := os.ReadFile(guide.Path)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"# Writing kou-conveyor plugins", "## 1. Choose the scope", ".harness/plugins", "reference/web.md"} {
		if !strings.Contains(string(content), want) {
			t.Fatalf("SKILL.md lacks %q", want)
		}
	}
	if _, err := os.Stat(filepath.Join(filepath.Dir(guide.Path), "reference", "web.md")); err != nil {
		t.Fatalf("the reference SKILL.md names is not beside it: %v", err)
	}
}
