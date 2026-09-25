package main

import "github.com/charmbracelet/lipgloss"

// One accent on a graphite/paper base, matching the web cockpit. Everything
// else is weight, dimness and rules; colour carries state only.
var (
	colorAccent = lipgloss.Color("#FF5F1F")
	colorText   = lipgloss.AdaptiveColor{Light: "#1A1A18", Dark: "#E8E6E0"}
	colorMuted  = lipgloss.AdaptiveColor{Light: "#5F5E58", Dark: "#A3A199"}
	colorFaint  = lipgloss.AdaptiveColor{Light: "#9C9A92", Dark: "#63615A"}
	colorRule   = lipgloss.AdaptiveColor{Light: "#CFCDC5", Dark: "#34332F"}
	colorOK     = lipgloss.AdaptiveColor{Light: "#2E7A34", Dark: "#9CCF83"}
	colorWarn   = lipgloss.AdaptiveColor{Light: "#9A6200", Dark: "#EFB443"}
	colorErr    = lipgloss.AdaptiveColor{Light: "#C62839", Dark: "#FF5A6E"}
	colorInk    = lipgloss.AdaptiveColor{Light: "#FFFFFF", Dark: "#0C0C0B"}
	colorSelect = lipgloss.AdaptiveColor{Light: "#E6E4DC", Dark: "#262522"}
	// Selected text: the accent over the base, like the web cockpit's.
	colorSelection = lipgloss.AdaptiveColor{Light: "#F7CEBB", Dark: "#612912"}
	// Lines a diff adds and removes, tinted like the web cockpit's.
	colorAdded   = lipgloss.AdaptiveColor{Light: "#E3F1E0", Dark: "#18241A"}
	colorRemoved = lipgloss.AdaptiveColor{Light: "#FAE3E5", Dark: "#2C181B"}
)

// With NO_COLOR the renderer drops every attribute, so state must also be
// legible from glyphs alone; noColor lets views pick glyphs that are.
type styles struct {
	noColor bool

	text, muted, faint, rule, accent, ok, warn, err lipgloss.Style
	label, accentLabel, errLabel                    lipgloss.Style
	code, codeBlock, quote, bold, link              lipgloss.Style
	key, keyHint, selected, selection, toast        lipgloss.Style
	hover, hoverAction, hoverBackground             lipgloss.Style
	diffAdd, diffDel, diffHunk                      lipgloss.Style
	badge                                           map[string]lipgloss.Style
	box                                             lipgloss.Style
}

func newStyles(noColor bool) styles {
	s := lipgloss.NewStyle()
	st := styles{
		noColor:     noColor,
		text:        s.Foreground(colorText),
		muted:       s.Foreground(colorMuted),
		faint:       s.Foreground(colorFaint),
		rule:        s.Foreground(colorRule),
		accent:      s.Foreground(colorAccent),
		ok:          s.Foreground(colorOK),
		warn:        s.Foreground(colorWarn),
		err:         s.Foreground(colorErr),
		label:       s.Foreground(colorMuted).Bold(true),
		accentLabel: s.Foreground(colorAccent).Bold(true),
		errLabel:    s.Foreground(colorErr).Bold(true),
		code:        s.Foreground(colorAccent),
		codeBlock:   s.Foreground(colorMuted),
		quote:       s.Foreground(colorMuted).Italic(true),
		bold:        s.Bold(true).Foreground(colorText),
		link:        s.Foreground(colorAccent).Underline(true),
		key:         s.Foreground(colorText).Bold(true),
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
		box:             s.Border(lipgloss.NormalBorder()).BorderForeground(colorAccent).Padding(0, 1),
	}
	if noColor {
		// Without colours a selection still has to show.
		st.selection = s.Reverse(true)
		st.toast = s.Bold(true).Padding(0, 1)
		st.hover = s.Underline(true)
		st.hoverAction = s.Underline(true).Bold(true)
	}
	badge := func(bg lipgloss.TerminalColor) lipgloss.Style {
		return s.Background(bg).Foreground(colorInk).Bold(true).Padding(0, 1)
	}
	st.badge = map[string]lipgloss.Style{
		"idle":     s.Foreground(colorMuted).Bold(true).Padding(0, 1),
		"loading":  s.Foreground(colorMuted).Bold(true).Padding(0, 1),
		"running":  badge(colorAccent),
		"stopping": badge(colorWarn),
		"in use":   s.Foreground(colorWarn).Bold(true).Padding(0, 1),
		"done":     badge(colorOK),
		"failed":   badge(colorErr),
		"stopped":  s.Foreground(colorText).Bold(true).Padding(0, 1),
	}
	return st
}
