package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

// /skills lists the project's skills, then the system-wide ones, and those
// a skill of the same name replaces.
func TestSkillsListsTheProjectsAndTheSystems(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	m := testModel(t)
	write := func(directory, name, description string) {
		path := filepath.Join(directory, name, "SKILL.md")
		os.MkdirAll(filepath.Dir(path), 0o755)
		os.WriteFile(path, []byte("---\nname: "+name+"\ndescription: "+description+"\n---\n"), 0o644)
	}
	write(skill.HarnessDirectory(m.opt.Workspace), "commit", "The project's commits.")
	write(skill.AgentsDirectory(home), "commit", "Any commits.")
	write(skill.AgentsDirectory(home), "animate", "Animate things.")

	m.command("/skills")
	if m.picker == nil || m.picker.kind != "skills" || len(m.picker.items) != 3 {
		t.Fatalf("picker = %+v", m.picker)
	}
	var rows []string
	for _, item := range m.picker.items {
		rows = append(rows, item.title+" | "+item.detail)
	}
	want := "commit — The project's commits. | project · .harness/skills\n" +
		"animate — Animate things. | system · ~/.agents/skills\n" +
		"commit — Any commits. | replaced · system"
	if got := strings.Join(rows, "\n"); got != want {
		t.Fatalf("rows:\n%s\nwant:\n%s", got, want)
	}
}
