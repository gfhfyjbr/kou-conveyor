package canvascli

import (
	"fmt"
	"slices"
	"strings"
	"unicode/utf8"
)

// glyph is a node's mark, as the canvas shows it.
func glyph(n viewNode) string {
	switch n.Kind {
	case "agent":
		return "◆"
	case "source":
		return "⚡"
	case "note":
		return "¶"
	}
	switch n.Preset {
	case "claude-code":
		return "✻"
	case "codex":
		return "◎"
	case "opencode":
		return "⌬"
	}
	return "▣"
}

// kindLabel is a node's kind and preset: terminal/codex, agent,
// source/github-events:issues.
func kindLabel(n viewNode) string {
	label := n.Kind
	switch {
	case n.Plugin != "" && n.Plugin != "canvas" && n.Kind == "source":
		label += "/" + n.Plugin + ":" + n.Preset
	case n.Plugin != "" && n.Plugin != "canvas":
		label += "/" + n.Plugin + "/" + n.Preset
	case n.Preset != "":
		label += "/" + n.Preset
	}
	return label
}

func place(n viewNode) string { return fmt.Sprintf("(%d,%d %d×%d)", n.X, n.Y, n.W, n.H) }

// clip shortens text to n characters.
func clip(text string, n int) string {
	text = strings.Join(strings.Fields(text), " ")
	if utf8.RuneCountInString(text) <= n {
		return text
	}
	runes := []rune(text)
	return string(runes[:n-1]) + "…"
}

// pad pads text with spaces to n characters.
func pad(text string, n int) string {
	if count := utf8.RuneCountInString(text); count < n {
		return text + strings.Repeat(" ", n-count)
	}
	return text
}

// formatView is the canvas as kou-canvas view prints it.
func formatView(v view, full bool) string {
	var b strings.Builder
	state := "paused"
	if v.Canvas.Live {
		state = "live"
	}
	fmt.Fprintf(&b, "canvas «%s» (%s)", v.Canvas.Title, state)
	me := ""
	if v.Me != nil {
		me = v.Me.ID
		fmt.Fprintf(&b, " · you: %s «%s» %s", v.Me.ID, v.Me.Title, kindLabel(*v.Me))
	}
	if v.Scope != "" {
		fmt.Fprintf(&b, " · access %s", v.Scope)
	}
	b.WriteString("\n")
	if len(v.Nodes) == 0 {
		b.WriteString("no nodes\n")
	}
	titles := map[string]string{}
	titleWidth, kindWidth, placeWidth := 0, 0, 0
	for _, n := range v.Nodes {
		title := clip(n.Title, 28)
		if n.ID == me {
			title += " (you)"
		}
		titles[n.ID] = title
		titleWidth = max(titleWidth, utf8.RuneCountInString(title))
		kindWidth = max(kindWidth, utf8.RuneCountInString(kindLabel(n)))
		placeWidth = max(placeWidth, utf8.RuneCountInString(place(n)))
	}
	for _, n := range v.Nodes {
		status := n.Status
		if status == "" {
			status = "-"
		}
		fmt.Fprintf(&b, "  %-9s %s %s  %s  %-8s %s", n.ID, glyph(n), pad(titles[n.ID], titleWidth), pad(kindLabel(n), kindWidth), status, pad(place(n), placeWidth))
		var extras []string
		if n.Detail != "" {
			extras = append(extras, clip(n.Detail, 40))
		}
		if len(n.Inputs) != 0 && !slices.Equal(n.Inputs, []string{"in"}) {
			extras = append(extras, "in: "+strings.Join(n.Inputs, ", "))
		}
		if len(n.Outputs) != 0 && !slices.Equal(n.Outputs, []string{"out"}) && !slices.Equal(n.Outputs, []string{"out", "exit"}) {
			extras = append(extras, "out: "+strings.Join(n.Outputs, ", "))
		}
		if n.Branch != "" {
			extras = append(extras, "⎇ "+n.Branch)
		}
		if n.Program != "" {
			extras = append(extras, "runs "+clip(n.Program, 30))
		}
		if n.Pending > 0 {
			extras = append(extras, fmt.Sprintf("%d pending", n.Pending))
		}
		if n.Proposed {
			extras = append(extras, "waits for the user's approval")
		}
		if n.CreatedBy != "" && n.CreatedBy != "user" {
			extras = append(extras, "made by "+n.CreatedBy)
		}
		if len(extras) != 0 {
			b.WriteString("  " + strings.Join(extras, " · "))
		}
		b.WriteString("\n")
		if full && n.Text != "" {
			for line := range strings.SplitSeq(strings.TrimRight(n.Text, "\n"), "\n") {
				b.WriteString("      │ " + line + "\n")
			}
		}
	}
	if len(v.Edges) != 0 {
		b.WriteString("connections:\n")
		for _, e := range v.Edges {
			from, to := e.From, e.To
			if title := v.titleOf(nodeOf(from)); title != "" {
				from += " «" + title + "»"
			}
			if title := v.titleOf(nodeOf(to)); title != "" {
				to += " «" + title + "»"
			}
			fmt.Fprintf(&b, "  %-9s %s → %s", e.ID, from, to)
			if note := strings.TrimPrefix(modeNote(e), ", "); note != "" {
				b.WriteString("  (" + note + ")")
			}
			if full && e.Template != "" {
				b.WriteString("  template: " + clip(e.Template, 80))
			}
			b.WriteString("\n")
		}
	}
	if len(v.Free) != 0 {
		places := make([]string, 0, len(v.Free))
		for _, f := range v.Free {
			places = append(places, fmt.Sprintf("%s at (%d,%d)", sideOfYou(f.Side), f.X, f.Y))
		}
		b.WriteString("free: " + strings.Join(places, " · ") + "\n")
	}
	return b.String()
}

func (v view) titleOf(id string) string {
	for _, n := range v.Nodes {
		if n.ID == id {
			return n.Title
		}
	}
	return ""
}

// sideOfYou says a side of the agent's node: right of you, below you.
func sideOfYou(side string) string {
	if side == "right" || side == "left" {
		return side + " of you"
	}
	return side + " you"
}

// formatSelf is what kou-canvas self prints.
func formatSelf(s self) string {
	var b strings.Builder
	state := "paused"
	if s.Canvas.Live {
		state = "live"
	}
	n := s.Node
	fmt.Fprintf(&b, "you are %s «%s» (%s) on canvas «%s» (%s) · access %s\n", n.ID, n.Title, kindLabel(n), s.Canvas.Title, state, s.Scope)
	fmt.Fprintf(&b, "at %s", place(n))
	if len(n.Inputs) != 0 {
		b.WriteString(" · inputs: " + strings.Join(n.Inputs, ", "))
	}
	if len(n.Outputs) != 0 {
		b.WriteString(" · outputs: " + strings.Join(n.Outputs, ", "))
	}
	b.WriteString("\n")
	if n.Worktree != "" {
		fmt.Fprintf(&b, "worktree: %s on %s\n", n.Worktree, n.Branch)
	}
	if s.Workspace != "" {
		fmt.Fprintf(&b, "workspace: %s\n", s.Workspace)
	}
	if s.Page != "" {
		fmt.Fprintf(&b, "page: %s\n", s.Page)
	}
	if s.Brief != "" {
		b.WriteString("\n" + strings.TrimSpace(s.Brief) + "\n")
	}
	return b.String()
}
