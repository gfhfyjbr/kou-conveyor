package main

import "github.com/charmbracelet/lipgloss"

// One accent on a graphite/paper base, the web cockpit's tokens: fg to fg-4
// are text, muted, faint and ghost; line and line-2 are the hairlines.
// Everything else is weight, dimness and rules; colour carries state, and
// the tokens of code.
var (
	colorAccent = lipgloss.AdaptiveColor{Light: "#E4470C", Dark: "#FF5B1F"}
	colorText   = lipgloss.AdaptiveColor{Light: "#151513", Dark: "#ECEBE6"}
	colorMuted  = lipgloss.AdaptiveColor{Light: "#46453F", Dark: "#B3B1A9"}
	colorFaint  = lipgloss.AdaptiveColor{Light: "#77756C", Dark: "#7F7D76"}
	colorGhost  = lipgloss.AdaptiveColor{Light: "#A19E94", Dark: "#5A5852"}
	colorRule   = lipgloss.AdaptiveColor{Light: "#DCDAD2", Dark: "#34332F"}
	colorRule2  = lipgloss.AdaptiveColor{Light: "#C6C3B9", Dark: "#4A4944"}
	colorOK     = lipgloss.AdaptiveColor{Light: "#2E7A34", Dark: "#9CCF83"}
	colorWarn   = lipgloss.AdaptiveColor{Light: "#9A6200", Dark: "#EFB443"}
	colorErr    = lipgloss.AdaptiveColor{Light: "#CC1F3D", Dark: "#FF5468"}
	colorInk    = lipgloss.AdaptiveColor{Light: "#FFFFFF", Dark: "#0C0C0B"}
	// The raised surfaces: a selected or hovered row (bg-3), and chips and
	// keys (bg-4).
	colorSelect = lipgloss.AdaptiveColor{Light: "#ECEBE5", Dark: "#191917"}
	colorChip   = lipgloss.AdaptiveColor{Light: "#E2E0D8", Dark: "#22221F"}
	// The accent laid thinly over the base: a running state, a forced prompt.
	colorAccentSoft = lipgloss.AdaptiveColor{Light: "#F3D6C8", Dark: "#3A1B0E"}
	// Selected text: the accent over the base, like the web cockpit's.
	colorSelection = lipgloss.AdaptiveColor{Light: "#F7CEBB", Dark: "#612912"}
	// Lines a diff adds and removes, tinted like the web cockpit's.
	colorAdded   = lipgloss.AdaptiveColor{Light: "#E3F1E0", Dark: "#18241A"}
	colorRemoved = lipgloss.AdaptiveColor{Light: "#FAE3E5", Dark: "#2C181B"}
	// The tokens of code, in the web cockpit's syntax colours (--syn-*).
	colorSynText    = lipgloss.AdaptiveColor{Light: "#1C1C19", Dark: "#E3E2DC"}
	colorSynKeyword = lipgloss.AdaptiveColor{Light: "#C23D0A", Dark: "#FF8A57"}
	colorSynType    = lipgloss.AdaptiveColor{Light: "#2D5F9A", Dark: "#7AA7D9"}
	colorSynConst   = lipgloss.AdaptiveColor{Light: "#9C3D87", Dark: "#D28BC4"}
	colorSynBuiltin = lipgloss.AdaptiveColor{Light: "#1F7A72", Dark: "#6FC2B8"}
	colorSynFunc    = lipgloss.AdaptiveColor{Light: "#8A5A00", Dark: "#EFC36E"}
	colorSynString  = lipgloss.AdaptiveColor{Light: "#2E7A34", Dark: "#9CCF83"}
	colorSynComment = lipgloss.AdaptiveColor{Light: "#8F8C83", Dark: "#6D6B64"}
	colorSynPunct   = lipgloss.AdaptiveColor{Light: "#6F6C64", Dark: "#8C8A83"}
	colorSynOp      = lipgloss.AdaptiveColor{Light: "#55534C", Dark: "#B3B1A9"}
	colorSynProp    = lipgloss.AdaptiveColor{Light: "#3D3C38", Dark: "#CFCDC5"}
)

// With NO_COLOR the renderer drops every attribute, so state must also be
// legible from glyphs alone; noColor lets views pick glyphs that are.
type styles struct {
	noColor bool

	text, muted, faint, ghost, rule, rule2, accent, ok, warn, err lipgloss.Style
	label, accentLabel, errLabel                                  lipgloss.Style
	code, codeBlock, quote, bold, link                            lipgloss.Style
	key, keyHint, selected, selection, toast                      lipgloss.Style
	hover, hoverAction, hoverBackground                           lipgloss.Style
	diffAdd, diffDel, diffHunk                                    lipgloss.Style
	// chip is a raised label; button the accent one, and buttonOff a button
	// that does nothing now.
	chip, chipAccent, button, buttonOff lipgloss.Style
	// state are the run-state chips of the bar, by phase.
	state map[string]lipgloss.Style
	// syntax colours the tokens of code by their class (highlight.Classes).
	syntax map[string]lipgloss.Style
}

func newStyles(noColor bool) styles {
	s := lipgloss.NewStyle()
	st := styles{
		noColor:     noColor,
		text:        s.Foreground(colorText),
		muted:       s.Foreground(colorMuted),
		faint:       s.Foreground(colorFaint),
		ghost:       s.Foreground(colorGhost),
		rule:        s.Foreground(colorRule),
		rule2:       s.Foreground(colorRule2),
		accent:      s.Foreground(colorAccent),
		ok:          s.Foreground(colorOK),
		warn:        s.Foreground(colorWarn),
		err:         s.Foreground(colorErr),
		label:       s.Foreground(colorFaint).Bold(true),
		accentLabel: s.Foreground(colorAccent).Bold(true),
		errLabel:    s.Foreground(colorErr).Bold(true),
		code:        s.Foreground(colorAccent),
		codeBlock:   s.Foreground(colorMuted),
		quote:       s.Foreground(colorMuted).Italic(true),
		bold:        s.Bold(true).Foreground(colorText),
		link:        s.Foreground(colorAccent).Underline(true),
		key:         s.Background(colorChip).Foreground(colorMuted).Bold(true).Padding(0, 1),
		keyHint:     s.Foreground(colorFaint),
		selected:    s.Background(colorSelect).Foreground(colorText).Bold(true),
		selection:   s.Background(colorSelection).Foreground(colorText),
		toast:       s.Background(colorOK).Foreground(colorInk).Bold(true).Padding(0, 1),
		hover:       s.Background(colorSelect).Foreground(colorText),
		hoverAction: s.Background(colorSelect).Foreground(colorAccent).Bold(true),
		// Only the background, laid under text that keeps its colours.
		hoverBackground: s.Background(colorSelect),
		diffAdd:         s.Foreground(colorText).Background(colorAdded),
		diffDel:         s.Foreground(colorText).Background(colorRemoved),
		diffHunk:        s.Foreground(colorMuted).Background(colorSelect),
		chip:            s.Background(colorChip).Foreground(colorMuted).Bold(true).Padding(0, 1),
		chipAccent:      s.Background(colorAccentSoft).Foreground(colorAccent).Bold(true).Padding(0, 1),
		button:          s.Background(colorAccent).Foreground(colorInk).Bold(true).Padding(0, 1),
		buttonOff:       s.Background(colorChip).Foreground(colorGhost).Bold(true).Padding(0, 1),
	}
	if noColor {
		// Without colours a selection still has to show, and the raised
		// surfaces read from their weight.
		st.selection = s.Reverse(true)
		st.toast = s.Bold(true).Padding(0, 1)
		st.hover = s.Underline(true)
		st.hoverAction = s.Underline(true).Bold(true)
		st.key = s.Bold(true)
		st.chip, st.chipAccent = s.Bold(true).Padding(0, 1), s.Bold(true).Padding(0, 1)
		st.button, st.buttonOff = s.Reverse(true).Bold(true).Padding(0, 1), s.Padding(0, 1)
	}
	chip := func(fg lipgloss.TerminalColor) lipgloss.Style {
		return s.Background(colorChip).Foreground(fg).Bold(true).Padding(0, 1)
	}
	st.state = map[string]lipgloss.Style{
		"idle":     chip(colorFaint),
		"loading":  chip(colorFaint),
		"running":  st.chipAccent,
		"stopping": chip(colorWarn),
		"in use":   chip(colorWarn),
		"done":     chip(colorOK),
		"failed":   chip(colorErr),
		"stopped":  chip(colorText),
	}
	// The classes of tokens as the web cockpit colours them (theme.css).
	st.syntax = map[string]lipgloss.Style{}
	paint := func(style lipgloss.Style, classes ...string) {
		for _, class := range classes {
			st.syntax[class] = style
		}
	}
	paint(s.Foreground(colorSynText), "")
	paint(s.Foreground(colorSynKeyword), "k", "kd", "kn", "ow", "nt")
	paint(s.Foreground(colorSynKeyword).Bold(true), "gh")
	paint(s.Foreground(colorSynType), "kt", "nc")
	paint(s.Foreground(colorSynConst), "kc", "no", "m", "l", "nd", "cp", "ss", "nl")
	paint(s.Foreground(colorSynBuiltin), "nb", "se", "si", "sr")
	paint(s.Foreground(colorSynFunc), "nf", "na")
	paint(s.Foreground(colorSynFunc).Bold(true), "gu")
	paint(s.Foreground(colorSynString), "s")
	paint(s.Foreground(colorSynComment), "gp")
	paint(s.Foreground(colorSynComment).Italic(true), "c", "sd")
	paint(s.Foreground(colorSynComment).Bold(true), "cs")
	paint(s.Foreground(colorSynPunct), "p")
	paint(s.Foreground(colorSynOp), "o", "go")
	paint(s.Foreground(colorSynProp), "py", "nv", "nn")
	paint(s.Foreground(colorErr).Background(colorRemoved), "gd")
	paint(s.Foreground(colorOK).Background(colorAdded), "gi")
	paint(s.Foreground(colorSynText).Italic(true), "ge")
	paint(s.Foreground(colorSynText).Bold(true), "gs")
	paint(s.Foreground(colorErr), "gr", "err")
	return st
}

// token is the style of a token of code of a class.
func (st styles) token(class string) lipgloss.Style {
	if style, ok := st.syntax[class]; ok {
		return style
	}
	return st.syntax[""]
}

// providerColor is the colour the web cockpit gives a model provider's
// dot; the ghost for one it does not know.
func providerColor(provider string) lipgloss.TerminalColor {
	switch provider {
	case "anthropic", "claude":
		return lipgloss.Color("#D9774F")
	case "openai", "codex", "openai-codex":
		return lipgloss.Color("#8F8AFF")
	case "google", "antigravity", "gemini":
		return lipgloss.Color("#3FB6A8")
	case "moonshot", "kimi":
		return lipgloss.Color("#4F95FF")
	case "meta", "cognition", "devin":
		return lipgloss.Color("#6F8CFF")
	case "deepseek":
		return lipgloss.Color("#5B8DEF")
	case "qwen":
		return lipgloss.Color("#A38CF4")
	case "mistral":
		return lipgloss.Color("#FF8A3D")
	case "xai":
		return colorText
	}
	return colorGhost
}
