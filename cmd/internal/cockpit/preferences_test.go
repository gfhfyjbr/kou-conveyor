package cockpit_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestEffortIsSharedThroughPreferences(t *testing.T) {
	dir := t.TempDir()
	path := cockpit.PreferencesPath(filepath.Join(dir, "config", "settings.json"))
	if path != filepath.Join(dir, "config", "preferences.json") || cockpit.PreferencesPath("") != "" {
		t.Fatalf("path = %q", path)
	}
	if p := cockpit.LoadPreferences(path); p.Effort != "" {
		t.Fatalf("no file: %+v", p)
	}
	seen, changed := cockpit.PreferencesChanged(path, nil)
	if seen != nil || changed {
		t.Fatalf("a missing file changed: %v %v", seen, changed)
	}

	if err := cockpit.SaveEffort(path, "max"); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); p.Effort != "max" {
		t.Fatalf("saved = %+v", p)
	}
	seen, changed = cockpit.PreferencesChanged(path, seen)
	if !changed {
		t.Fatal("saving was not a change")
	}
	if _, changed := cockpit.PreferencesChanged(path, seen); changed {
		t.Fatal("reading again was a change")
	}
	// Another cockpit saves: the file is replaced.
	if err := cockpit.SaveEffort(path, "low"); err != nil {
		t.Fatal(err)
	}
	if _, changed := cockpit.PreferencesChanged(path, seen); !changed {
		t.Fatal("another save went unnoticed")
	}
	if info, _ := os.Stat(path); info.Mode().Perm()&0o077 != 0 {
		t.Fatalf("mode = %v", info.Mode())
	}

	if err := cockpit.SaveEffort(path, "huge"); err == nil {
		t.Fatal("saved an unknown effort")
	}
	if err := cockpit.SaveEffort("", "low"); err == nil {
		t.Fatal("saved without a place for preferences")
	}
	// What no cockpit understands only fills in nothing.
	os.WriteFile(path, []byte(`{"effort":"extreme","future":true}`), 0o600)
	if p := cockpit.LoadPreferences(path); p.Effort != "" {
		t.Fatalf("unknown effort = %+v", p)
	}
	os.WriteFile(path, []byte(`not json`), 0o600)
	if p := cockpit.LoadPreferences(path); p.Effort != "" {
		t.Fatalf("broken file = %+v", p)
	}
	if err := cockpit.SaveEffort(path, "medium"); err != nil || cockpit.LoadPreferences(path).Effort != "medium" {
		t.Fatalf("saving over a broken file: %v", err)
	}

	// The terminal cockpit's layout is kept beside the effort.
	if err := cockpit.SaveLayout(path, cockpit.LayoutCompact); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); p.Layout != cockpit.LayoutCompact || p.Effort != "medium" {
		t.Fatalf("after saving the layout: %+v", p)
	}
	if err := cockpit.SaveEffort(path, "max"); err != nil || cockpit.LoadPreferences(path).Layout != cockpit.LayoutCompact {
		t.Fatalf("saving the effort lost the layout: %v", err)
	}
	if err := cockpit.SaveLayout(path, "sideways"); err == nil {
		t.Fatal("saved an unknown layout")
	}
	os.WriteFile(path, []byte(`{"tui_layout":"sideways","effort":"low"}`), 0o600)
	if p := cockpit.LoadPreferences(path); p.Layout != "" || p.Effort != "low" {
		t.Fatalf("unknown layout = %+v", p)
	}
}

// The web cockpit saves the effort; the terminal's own choices stay.
func TestTerminalPreferencesSurviveTheEffort(t *testing.T) {
	path := cockpit.PreferencesPath(filepath.Join(t.TempDir(), "settings.json"))
	if err := cockpit.SaveChangesPanel(path, true); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveLayout(path, cockpit.LayoutCompact); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveEffort(path, "max"); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); !p.Changes || p.Layout != cockpit.LayoutCompact || p.Effort != "max" {
		t.Fatalf("preferences = %+v", p)
	}
	if err := cockpit.SaveChangesPanel(path, false); err != nil {
		t.Fatal(err)
	}
	if data, _ := os.ReadFile(path); string(data) != `{"effort":"max","tui_layout":"compact"}`+"\n" {
		t.Fatalf("file = %s", data)
	}

	// The panel's width, in columns, stays beside them; 0 is its default.
	if err := cockpit.SaveChangesWidth(path, 72); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveEffort(path, "low"); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); p.ChangesWidth != 72 || p.Layout != cockpit.LayoutCompact || p.Effort != "low" {
		t.Fatalf("preferences = %+v", p)
	}
	if err := cockpit.SaveChangesWidth(path, -3); err == nil {
		t.Fatal("saved a negative width")
	}
	if err := cockpit.SaveChangesWidth(path, 0); err != nil {
		t.Fatal(err)
	}
	if data, _ := os.ReadFile(path); string(data) != `{"effort":"low","tui_layout":"compact"}`+"\n" {
		t.Fatalf("file = %s", data)
	}
	os.WriteFile(path, []byte(`{"tui_changes_width":-40}`), 0o600)
	if p := cockpit.LoadPreferences(path); p.ChangesWidth != 0 {
		t.Fatalf("a negative width = %+v", p)
	}
}
