package main

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The colours an appearance sets, and the ones it leaves.
func appearanceColours() []lipgloss.AdaptiveColor {
	return []lipgloss.AdaptiveColor{
		colorAccent, colorText, colorMuted, colorFaint, colorGhost, colorRule, colorRule2, colorInk, colorSelect, colorChip,
		colorAccentSoft, colorSelection, colorSynText, colorSynComment, colorSynPunct, colorSynOp, colorSynProp,
		colorOK, colorWarn, colorErr, colorAdded, colorRemoved, colorSynKeyword, colorSynType, colorSynString,
	}
}

// The default appearance is the cockpit's look as it was before there was
// a choice; another one goes back to it colour for colour.
func TestAppearanceKeepsTheCockpitsOwnLook(t *testing.T) {
	t.Cleanup(func() { applyAppearance(cockpit.Appearance{}) })
	before := appearanceColours()
	if applyAppearance(cockpit.Appearance{Accent: "Orange", Shade: "graphite"}) {
		t.Fatal("orange on graphite is another look than the default")
	}
	if !applyAppearance(cockpit.Appearance{Accent: "blue", Shade: "slate", Radius: 8}) {
		t.Fatal("blue on slate is the look there was")
	}
	if colorAccent.Dark != "#4c8dff" || colorAccent.Light != "#2160e0" || colorText.Dark != "#e7ebf2" || corners != roundCorners {
		t.Fatalf("blue on slate: accent %+v, text %+v, corners %+v", colorAccent, colorText, corners)
	}
	if colorSynKeyword != before[22] || colorErr != before[19] {
		t.Fatal("the colours of state and of code's tokens changed with the accent")
	}
	applyAppearance(cockpit.Appearance{})
	for i, c := range appearanceColours() {
		if !strings.EqualFold(c.Light, before[i].Light) || !strings.EqualFold(c.Dark, before[i].Dark) {
			t.Errorf("colour %d is %+v again, was %+v", i, c, before[i])
		}
	}
	if corners != squareCorners {
		t.Fatalf("corners %+v", corners)
	}
	if !decoration("  ╭────╮") || !decoration("╰──") {
		t.Fatal("a row of round corners is not a decoration")
	}
}

// What /appearance chooses is the web cockpit's look too, and what the web
// cockpit chooses is this one's.
func TestAppearanceIsSharedWithTheWebCockpit(t *testing.T) {
	m := testModel(t)
	t.Cleanup(func() { applyAppearance(cockpit.Appearance{}) })
	m.opt.preferences = cockpit.PreferencesPath(filepath.Join(t.TempDir(), "settings.json"))
	if view := ansi.Strip(m.View()); !strings.Contains(view, "┌") || strings.Contains(view, "╭") {
		t.Fatalf("the default corners are not square:\n%s", view)
	}
	m.command("/appearance accent blue")
	m.command("/appearance shade slate")
	m.command("/corners round")
	saved := cockpit.LoadPreferences(m.opt.preferences).Appearance()
	if saved != (cockpit.Appearance{Accent: "blue", Shade: "slate", Radius: 12}) || !strings.Contains(m.note.text, "blue on slate, round corners") {
		t.Fatalf("saved %+v, notice %q", saved, m.note.text)
	}
	if colorAccent.Dark != "#4c8dff" {
		t.Fatalf("accent %+v", colorAccent)
	}
	if view := ansi.Strip(m.View()); !strings.Contains(view, "╭") || strings.Contains(view, "┌") {
		t.Fatalf("round corners are not drawn:\n%s", view)
	}
	m.command("/appearance accent chartreuse")
	if m.note.level != "error" || look.Accent != "blue" {
		t.Fatalf("an unknown accent: %q (%s), look %+v", m.note.text, m.note.level, look)
	}

	// A colour of one's own, square, chosen in the web cockpit.
	if err := cockpit.SaveAppearance(m.opt.preferences, cockpit.Appearance{Accent: "#1E3A8A"}); err != nil {
		t.Fatal(err)
	}
	m.Update(prefsMsg{})
	if look != (cockpit.Appearance{Accent: "#1e3a8a"}) || colorAccent.Dark != "#5376cc" || colorAccent.Light != "#1e3a8a" || corners != squareCorners {
		t.Fatalf("look %+v, accent %+v, corners %+v", look, colorAccent, corners)
	}
	if view := ansi.Strip(m.View()); strings.Contains(view, "╭") {
		t.Fatalf("the corners stayed round:\n%s", view)
	}
}

// The list of /appearance takes each choice through its values with ← and
// →, and enter steps on.
func TestAppearancePickerStepsThroughTheChoices(t *testing.T) {
	m := testModel(t)
	t.Cleanup(func() { applyAppearance(cockpit.Appearance{}) })
	m.opt.preferences = cockpit.PreferencesPath(filepath.Join(t.TempDir(), "settings.json"))
	m.command("/appearance")
	if m.picker == nil || m.picker.kind != "appearance" {
		t.Fatalf("picker %+v", m.picker)
	}
	m.Update(key("right"))
	if look.Accent != "amber" {
		t.Fatalf("→ on the accent: %+v", look)
	}
	m.Update(key("left"))
	m.Update(key("left"))
	if look.Accent != "mono" {
		t.Fatalf("← twice on the accent: %+v", look)
	}
	m.Update(key("down"))
	m.Update(key("down"))
	m.Update(key("enter"))
	if look.Radius != 4 || corners != roundCorners || m.picker == nil || m.picker.cursor != 2 {
		t.Fatalf("enter on the corners: %+v, corners %+v, picker %+v", look, corners, m.picker)
	}
	view := ansi.Strip(m.View())
	if !strings.Contains(view, "‹ soft · 4px ›") || !strings.Contains(view, "‹ mono ›") || !strings.Contains(view, "‹ graphite ›") {
		t.Fatalf("the list:\n%s", view)
	}
	if saved := cockpit.LoadPreferences(m.opt.preferences).Appearance(); saved != (cockpit.Appearance{Accent: "mono", Radius: 4}) {
		t.Fatalf("saved %+v", saved)
	}
	m.Update(key("down"))
	m.Update(key("enter"))
	if look != (cockpit.Appearance{}) || corners != squareCorners {
		t.Fatalf("the cockpit's own look: %+v", look)
	}
	if saved := cockpit.LoadPreferences(m.opt.preferences).Appearance(); saved != (cockpit.Appearance{}) {
		t.Fatalf("saved %+v", saved)
	}
	press(m, key("esc"))
	if m.picker != nil {
		t.Fatal("esc left the list open")
	}
}
