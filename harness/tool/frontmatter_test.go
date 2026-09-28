package tool

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// Skills come from other agents' directories too (~/.agents/skills), whose
// frontmatter uses all of YAML — or is not quite YAML.
func TestDiscoverSkillsReadsTheirFrontmatter(t *testing.T) {
	directory := filepath.Join(t.TempDir(), "skills[1]") // a glob character in the path
	files := map[string]string{
		"folded": "---\nname: folded\ndescription: >\n  Deploy the branch,\n  then check it.\ndisable-model-invocation: true\n---\nBody.\n",
		"quoted": "---\nname: quoted\ndescription: 'Commit: with a colon'\nlicense: MIT\nmetadata:\n  version: \"1.0\"\n---\n",
		"loose":  "---\nname: loose\ndescription: Use when: the user asks, not before\n---\n",
		"crlf":   "\uFEFF---\r\nname: crlf\r\ndescription: \"Written on Windows.\"\r\n---\r\n",
		"manual": "---\nname: manual\ndescription: Only when asked: by name\ndisable-model-invocation: \"true\"\n---\n",
	}
	for name, content := range files {
		path := filepath.Join(directory, name, "SKILL.md")
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	// A skill linked in from elsewhere counts, as in ~/.agents/skills.
	elsewhere := filepath.Join(t.TempDir(), "linked")
	os.MkdirAll(elsewhere, 0o700)
	os.WriteFile(filepath.Join(elsewhere, "SKILL.md"), []byte("---\nname: linked\ndescription: From elsewhere.\n---\n"), 0o600)
	if err := os.Symlink(elsewhere, filepath.Join(directory, "linked")); err != nil {
		t.Fatal(err)
	}
	// A file beside the skills is none.
	os.WriteFile(filepath.Join(directory, "README.md"), []byte("not a skill"), 0o600)

	skills, problems := DiscoverSkills(directory)
	if len(problems) != 0 {
		t.Fatalf("problems = %v", problems)
	}
	path := func(name string) string { return filepath.Join(directory, name, "SKILL.md") }
	want := []Skill{
		{Name: "crlf", Description: "Written on Windows.", Path: path("crlf")},
		{Name: "folded", Description: "Deploy the branch, then check it.", Path: path("folded"), Manual: true},
		{Name: "linked", Description: "From elsewhere.", Path: path("linked")},
		{Name: "loose", Description: "Use when: the user asks, not before", Path: path("loose")},
		{Name: "manual", Description: "Only when asked: by name", Path: path("manual"), Manual: true},
		{Name: "quoted", Description: "Commit: with a colon", Path: path("quoted")},
	}
	if !reflect.DeepEqual(skills, want) {
		t.Fatalf("skills =\n%#v\nwant\n%#v", skills, want)
	}
}

func TestDiscoverSkillsReportsAnUnreadableDirectory(t *testing.T) {
	file := filepath.Join(t.TempDir(), "skills")
	os.WriteFile(file, []byte("a file"), 0o600)
	if skills, problems := DiscoverSkills(file); len(skills) != 0 || len(problems) != 1 {
		t.Fatalf("skills %v, problems %v", skills, problems)
	}
}
