package contextbuilder

import (
	_ "embed"
	"encoding/xml"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

//go:embed prompts/skill-preamble.md
var skillPreambleFile string

var skillPreamble = strings.TrimSpace(skillPreambleFile)

type availableSkills struct {
	XMLName xml.Name      `xml:"available_skills"`
	Skills  []promptSkill `xml:"skill"`
}

type promptSkill struct {
	Name        string `xml:"name"`
	Description string `xml:"description"`
	Location    string `xml:"location"`
}

// formatSkillsForPrompt lists the skills for the system prompt. A skill the
// model is not to load on its own is named apart, for when the user asks
// for it.
func formatSkillsForPrompt(skills []tool.Skill) string {
	var promptSkills []promptSkill
	var manual []string
	for _, skill := range skills {
		if skill.Manual {
			manual = append(manual, skill.Name)
			continue
		}
		promptSkills = append(promptSkills, promptSkill{
			Name:        skill.Name,
			Description: skill.Description,
			Location:    skill.Path,
		})
	}
	if len(promptSkills) == 0 && len(manual) == 0 {
		return ""
	}
	parts := []string{skillPreamble}
	if len(promptSkills) > 0 {
		encoded, err := xml.Marshal(availableSkills{Skills: promptSkills})
		if err != nil {
			panic(err)
		}
		parts = append(parts, string(encoded))
	}
	if len(manual) > 0 {
		parts = append(parts, "Load these skills with SkillUse only when the user asks for one of them by name: "+strings.Join(manual, ", ")+".")
	}
	return strings.Join(parts, "\n\n")
}
