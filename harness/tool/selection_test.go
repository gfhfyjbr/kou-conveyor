package tool

import (
	"fmt"
	"slices"
	"testing"
)

func TestRegistryKeepsSelectedStaticToolsAcrossCatalogChanges(t *testing.T) {
	allNames := []string{BashName, ViewImageName, SkillUseName}
	for selection := range 1 << len(allNames) {
		t.Run(fmt.Sprint(selection), func(t *testing.T) {
			var enabled []string
			var want []string
			for index, name := range allNames {
				if selection&(1<<index) != 0 {
					enabled = append(enabled, name, name)
					want = append(want, name)
				}
			}
			slices.Reverse(enabled)
			registry := NewRegistry(StaticTranslators{}, enabled...)
			assertSelection := func() {
				t.Helper()
				var got []string
				for _, definition := range registry.StaticDefinitions() {
					got = append(got, definition.Tool.Name)
				}
				if !slices.Equal(got, want) {
					t.Fatalf("static tools = %v, want %v", got, want)
				}
				for _, name := range allNames {
					translator, exists := registry.Resolve(name)
					selected := slices.Contains(want, name)
					if exists != selected || (translator != nil) != selected {
						t.Fatalf("resolve %s = (%T, %t), selected = %t", name, translator, exists, selected)
					}
				}
			}
			assertSelection()
			clear(enabled)
			assertSelection()
			skillID, err := registry.RegisterSkill(Skill{
				Name: "review", Description: "Review code.", Path: "/skills/review/SKILL.md",
			})
			if err != nil {
				t.Fatal(err)
			}
			assertSelection()
			registry.UnregisterSkill(skillID)
			assertSelection()
		})
	}
}

func TestRegistryDoesNotResolveUnselectedTranslators(t *testing.T) {
	registry := NewRegistry(StaticTranslators{
		Bash: &fixedTranslator{}, ViewImage: &fixedTranslator{},
	})
	if got := registry.StaticDefinitions(); len(got) != 0 {
		t.Fatalf("empty selection advertises %v", got)
	}
	for _, name := range []string{BashName, ViewImageName, SkillUseName} {
		translator, exists := registry.Resolve(name)
		if exists || translator != nil {
			t.Fatalf("unselected %s resolves to (%T, %t)", name, translator, exists)
		}
	}
}
