package cockpit

import (
	"os"
	"path/filepath"
	"testing"
)

// The default appearance is the cockpits' look as it was before there was
// a choice, colour for colour.
func TestDefaultAppearanceKeepsTheCockpitsLook(t *testing.T) {
	dark := Appearance{}.Palette(true)
	want := Palette{
		Accent: "#ff5b1f", AccentInk: "#0c0c0b", AccentSoft: "#3a1b0e", Selection: "#612912",
		BG: "#0c0c0b", BG2: "#121211", BG3: "#191917", BG4: "#22221f", Line: "#262623", Line2: "#373632",
		FG: "#ecebe6", FG2: "#b3b1a9", FG3: "#7f7d76", FG4: "#5a5852", Code: "#0f0f0e", Rule: "#34332f", Rule2: "#4a4944",
		SynFG: "#e3e2dc", SynComment: "#6d6b64", SynPunct: "#8c8a83", SynOp: "#b3b1a9", SynProp: "#cfcdc5",
	}
	if dark != want {
		t.Fatalf("dark = %+v\nwant %+v", dark, want)
	}
	light := Appearance{Accent: "orange", Shade: "graphite"}.Palette(false)
	want = Palette{
		Accent: "#e4470c", AccentInk: "#ffffff", AccentSoft: "#f3d6c8", Selection: "#f7cebb",
		BG: "#f4f3ef", BG2: "#fbfaf7", BG3: "#ecebe5", BG4: "#e2e0d8", Line: "#dcdad2", Line2: "#c6c3b9",
		FG: "#151513", FG2: "#46453f", FG3: "#77756c", FG4: "#a19e94", Code: "#efeee8", Rule: "#dcdad2", Rule2: "#c6c3b9",
		SynFG: "#1c1c19", SynComment: "#8f8c83", SynPunct: "#6f6c64", SynOp: "#55534c", SynProp: "#3d3c38",
	}
	if light != want {
		t.Fatalf("light = %+v\nwant %+v", light, want)
	}
	if got := (Appearance{Accent: "chartreuse", Shade: "plaid"}).Palette(true); got != (Appearance{}).Palette(true) {
		t.Fatalf("unknown presets = %+v", got)
	}
}

func TestAccentsReadOnTheirBackground(t *testing.T) {
	for _, accent := range append(Accents(), "#1e3a8a", "#ffe066", "#000000", "#ffffff", "#7C3AED") {
		for _, dark := range []bool{true, false} {
			p := Appearance{Accent: accent}.Palette(dark)
			need := 3.0
			if dark {
				need = 4.5
			}
			if got := contrastRatio(p.Accent, p.BG); got < need {
				t.Errorf("%s (dark %v) = %s: contrast %.2f on %s", accent, dark, p.Accent, got, p.BG)
			}
			if got := contrastRatio(p.Accent, p.AccentInk); got < 3 {
				t.Errorf("%s (dark %v): ink %s on %s has contrast %.2f", accent, dark, p.AccentInk, p.Accent, got)
			}
		}
	}
	// A colour that reads already is kept as it is.
	if got := (Appearance{Accent: "#10B981"}).Palette(true).Accent; got != "#10b981" {
		t.Fatalf("readable custom accent = %s", got)
	}
	if got := (Appearance{Accent: "#1e3a8a"}).Palette(false).Accent; got != "#1e3a8a" {
		t.Fatalf("readable custom accent in light = %s", got)
	}
}

func TestShadesTintTheGreys(t *testing.T) {
	neutral := Appearance{Shade: "neutral"}.Palette(true)
	for _, c := range []string{neutral.BG, neutral.Line2, neutral.FG, neutral.FG3} {
		if c[1:3] != c[3:5] || c[3:5] != c[5:7] {
			t.Errorf("neutral grey %s is tinted", c)
		}
	}
	graphite := Appearance{}.Palette(true)
	for _, shade := range Shades() {
		for _, dark := range []bool{true, false} {
			p := Appearance{Shade: shade}.Palette(dark)
			g := Appearance{}.Palette(dark)
			// The lightness stays: the text reads as well on every shade.
			if a, b := contrastRatio(p.FG3, p.BG), contrastRatio(g.FG3, g.BG); a < b-0.25 || a > b+0.25 {
				t.Errorf("%s (dark %v): faint text contrast %.2f, graphite's %.2f", shade, dark, a, b)
			}
		}
	}
	if slate := (Appearance{Shade: "slate"}).Palette(true); slate.BG == graphite.BG || hexOKLCH(slate.BG3).h < 200 {
		t.Fatalf("slate is not blue: %+v", slate)
	}
}

// The browser cockpit works the colours out in JavaScript
// (plugins/theme/web/appearance.js), the iPhone app in Swift: these are
// what the browser makes of them, and the terminal cockpit must agree.
func TestPaletteIsTheBrowsers(t *testing.T) {
	greys := func(p Palette) string { return p.BG + " " + p.BG3 + " " + p.Line2 + " " + p.FG + " " + p.FG3 }
	for _, c := range []struct {
		shade string
		dark  bool
		want  string
	}{
		{"slate", true, "#0b0c0e #16191d #31373e #e7ebf2 #777e87"},
		{"midnight", false, "#f0f3fa #e6ebf5 #bbc3d4 #141517 #6f7580"},
		{"neutral", true, "#0c0c0c #191919 #363636 #ebebeb #7d7d7d"},
		{"mocha", true, "#0e0b09 #1d1814 #3f332c #f2e9e3 #877a72"},
	} {
		if got := greys(Appearance{Shade: c.shade}.Palette(c.dark)); got != c.want {
			t.Errorf("%s (dark %v) = %s, the browser's %s", c.shade, c.dark, got, c.want)
		}
	}
	code := func(p Palette) string {
		return p.SynFG + " " + p.SynComment + " " + p.SynPunct + " " + p.SynOp + " " + p.SynProp
	}
	if got := code(Appearance{Shade: "slate"}.Palette(true)); got != "#dde2ea #656c75 #848b94 #aab2bc #c6ced8" {
		t.Errorf("slate's code in dark = %s", got)
	}
	if got := code(Appearance{Shade: "mocha"}.Palette(false)); got != "#1e1b19 #948a84 #736a65 #59514d #3f3b38" {
		t.Errorf("mocha's code in light = %s", got)
	}
	for accent, want := range map[string][2]string{
		"#1e3a8a": {"#5376cc", "#1e3a8a"},
		"#ffe066": {"#ffe066", "#a68a00"},
		"#7c3aed": {"#8d54ff", "#7c3aed"},
		"#000000": {"#7a7a7a", "#000000"},
	} {
		if dark, light := (Appearance{Accent: accent}).Palette(true).Accent, (Appearance{Accent: accent}).Palette(false).Accent; dark != want[0] || light != want[1] {
			t.Errorf("%s = %s and %s, the browser's %s", accent, dark, light, want)
		}
	}
}

func TestAppearanceIsSharedThroughPreferences(t *testing.T) {
	path := filepath.Join(t.TempDir(), "preferences.json")
	if err := SaveAccent(path, " Blue "); err != nil {
		t.Fatal(err)
	}
	if err := SaveShade(path, "slate"); err != nil {
		t.Fatal(err)
	}
	if err := SaveRadius(path, 8); err != nil {
		t.Fatal(err)
	}
	if err := SaveEffort(path, "max"); err != nil {
		t.Fatal(err)
	}
	if got := LoadPreferences(path).Appearance(); got != (Appearance{Accent: "blue", Shade: "slate", Radius: 8}) {
		t.Fatalf("saved = %+v", got)
	}
	if err := SaveAccent(path, "#A1B2C3"); err != nil || LoadPreferences(path).Accent != "#a1b2c3" {
		t.Fatalf("custom accent: %v %+v", err, LoadPreferences(path))
	}
	for _, bad := range []string{"#abc", "a1b2c3", "#a1b2c3ff", "chartreuse", ""} {
		if err := SaveAccent(path, bad); err == nil {
			t.Errorf("saved accent %q", bad)
		}
	}
	if err := SaveShade(path, "plaid"); err == nil {
		t.Error("saved an unknown shade")
	}
	for _, bad := range []int{-1, MaxRadius + 1} {
		if err := SaveRadius(path, bad); err == nil {
			t.Errorf("saved radius %d", bad)
		}
	}
	// The defaults are no choice at all.
	SaveAccent(path, "orange")
	SaveShade(path, "graphite")
	SaveRadius(path, 0)
	if p := LoadPreferences(path); p.Appearance() != (Appearance{}) || p.Effort != "max" {
		t.Fatalf("after choosing the defaults: %+v", p)
	}
	if data, _ := os.ReadFile(path); string(data) != "{\"effort\":\"max\"}\n" {
		t.Fatalf("file = %s", data)
	}
	// What no cockpit understands only fills in nothing.
	os.WriteFile(path, []byte(`{"accent":"#12345","shade":"plaid","radius":99}`), 0o600)
	if got := LoadPreferences(path).Appearance(); got != (Appearance{}) {
		t.Fatalf("unknown appearance = %+v", got)
	}
}
