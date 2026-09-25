package cockpit

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
)

// Preferences are choices the cockpits keep that are not part of the
// connection: the effort the next runs think with, whichever cockpit set it,
// and how the terminal cockpit lays itself out. They live beside the
// settings file, which holds a key and is rewritten by the connection form;
// this file changes whenever a preference does.
type Preferences struct {
	Effort string `json:"effort,omitzero"`
	// Layout is how the terminal cockpit starts: LayoutFullscreen, in a
	// screen of its own, or LayoutCompact, below the command that started it.
	Layout string `json:"tui_layout,omitzero"`
	// Changes opens the terminal cockpit's changes panel as it starts.
	Changes bool `json:"tui_changes,omitzero"`
	// ChangesWidth is how many columns the terminal cockpit's changes panel
	// takes once its edge was dragged; 0 leaves the panel its default.
	ChangesWidth int `json:"tui_changes_width,omitzero"`
}

// Terminal cockpit layouts.
const (
	LayoutFullscreen = "fullscreen"
	LayoutCompact    = "compact"
)

// PreferencesPath is the preferences file that goes with a settings file,
// or "" when there is none.
func PreferencesPath(settingsFile string) string {
	if settingsFile == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(settingsFile), "preferences.json")
}

// LoadPreferences reads preferences. A missing or unreadable file has none,
// and values no cockpit understands are left out: preferences only ever
// fill in defaults.
func LoadPreferences(path string) Preferences {
	var p Preferences
	if path == "" {
		return p
	}
	if data, err := os.ReadFile(path); err == nil {
		_ = json.Unmarshal(data, &p)
	}
	if !ValidThinkingLevel(p.Effort) {
		p.Effort = ""
	}
	if p.Layout != LayoutFullscreen && p.Layout != LayoutCompact {
		p.Layout = ""
	}
	if p.ChangesWidth < 0 || p.ChangesWidth > maxPanelColumns {
		p.ChangesWidth = 0
	}
	return p
}

// maxPanelColumns is the most columns a saved panel width can be: wider
// than any terminal, and less than what a mistake would make it.
const maxPanelColumns = 2000

// SaveEffort records the effort, keeping the other preferences.
func SaveEffort(path, level string) error {
	if !ValidThinkingLevel(level) {
		return fmt.Errorf("unknown effort %q", level)
	}
	return savePreferences(path, func(p *Preferences) { p.Effort = level })
}

// SaveLayout records how the terminal cockpit starts, keeping the other
// preferences.
func SaveLayout(path, layout string) error {
	if layout != LayoutFullscreen && layout != LayoutCompact {
		return fmt.Errorf("unknown layout %q", layout)
	}
	return savePreferences(path, func(p *Preferences) { p.Layout = layout })
}

// SaveChangesPanel records whether the terminal cockpit starts with its
// changes panel open, keeping the other preferences.
func SaveChangesPanel(path string, open bool) error {
	return savePreferences(path, func(p *Preferences) { p.Changes = open })
}

// SaveChangesWidth records how many columns the terminal cockpit's changes
// panel takes (0 for its default), keeping the other preferences.
func SaveChangesWidth(path string, columns int) error {
	if columns < 0 || columns > maxPanelColumns {
		return fmt.Errorf("a panel %d columns wide", columns)
	}
	return savePreferences(path, func(p *Preferences) { p.ChangesWidth = columns })
}

// savePreferences changes the preferences. The file is replaced whole, so a
// cockpit reading it never sees half of it.
func savePreferences(path string, change func(*Preferences)) error {
	if path == "" {
		return errors.New("preferences are unavailable: set KOU_CONVEYOR_CONFIG")
	}
	p := LoadPreferences(path)
	change(&p)
	data, err := json.Marshal(p, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return fmt.Errorf("save preferences: %w", err)
	}
	if err := replaceFile(path, append(data, '\n')); err != nil {
		return fmt.Errorf("save preferences: %w", err)
	}
	return nil
}

// PreferencesChanged reports whether the preferences file is another than
// the one described by seen, which may be nil. Saving replaces the file, so
// every change makes it another file.
func PreferencesChanged(path string, seen fs.FileInfo) (fs.FileInfo, bool) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, seen != nil
	}
	return info, seen == nil || !os.SameFile(info, seen) || !info.ModTime().Equal(seen.ModTime())
}
