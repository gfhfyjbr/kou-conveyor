package contextbuilder

import (
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// A skill whose frontmatter keeps the model from loading it on its own is
// named apart from the list, for when the user asks for it.
func TestBuilderNamesManualSkillsApart(t *testing.T) {
	current := NewBuilder(
		tool.Skill{Name: "review", Description: "Review code.", Path: "/skills/review/SKILL.md"},
		tool.Skill{Name: "deploy", Description: "Deploy to staging.", Path: "/skills/deploy/SKILL.md", Manual: true},
		tool.Skill{Name: "prototype", Description: "Prototype a UI.", Path: "/skills/prototype/SKILL.md", Manual: true},
	)
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	system := result.Request.Input[0].Data.(llm.Message).Text
	listed := "<available_skills><skill><name>review</name><description>Review code.</description><location>/skills/review/SKILL.md</location></skill></available_skills>"
	manual := "Load these skills with SkillUse only when the user asks for one of them by name: deploy, prototype."
	if !strings.Contains(system, listed) || !strings.Contains(system, listed+"\n\n"+manual) || strings.Contains(system, "Deploy to staging") {
		t.Fatalf("system prompt = %q", system)
	}

	// With only such skills, the model hears of them still.
	current.SetSkills([]tool.Skill{{Name: "deploy", Description: "Deploy to staging.", Path: "/skills/deploy/SKILL.md", Manual: true}})
	result, err = current.Build()
	if err != nil {
		t.Fatal(err)
	}
	system = result.Request.Input[0].Data.(llm.Message).Text
	if strings.Contains(system, "<available_skills>") || !strings.Contains(system, skillPreamble+"\n\nLoad these skills with SkillUse only when the user asks for one of them by name: deploy.") {
		t.Fatalf("system prompt = %q", system)
	}
}
