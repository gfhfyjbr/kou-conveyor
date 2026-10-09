package main

import (
	"io/fs"
	"regexp"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// guideFile reads a file of the built-in guide's skill, which tells the
// agent how to write plugins.
func guideFile(t *testing.T, name string) string {
	t.Helper()
	for _, current := range plugin.Builtins() {
		if current.Name != "guide" {
			continue
		}
		files, err := current.Sub("skills/kou-conveyor-plugins")
		if err != nil {
			t.Fatal(err)
		}
		content, err := fs.ReadFile(files, name)
		if err != nil {
			t.Fatal(err)
		}
		return string(content)
	}
	t.Fatal("no guide plugin")
	return ""
}

// The guide's reference covers what the cockpit's built-in plugins offer
// other plugins — services, slots, contribution points, hooks and events —
// and its skill names every built-in plugin, which a plugin of the same
// name replaces: what a built-in plugin adds is added there too.
func TestGuideCoversTheCockpit(t *testing.T) {
	reference := guideFile(t, "reference/web.md")
	skill := guideFile(t, "SKILL.md")
	offered := map[string]*regexp.Regexp{
		"service":            regexp.MustCompile(`cockpit\.provide\('([^']+)'`),
		"slot":               regexp.MustCompile(`ui\.(?:slot|mount)\('([^']+)'`),
		"contribution point": regexp.MustCompile(`contributions\('([^']+)'`),
		"hook":               regexp.MustCompile(`hooks\.(?:run|runAsync|first)\('([^']+)'`),
		"event":              regexp.MustCompile(`cockpit\.emit\('([^']+)'`),
	}
	slotNames := regexp.MustCompile(`'([a-z]+(?:\.[a-z]+)?)': [a-zA-Z]+[,\n ]`)
	scripts, err := fs.Glob(builtinFiles, "plugins/*/web/*.js")
	if err != nil || len(scripts) == 0 {
		t.Fatalf("scripts: %v, %v", scripts, err)
	}
	for _, script := range scripts {
		content, err := fs.ReadFile(builtinFiles, script)
		if err != nil {
			t.Fatal(err)
		}
		for what, pattern := range offered {
			for _, match := range pattern.FindAllStringSubmatch(string(content), -1) {
				if !strings.Contains(reference, "`"+match[1]+"`") {
					t.Errorf("reference/web.md lacks the %s %q (%s)", what, match[1], script)
				}
			}
		}
		// The layout declares its slots from an object of them.
		if strings.HasSuffix(script, "layout/web/layout.js") {
			_, declared, _ := strings.Cut(string(content), "const slots = {")
			declared, _, _ = strings.Cut(declared, "};")
			for _, match := range slotNames.FindAllStringSubmatch(declared, -1) {
				if !strings.Contains(reference, "`"+match[1]+"`") {
					t.Errorf("reference/web.md lacks the slot %q", match[1])
				}
			}
		}
	}
	builtins, problems := compiledAssets().builtins()
	if len(problems) != 0 {
		t.Fatal(problems)
	}
	names := []string{}
	for _, current := range append(plugin.Builtins(), builtins...) {
		names = append(names, current.Name)
	}
	for _, name := range slices.Compact(slices.Sorted(slices.Values(names))) {
		if !strings.Contains(skill, "`"+name+"`") {
			t.Errorf("SKILL.md does not name the built-in plugin %q", name)
		}
	}
}
