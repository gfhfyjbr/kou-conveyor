package plugin

import (
	"io/fs"
	"reflect"
	"strings"
	"testing"
)

// The plugins compiled in as directories load, and run unless turned off.
func TestBuiltinsCompiledInLoad(t *testing.T) {
	plugins, problems := compiledBuiltins()
	if len(problems) != 0 {
		t.Fatalf("problems = %v", problems)
	}
	var guide *Plugin
	for index := range plugins {
		if plugins[index].Name == "guide" {
			guide = &plugins[index]
		}
	}
	if guide == nil || guide.Source != SourceBuiltin || guide.Directory != "" || guide.Files == nil || guide.Skills != "skills" {
		t.Fatalf("guide = %+v", guide)
	}
	if skills, err := guide.Sub(guide.Skills); err != nil || !fsExists(skills, "kou-conveyor-plugins/SKILL.md") {
		t.Fatalf("its skills: %v", err)
	}
	if _, err := guide.Sub("../elsewhere"); err == nil {
		t.Fatal("Sub left the plugin")
	}
	found := Discover(Options{})
	if len(found.Errors) != 0 || !activePlugin(found, "guide") {
		t.Fatalf("found = %+v", found)
	}
	if found = Discover(Options{Disabled: []string{"guide"}}); activePlugin(found, "guide") {
		t.Fatal("guide runs although it is turned off")
	}
}

// The guide's skill documents every field of plugin.json: a field added to
// the manifest is added there too.
func TestGuideDocumentsTheManifest(t *testing.T) {
	skill, err := fs.ReadFile(builtinFiles, "builtin/guide/skills/kou-conveyor-plugins/SKILL.md")
	if err != nil {
		t.Fatal(err)
	}
	for _, value := range []any{Manifest{}, Tool{}, Command{}, Web{}} {
		kind := reflect.TypeOf(value)
		for index := range kind.NumField() {
			name, _, _ := strings.Cut(kind.Field(index).Tag.Get("json"), ",")
			if name != "" && !strings.Contains(string(skill), "`"+name+"`") {
				t.Errorf("SKILL.md does not document %s's %q", kind.Name(), name)
			}
		}
	}
}

func fsExists(fsys fs.FS, name string) bool {
	_, err := fs.Stat(fsys, name)
	return err == nil
}

func activePlugin(found Found, name string) bool {
	for _, current := range found.Active() {
		if current.Name == name {
			return true
		}
	}
	return false
}
