package agentrunner

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

func writeRunSkill(t *testing.T, directory, name, description string) string {
	t.Helper()
	path := filepath.Join(directory, name, "SKILL.md")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("---\nname: "+name+"\ndescription: "+description+"\n---\nBody.\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	// A second's step in the file times, whatever the file system's
	// resolution.
	later := time.Now().Add(2 * time.Second)
	os.Chtimes(path, later, later)
	return path
}

// A run has the project's skills — the workspace's .harness/skills and
// .agents/skills — and the system-wide ones — ~/.agents/skills and skills/
// in the configuration directory — a project's replacing a system one of
// the same name.
func TestRunnerHasTheProjectsAndTheSystemsSkills(t *testing.T) {
	root := t.TempDir()
	workspace, home := filepath.Join(root, "project"), filepath.Join(root, "home")
	config := filepath.Join(root, "config") // where runWithPlugins has it
	project := writeRunSkill(t, skill.HarnessDirectory(workspace), "commit", "The project's commits.")
	writeRunSkill(t, skill.AgentsDirectory(workspace), "deploy", "Deploy the project.")
	system := writeRunSkill(t, skill.AgentsDirectory(home), "commit", "Any commits.")
	writeRunSkill(t, skill.AgentsDirectory(home), "animate", "Animate things.")
	writeRunSkill(t, skill.UserDirectory(config), "release", "Release kou-conveyor.")
	var systems []string
	stdout, stderr, code := runWithPlugins(t, workspace, map[string]string{"HOME": home}, `{"prompt":"work"}`,
		func(n int, request llm.Request) llm.Response {
			systems = append(systems, request.Input[0].Data.(llm.Message).Text)
			if n == 1 {
				// A skill comes to ~/.agents/skills while the agent works.
				writeRunSkill(t, skill.AgentsDirectory(home), "later", "Come later.")
				return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command": "true"}`}}}}
			}
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}
		})
	if code != 0 {
		t.Fatalf("exit %d: %s\n%s", code, stderr, stdout)
	}
	if len(systems) != 2 {
		t.Fatalf("%d turns", len(systems))
	}
	first := systems[0]
	for _, want := range []string{"<name>commit</name><description>The project&#39;s commits.</description><location>" + project, "<name>deploy</name>", "<name>animate</name>", "<name>release</name>"} {
		if !strings.Contains(first, want) {
			t.Fatalf("the system prompt lacks %q: %s", want, first)
		}
	}
	// The project's skills come first.
	if strings.Contains(first, system) || strings.Index(first, "<name>deploy</name>") > strings.Index(first, "<name>release</name>") || strings.Contains(first, "<name>later</name>") {
		t.Fatalf("system prompt = %s", first)
	}
	if !strings.Contains(systems[1], "<name>later</name><description>Come later.</description>") {
		t.Fatalf("the skill that came is not in the next request: %s", systems[1])
	}
	if !strings.Contains(stderr, "skill> the skills changed; the agent has 5 from its next turn: +later") {
		t.Fatalf("stderr = %s", stderr)
	}
}

func TestRunnerListsSkills(t *testing.T) {
	root := t.TempDir()
	workspace, home := filepath.Join(root, "project"), filepath.Join(root, "home")
	writeRunSkill(t, skill.HarnessDirectory(workspace), "commit", "The project's commits.")
	writeRunSkill(t, skill.AgentsDirectory(home), "commit", "Any commits.")
	broken := filepath.Join(skill.AgentsDirectory(home), "broken", "SKILL.md")
	os.MkdirAll(filepath.Dir(broken), 0o755)
	os.WriteFile(broken, []byte("no frontmatter"), 0o644)
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(), []string{"-list-skills", "-workspace", workspace}, func(name string) string {
		if name == "HOME" {
			return home
		}
		return ""
	}, func() []string { return nil }, strings.NewReader(""), &stdout, &stderr, testConfig(&fakeClient{}))
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr.String())
	}
	lines := strings.Split(strings.TrimSpace(stdout.String()), "\n")
	if len(lines) != 3 ||
		!strings.Contains(lines[0], `"name":"commit"`) || !strings.Contains(lines[0], `"scope":"project","directory":".harness/skills","active":true`) ||
		!strings.Contains(lines[1], `"scope":"system","directory":"~/.agents/skills","active":false,"reason":"replaced by the skill of the same name in .harness/skills"`) ||
		!strings.Contains(lines[2], `"error":"parse skill`) {
		t.Fatalf("listing:\n%s", stdout.String())
	}
}
