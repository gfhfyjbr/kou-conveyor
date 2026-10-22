package main

import (
	"cmp"
	"slices"
	"strconv"
	"strings"

	"github.com/charmbracelet/bubbles/textarea"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// look is the appearance the cockpit is drawn in: the accent, the shade of
// the greys and the corners, chosen here or in the web cockpit — the
// preferences keep it for both. The colours (styles.go) and the corners
// below follow it, and the styles are built of them.
var look cockpit.Appearance

// boxCorners are the glyphs at the corners of a box. A terminal has one
// round corner, of light lines, so any radius the web cockpit draws is
// round here. A card's coloured edge is heavy while square, as the web
// cockpit's 2px bar; round, it is light, as the web cockpit then has no
// bar.
type boxCorners struct{ tl, tr, bl, br, heavyTop, heavySide, heavyBottom string }

var (
	squareCorners = boxCorners{"┌", "┐", "└", "┘", "┎", "┃", "┖"}
	roundCorners  = boxCorners{"╭", "╮", "╰", "╯", "╭", "│", "╰"}
	corners       = squareCorners
)

// cornerPresets are the web cockpit's corners, by name.
var cornerPresets = []struct {
	name   string
	radius int
}{{"sharp", 0}, {"soft", 4}, {"rounded", 8}, {"round", 12}}

// applyAppearance sets the colours and the corners of a, and reports
// whether they are others than they were: the styles are then built anew
// (restyle). The colours of state, of diffs and of code's own tokens stay.
func applyAppearance(a cockpit.Appearance) bool {
	a = a.Normal()
	if a == look {
		return false
	}
	look = a
	dark, light := a.Palette(true), a.Palette(false)
	set := func(c *lipgloss.AdaptiveColor, light, dark string) {
		*c = lipgloss.AdaptiveColor{Light: light, Dark: dark}
	}
	set(&colorAccent, light.Accent, dark.Accent)
	set(&colorText, light.FG, dark.FG)
	set(&colorMuted, light.FG2, dark.FG2)
	set(&colorFaint, light.FG3, dark.FG3)
	set(&colorGhost, light.FG4, dark.FG4)
	set(&colorRule, light.Rule, dark.Rule)
	set(&colorRule2, light.Rule2, dark.Rule2)
	set(&colorInk, light.AccentInk, dark.AccentInk)
	set(&colorSelect, light.BG3, dark.BG3)
	set(&colorChip, light.BG4, dark.BG4)
	set(&colorAccentSoft, light.AccentSoft, dark.AccentSoft)
	set(&colorSelection, light.Selection, dark.Selection)
	set(&colorSynText, light.SynFG, dark.SynFG)
	set(&colorSynComment, light.SynComment, dark.SynComment)
	set(&colorSynPunct, light.SynPunct, dark.SynPunct)
	set(&colorSynOp, light.SynOp, dark.SynOp)
	set(&colorSynProp, light.SynProp, dark.SynProp)
	corners = squareCorners
	if a.Radius > 0 {
		corners = roundCorners
	}
	return true
}

// styleComposer gives the composer the styles of its text, its placeholder
// and its edge.
func styleComposer(in *textarea.Model, st styles) {
	focused, blurred := textarea.DefaultStyles()
	focused.CursorLine = st.text
	focused.Text = st.text
	focused.Placeholder = st.ghost
	focused.Prompt = st.rule2
	focused.EndOfBuffer = st.ghost
	blurred.Prompt = st.rule2
	blurred.Placeholder = st.ghost
	in.FocusedStyle, in.BlurredStyle = focused, blurred
}

// restyle draws the cockpit anew in the colours and the corners applied:
// the styles, the fields that keep their own and every entry drawn in the
// old ones.
func (m *uiModel) restyle() {
	st := newStyles(m.opt.noColor)
	m.styles = st
	styleComposer(&m.input, st)
	// The textarea draws with the styles of its state as last set.
	if m.input.Focused() {
		m.input.Focus()
	} else {
		m.input.Blur()
	}
	if m.picker != nil {
		m.picker.query.PromptStyle, m.picker.query.PlaceholderStyle = st.accent, st.faint
		if m.picker.kind == "appearance" {
			cursor := m.picker.cursor
			m.picker.setItems(m.appearanceItems())
			m.picker.cursor = cursor
		}
	}
	if m.form != nil {
		for i := range m.form.fields {
			f := &m.form.fields[i]
			f.TextStyle, f.PlaceholderStyle, f.Cursor.Style = st.text, st.faint, st.accent
		}
	}
	clear(m.cache)
	m.changes.drawn = drawnDiff{}
	m.refreshKeep()
}

// chooseAppearance draws the cockpit in a and keeps it for both cockpits;
// the web cockpit takes it up when it is next looked at. Without the
// preferences it holds for this window.
func (m *uiModel) chooseAppearance(a cockpit.Appearance) tea.Cmd {
	a = a.Normal()
	if applyAppearance(a) {
		m.restyle()
	}
	m.prefs.Accent, m.prefs.Shade, m.prefs.Radius = a.Accent, a.Shade, a.Radius
	if err := cockpit.SaveAppearance(m.opt.preferences, a); err != nil {
		return m.notify("appearance of this window only: "+err.Error(), "warn")
	}
	m.prefsSeen, _ = cockpit.PreferencesChanged(m.opt.preferences, nil)
	return nil
}

// appearanceCommand is /appearance: alone, the list; then the accent, the
// shade or the corners named, or reset to the cockpit's own look. A value
// alone says what it is, as in the web cockpit.
func (m *uiModel) appearanceCommand(arg string) tea.Cmd {
	fields := strings.Fields(strings.ToLower(arg))
	if len(fields) == 0 {
		return m.openPicker("appearance")
	}
	what, value := fields[0], strings.Join(fields[1:], " ")
	next := look
	switch {
	case what == "reset" && value == "":
		next = cockpit.Appearance{}
	case what == "accent" && validAccent(value):
		next.Accent = value
	case what == "shade" && cockpit.ValidShade(value):
		next.Shade = value
	case (what == "corners" || what == "radius") && radiusOf(value) >= 0:
		next.Radius = radiusOf(value)
	case value == "" && validAccent(what):
		next.Accent = what
	case value == "" && cockpit.ValidShade(what):
		next.Shade = what
	case value == "" && radiusOf(what) >= 0:
		next.Radius = radiusOf(what)
	default:
		names := make([]string, len(cornerPresets))
		for i, c := range cornerPresets {
			names[i] = c.name
		}
		return m.notify("/appearance accent "+strings.Join(cockpit.Accents(), "|")+"|#rrggbb · shade "+strings.Join(cockpit.Shades(), "|")+
			" · corners "+strings.Join(names, "|")+"|0–"+strconv.Itoa(cockpit.MaxRadius)+" · reset", "error")
	}
	if cmd := m.chooseAppearance(next); cmd != nil {
		return cmd
	}
	return m.notify("appearance: "+describeAppearance(look), "info")
}

func validAccent(accent string) bool {
	_, err := cockpit.NormalizeAccent(accent)
	return accent != "" && err == nil
}

// radiusOf is the radius of a corners preset's name or of a number of
// pixels, or -1.
func radiusOf(value string) int {
	for _, c := range cornerPresets {
		if c.name == value {
			return c.radius
		}
	}
	n, err := strconv.Atoi(strings.TrimSuffix(value, "px"))
	if err != nil || !cockpit.ValidRadius(n) {
		return -1
	}
	return n
}

// cornersName is a radius by its preset's name, or in pixels.
func cornersName(radius int) string {
	for _, c := range cornerPresets {
		if c.radius == radius {
			return c.name
		}
	}
	return strconv.Itoa(radius) + "px"
}

func describeAppearance(a cockpit.Appearance) string {
	return cmp.Or(a.Accent, cockpit.DefaultAccent) + " on " + cmp.Or(a.Shade, cockpit.DefaultShade) + ", " + cornersName(a.Radius) + " corners"
}

// appearanceItems are the rows of /appearance: ← and → take the accent,
// the shade and the corners through their choices, as enter does forwards.
func (m *uiModel) appearanceItems() []pickerItem {
	step := func(what string) func(*uiModel) tea.Cmd {
		return func(m *uiModel) tea.Cmd { return m.stepAppearance(what, 1) }
	}
	choice := func(value string) string { return "‹ " + value + " ›" }
	radius := cornersName(look.Radius)
	if look.Radius > 0 {
		radius += " · " + strconv.Itoa(look.Radius) + "px"
	}
	return []pickerItem{
		{title: "Accent", detail: choice(cmp.Or(look.Accent, cockpit.DefaultAccent)), id: "accent", action: step("accent")},
		{title: "Shade of the greys", detail: choice(cmp.Or(look.Shade, cockpit.DefaultShade)), id: "shade", action: step("shade")},
		{title: "Corners", detail: choice(radius), id: "corners", action: step("corners")},
		{title: "The cockpit's own look", detail: "orange on graphite, sharp", id: "reset", current: look == cockpit.Appearance{},
			action: func(m *uiModel) tea.Cmd { return m.chooseAppearance(cockpit.Appearance{}) }},
	}
}

// appearanceKey takes ← and → in /appearance.
func (m *uiModel) appearanceKey(msg tea.KeyMsg) (tea.Cmd, bool) {
	step := map[string]int{"left": -1, "h": -1, "right": 1, "l": 1}[msg.String()]
	item := m.picker.selected()
	if step == 0 || item == nil {
		return nil, false
	}
	return m.stepAppearance(item.id, step), true
}

// stepAppearance takes the accent, the shade or the corners step choices
// on, around the end; a colour of one's own or a radius between presets
// steps to the first or the last.
func (m *uiModel) stepAppearance(what string, step int) tea.Cmd {
	cycle := func(choices []string, current string) string {
		i := slices.Index(choices, current)
		switch {
		case i < 0 && step > 0:
			return choices[0]
		case i < 0:
			return choices[len(choices)-1]
		}
		return choices[((i+step)%len(choices)+len(choices))%len(choices)]
	}
	next := look
	switch what {
	case "accent":
		next.Accent = cycle(cockpit.Accents(), cmp.Or(look.Accent, cockpit.DefaultAccent))
	case "shade":
		next.Shade = cycle(cockpit.Shades(), cmp.Or(look.Shade, cockpit.DefaultShade))
	case "corners":
		names := make([]string, len(cornerPresets))
		for i, c := range cornerPresets {
			names[i] = c.name
		}
		next.Radius = radiusOf(cycle(names, cornersName(look.Radius)))
	default:
		return nil
	}
	return m.chooseAppearance(next)
}
