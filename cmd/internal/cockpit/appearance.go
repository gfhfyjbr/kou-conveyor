package cockpit

import (
	"fmt"
	"math"
	"slices"
	"strconv"
	"strings"
)

// Appearance is how both cockpits look beyond light and dark: the accent
// colour, the shade of their greys and how round their corners are. The
// zero Appearance is the cockpits' own look: orange on graphite, with
// square corners.
type Appearance struct {
	// Accent is a preset (Accents) or a colour of the user's, #rrggbb;
	// "" is DefaultAccent.
	Accent string
	// Shade is a preset of greys (Shades); "" is DefaultShade.
	Shade string
	// Radius is how round corners are, in CSS pixels: 0 keeps them square,
	// MaxRadius is the roundest.
	Radius int
}

// Appearance is how the preferences say the cockpits look.
func (p Preferences) Appearance() Appearance {
	return Appearance{Accent: p.Accent, Shade: p.Shade, Radius: p.Radius}
}

// The cockpits' own look, and the roundest corners there are.
const (
	DefaultAccent = "orange"
	DefaultShade  = "graphite"
	MaxRadius     = 16
)

// Accent presets: the colour in dark and in light. The light one is darker,
// to read on a light background.
var accentPresets = []struct{ name, dark, light string }{
	{"orange", "#ff5b1f", "#e4470c"},
	{"amber", "#f5a524", "#a16207"},
	{"green", "#3ecf72", "#18853f"},
	{"teal", "#22c3b0", "#0b7f72"},
	{"blue", "#4c8dff", "#2160e0"},
	{"violet", "#9d7bff", "#6d44e0"},
	{"pink", "#ff5c9d", "#d0246c"},
	{"mono", "#ecebe6", "#151513"},
}

// Shade presets tint graphite's greys: each keeps its lightness, and takes
// the hue and graphite's chroma times dark or light. Graphite is the greys
// as they are.
var shadePresets = []struct {
	name             string
	hue, dark, light float64
}{
	{"graphite", 0, 1, 1},
	{"neutral", 0, 0, 0},
	{"slate", 255, 2.2, 1.0},
	{"midnight", 264, 5.0, 1.8},
	{"sage", 165, 2.0, 0.9},
	{"mauve", 305, 2.2, 1.0},
	{"mocha", 55, 3.0, 1.2},
}

// Accents are the accent presets, in the order the cockpits offer them.
func Accents() []string {
	names := make([]string, len(accentPresets))
	for i, a := range accentPresets {
		names[i] = a.name
	}
	return names
}

// Shades are the shade presets, in the order the cockpits offer them.
func Shades() []string {
	names := make([]string, len(shadePresets))
	for i, s := range shadePresets {
		names[i] = s.name
	}
	return names
}

// NormalizeAccent is the accent as preferences keep it: a preset's name, or
// a colour as #rrggbb in lower case.
func NormalizeAccent(accent string) (string, error) {
	accent = strings.ToLower(strings.TrimSpace(accent))
	if slices.Contains(Accents(), accent) {
		return accent, nil
	}
	if _, _, _, ok := parseHex(accent); ok {
		return accent, nil
	}
	return "", fmt.Errorf("accent must be %s or a colour as #rrggbb", strings.Join(Accents(), ", "))
}

// ValidShade reports whether shade is a shade preset.
func ValidShade(shade string) bool { return slices.Contains(Shades(), shade) }

// ValidRadius reports whether corners can be radius pixels round.
func ValidRadius(radius int) bool { return radius >= 0 && radius <= MaxRadius }

// SaveAccent records the accent of both cockpits, keeping the other
// preferences. The default is recorded as no choice.
func SaveAccent(path, accent string) error {
	accent, err := NormalizeAccent(accent)
	if err != nil {
		return err
	}
	if accent == DefaultAccent {
		accent = ""
	}
	return savePreferences(path, func(p *Preferences) { p.Accent = accent })
}

// SaveShade records the shade of both cockpits' greys, keeping the other
// preferences. The default is recorded as no choice.
func SaveShade(path, shade string) error {
	if !ValidShade(shade) {
		return fmt.Errorf("shade must be %s", strings.Join(Shades(), ", "))
	}
	if shade == DefaultShade {
		shade = ""
	}
	return savePreferences(path, func(p *Preferences) { p.Shade = shade })
}

// SaveRadius records how round both cockpits' corners are, keeping the
// other preferences.
func SaveRadius(path string, radius int) error {
	if !ValidRadius(radius) {
		return fmt.Errorf("radius must be 0 to %d", MaxRadius)
	}
	return savePreferences(path, func(p *Preferences) { p.Radius = radius })
}

// SaveAppearance records a whole appearance at once, keeping the other
// preferences. What no cockpit understands of it is recorded as the
// default, as LoadPreferences reads it.
func SaveAppearance(path string, a Appearance) error {
	a = a.Normal()
	return savePreferences(path, func(p *Preferences) { p.Accent, p.Shade, p.Radius = a.Accent, a.Shade, a.Radius })
}

// Normal is the appearance as preferences keep it: the accent normalized,
// the defaults as no choice and what no cockpit understands left out.
// Appearances that look alike are equal when normal.
func (a Appearance) Normal() Appearance {
	p := Preferences{Accent: a.Accent, Shade: a.Shade, Radius: a.Radius}
	normalAppearance(&p)
	return p.Appearance()
}

// normalAppearance leaves out of loaded preferences what no cockpit
// understands, and the defaults.
func normalAppearance(p *Preferences) {
	if accent, err := NormalizeAccent(p.Accent); err != nil || accent == DefaultAccent {
		p.Accent = ""
	} else {
		p.Accent = accent
	}
	if !ValidShade(p.Shade) || p.Shade == DefaultShade {
		p.Shade = ""
	}
	if !ValidRadius(p.Radius) {
		p.Radius = 0
	}
}

// Palette is the colours of an appearance, in light or in dark, as #rrggbb.
// The browser cockpit's tokens of the same names are these.
type Palette struct {
	// Accent is the accent colour; AccentInk is what is written on it.
	Accent, AccentInk string
	// AccentSoft and Selection are the accent laid over the background:
	// faintly, for a mark, and more, for a selection.
	AccentSoft, Selection string
	// The backgrounds, from the page's to a raised chip's, the hairlines
	// between them, and the text, from the strongest to the faintest.
	BG, BG2, BG3, BG4, Line, Line2, FG, FG2, FG3, FG4, Code string
	// Rule and Rule2 are the terminal cockpit's lines, which a terminal
	// draws a whole cell wide: brighter in dark than the browser's.
	Rule, Rule2 string
	// The greys of code: its text, comments, punctuation, operators and
	// properties. The colours of its other tokens stay as they are.
	SynFG, SynComment, SynPunct, SynOp, SynProp string
}

// The greys a shade tints, in graphite.
const (
	greyBG = iota
	greyBG2
	greyBG3
	greyBG4
	greyLine
	greyLine2
	greyFG
	greyFG2
	greyFG3
	greyFG4
	greyCode
	greyRule
	greyRule2
	greySynFG
	greySynComment
	greySynPunct
	greySynOp
	greySynProp
	greys
)

var graphite = [2][greys]string{
	{"#0c0c0b", "#121211", "#191917", "#22221f", "#262623", "#373632", "#ecebe6", "#b3b1a9", "#7f7d76", "#5a5852", "#0f0f0e", "#34332f", "#4a4944",
		"#e3e2dc", "#6d6b64", "#8c8a83", "#b3b1a9", "#cfcdc5"},
	{"#f4f3ef", "#fbfaf7", "#ecebe5", "#e2e0d8", "#dcdad2", "#c6c3b9", "#151513", "#46453f", "#77756c", "#a19e94", "#efeee8", "#dcdad2", "#c6c3b9",
		"#1c1c19", "#8f8c83", "#6f6c64", "#55534c", "#3d3c38"},
}

// The cockpits' own orange laid over their backgrounds, in dark and in
// light: tuned by hand, where other accents are mixed.
var orangeSoft = [2][2]string{{"#3a1b0e", "#612912"}, {"#f3d6c8", "#f7cebb"}}

// Palette is the colours of the appearance in dark, or else in light.
// Values no cockpit understands are the defaults.
func (a Appearance) Palette(dark bool) Palette {
	mode := 1
	if dark {
		mode = 0
	}
	var g [greys]string
	shade := shadePresets[0]
	for _, s := range shadePresets {
		if s.name == a.Shade {
			shade = s
		}
	}
	for i, hex := range graphite[mode] {
		if shade.name == DefaultShade {
			g[i] = hex
			continue
		}
		k := shade.light
		if dark {
			k = shade.dark
		}
		if (i >= greyFG && i <= greyFG4) || i >= greySynFG {
			// Tinted text reads as coloured sooner than a tinted ground.
			k = min(k, 1+(k-1)*0.4)
		}
		c := hexOKLCH(hex)
		g[i] = oklchHex(c.l, c.c*k, shade.hue)
	}
	p := Palette{
		BG: g[greyBG], BG2: g[greyBG2], BG3: g[greyBG3], BG4: g[greyBG4], Line: g[greyLine], Line2: g[greyLine2],
		FG: g[greyFG], FG2: g[greyFG2], FG3: g[greyFG3], FG4: g[greyFG4], Code: g[greyCode], Rule: g[greyRule], Rule2: g[greyRule2],
		SynFG: g[greySynFG], SynComment: g[greySynComment], SynPunct: g[greySynPunct], SynOp: g[greySynOp], SynProp: g[greySynProp],
	}
	p.Accent = accentColour(a.Accent, dark)
	if dark {
		p.AccentInk = p.BG
	} else {
		p.AccentInk = "#ffffff"
	}
	accent, _ := NormalizeAccent(a.Accent)
	switch {
	case accent == "" || accent == DefaultAccent:
		p.AccentSoft, p.Selection = orangeSoft[mode][0], orangeSoft[mode][1]
	case dark:
		p.AccentSoft, p.Selection = mixHex(p.Accent, p.BG, 0.2), mixHex(p.Accent, p.BG, 0.35)
	default:
		p.AccentSoft, p.Selection = mixHex(p.Accent, "#ffffff", 0.22), mixHex(p.Accent, "#ffffff", 0.28)
	}
	return p
}

// accentColour is an accent in dark or in light. A colour of the user's is
// made lighter in dark and darker in light until it reads on the
// background; presets read as they are.
func accentColour(accent string, dark bool) string {
	accent, err := NormalizeAccent(accent)
	if err != nil {
		accent = DefaultAccent
	}
	for _, a := range accentPresets {
		if a.name == accent {
			if dark {
				return a.dark
			}
			return a.light
		}
	}
	if dark {
		return fitContrast(accent, graphite[0][greyBG], 4.5, true)
	}
	return fitContrast(accent, graphite[1][greyBG], 3, false)
}

// ---------------------------------------------------------------- colour

// The colour arithmetic is OKLCH's (Björn Ottosson's Oklab), which keeps
// lightness as the eye sees it while hue and chroma change. The browser
// cockpit and the iPhone app do the same.

type lch struct{ l, c, h float64 }

func parseHex(s string) (r, g, b float64, ok bool) {
	if len(s) != 7 || s[0] != '#' {
		return 0, 0, 0, false
	}
	v, err := strconv.ParseUint(s[1:], 16, 32)
	if err != nil {
		return 0, 0, 0, false
	}
	return float64(v>>16&0xff) / 255, float64(v>>8&0xff) / 255, float64(v&0xff) / 255, true
}

func toLinear(c float64) float64 {
	if c <= 0.04045 {
		return c / 12.92
	}
	return math.Pow((c+0.055)/1.055, 2.4)
}

func fromLinear(c float64) float64 {
	c = max(0, min(1, c))
	if c <= 0.0031308 {
		return 12.92 * c
	}
	return 1.055*math.Pow(c, 1/2.4) - 0.055
}

func hexOKLCH(hex string) lch {
	r, g, b, _ := parseHex(hex)
	r, g, b = toLinear(r), toLinear(g), toLinear(b)
	l := math.Cbrt(0.4122214708*r + 0.5363325363*g + 0.0514459929*b)
	m := math.Cbrt(0.2119034982*r + 0.6806995451*g + 0.1073969566*b)
	s := math.Cbrt(0.0883024619*r + 0.2817188376*g + 0.6299787005*b)
	L := 0.2104542553*l + 0.7936177850*m - 0.0040720468*s
	A := 1.9779984951*l - 2.4285922050*m + 0.4505937099*s
	B := 0.0259040371*l + 0.7827717662*m - 0.8086757660*s
	return lch{L, math.Hypot(A, B), math.Mod(math.Atan2(B, A)*180/math.Pi+360, 360)}
}

func oklchLinear(l, c, h float64) (r, g, b float64) {
	A, B := c*math.Cos(h*math.Pi/180), c*math.Sin(h*math.Pi/180)
	l1 := l + 0.3963377774*A + 0.2158037573*B
	m1 := l - 0.1055613458*A - 0.0638541728*B
	s1 := l - 0.0894841775*A - 1.2914855480*B
	l1, m1, s1 = l1*l1*l1, m1*m1*m1, s1*s1*s1
	return 4.0767416621*l1 - 3.3077115913*m1 + 0.2309699292*s1,
		-1.2684380046*l1 + 2.6097574011*m1 - 0.3413193965*s1,
		-0.0041960863*l1 - 0.7034186147*m1 + 1.7076147010*s1
}

func inGamut(l, c, h float64) bool {
	r, g, b := oklchLinear(l, c, h)
	const e = 1e-4
	return r >= -e && r <= 1+e && g >= -e && g <= 1+e && b >= -e && b <= 1+e
}

// oklchHex is a colour as #rrggbb, its chroma reduced to what sRGB shows.
func oklchHex(l, c, h float64) string {
	if !inGamut(l, c, h) {
		lo, hi := 0.0, c
		for range 30 {
			if mid := (lo + hi) / 2; inGamut(l, mid, h) {
				lo = mid
			} else {
				hi = mid
			}
		}
		c = lo
	}
	r, g, b := oklchLinear(l, c, h)
	return rgbHex(fromLinear(r), fromLinear(g), fromLinear(b))
}

func rgbHex(r, g, b float64) string {
	byteOf := func(v float64) int { return int(math.Round(max(0, min(1, v)) * 255)) }
	return fmt.Sprintf("#%02x%02x%02x", byteOf(r), byteOf(g), byteOf(b))
}

func luminance(hex string) float64 {
	r, g, b, _ := parseHex(hex)
	return 0.2126*toLinear(r) + 0.7152*toLinear(g) + 0.0722*toLinear(b)
}

// contrastRatio is WCAG's contrast between two colours, 1 to 21.
func contrastRatio(a, b string) float64 {
	la, lb := luminance(a), luminance(b)
	return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
}

// fitContrast makes a colour lighter, or darker, keeping its hue and
// chroma, until it has the contrast needed against a background.
func fitContrast(hex, bg string, need float64, lighter bool) string {
	if contrastRatio(hex, bg) >= need {
		return hex
	}
	c := hexOKLCH(hex)
	lo, hi := 0.0, c.l
	if lighter {
		lo, hi = c.l, 1
	}
	for range 40 {
		mid := (lo + hi) / 2
		ok := contrastRatio(oklchHex(mid, c.c, c.h), bg) >= need
		if ok == lighter {
			hi = mid
		} else {
			lo = mid
		}
	}
	if lighter {
		return oklchHex(hi, c.c, c.h)
	}
	return oklchHex(lo, c.c, c.h)
}

// mixHex is t of a over the rest of b.
func mixHex(a, b string, t float64) string {
	ar, ag, ab, _ := parseHex(a)
	br, bg, bb, _ := parseHex(b)
	return rgbHex(ar*t+br*(1-t), ag*t+bg*(1-t), ab*t+bb*(1-t))
}
