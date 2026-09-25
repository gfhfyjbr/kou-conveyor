package main

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Layout, top to bottom, as the web cockpit's stage without its rail: the
// bar and its rule, a row of air, the transcript, the status line, the dock
// (the queue and the images on a tray, the composer in its box with its
// controls) and the key hints. The transcript takes whatever is left.
const (
	// margin is the air at the left of the transcript and the dock, and at
	// the right of the transcript.
	margin = 2
	// gutter is the columns before an entry's text: the margin, the time or
	// the prompt's number, the rail with the entry's node on it, a space.
	gutter    = margin + 8
	minInputs = 3 // composer rows, when the terminal has room for them
	maxInputs = 12
	// promptWidth is where the composer's text starts: after the margin, the
	// box's edge and a space.
	promptWidth = margin + 2
	// maxMeasure is the widest the cockpit lays itself out, in columns: on a
	// wider terminal it sits in the middle, as the web cockpit's stage keeps
	// its measure. Beside the changes panel, and inline, it takes the whole
	// width.
	maxMeasure = 132
)

// fitWidth chooses the width the cockpit lays itself out in.
func (m *uiModel) fitWidth() {
	m.width = m.termWidth
	if !m.compact && !m.changesShown() && m.termWidth > maxMeasure {
		m.width = maxMeasure
	}
}

// stageWidth is the width of the stage: the transcript's column and the
// dock under it. Beside the changes panel, which runs down the right side
// of the screen, the stage keeps the columns left of it.
func (m *uiModel) stageWidth() int {
	if panel, covers := m.panelColumns(); panel > 0 && !covers {
		return max(1, m.width-panel)
	}
	return m.width
}

// left is the terminal column the cockpit starts at: the middle of a wide
// terminal, where it keeps its measure.
func (m *uiModel) left() int {
	if m.compact {
		return 0
	}
	return max(0, (m.termWidth-m.width)/2)
}

// roomy reports a terminal with rows to spare for air: the row under the
// bar and the composer's bottom edge.
func (m *uiModel) roomy() bool { return m.height >= 24 }

// top is the screen row the transcript starts on: under the bar, its rule
// and, where there is room, a row of air.
func (m *uiModel) top() int {
	if m.roomy() {
		return 3
	}
	return 2
}

// chromeRows are the rows that are neither transcript nor composer text:
// the bar and its rule, the air under them, the status line, the composer's
// top edge, its controls, its bottom edge and the key hints.
func (m *uiModel) chromeRows() int {
	rows := m.top() + 4
	if m.roomy() {
		rows++
	}
	return rows
}

// Screen rows below the transcript.
func (m *uiModel) statusRow() int { return m.top() + m.view.Height }
func (m *uiModel) ruleRow() int   { return m.statusRow() + 1 + m.queueRows() + m.stripRows() }
func (m *uiModel) inputTop() int  { return m.ruleRow() + 1 }

// controlsRow is the row of the composer's controls: the model, the effort
// and the run button, inside the box, or on its bottom edge where the
// terminal is short.
func (m *uiModel) controlsRow() int { return m.inputTop() + m.input.Height() }

func (m *uiModel) layout() {
	if !m.ready {
		return
	}
	m.fitWidth()
	// The composer's text sits in its box: the margin, an edge and a space on
	// each side.
	m.input.SetWidth(max(promptWidth+4, m.stageWidth()-margin-2))
	m.resize()
}

// resize fits the composer to its content and gives the rest to the
// transcript.
func (m *uiModel) resize() {
	if !m.ready {
		return
	}
	textWidth := max(1, m.input.Width())
	rows := 0
	for _, line := range strings.Split(m.input.Value(), "\n") {
		rows += max(1, (ansi.StringWidth(line)+textWidth)/textWidth)
	}
	// The composer grows with its text, from a few rows up to two fifths of
	// the screen, and always leaves the transcript a row.
	lowest := minInputs
	if m.height < 20 || m.compact {
		lowest = 1
	}
	// The queue and the strip of images sit between the status line and
	// the composer.
	chrome := m.chromeRows()
	queue := m.queueRows() + m.stripRows()
	highest := clamp(m.height*2/5, 1, max(1, m.height-chrome-queue-1))
	rows = clamp(rows, min(lowest, highest), min(maxInputs, max(lowest, highest)))
	if rows != m.input.Height() {
		m.input.SetHeight(rows)
	}
	height := max(1, m.height-chrome-queue-m.input.Height())
	// The transcript keeps air and the scrollbar at its right; the changes
	// panel takes the screen's right side, when both fit.
	width := m.stageWidth() - margin - 2
	if height != m.view.Height || m.view.Width != max(1, width) {
		atBottom := m.view.AtBottom()
		m.view.Width, m.view.Height = max(1, width), height
		if atBottom || m.follow {
			m.view.GotoBottom()
		}
	}
}

// refresh re-renders the transcript and follows the newest output when the
// reader is at the bottom; otherwise it counts what arrived below.
func (m *uiModel) refresh() {
	m.render(true)
}

// refreshKeep re-renders without moving the scroll position.
func (m *uiModel) refreshKeep() {
	m.render(false)
}

func (m *uiModel) render(follow bool) {
	if !m.ready {
		return
	}
	width := max(20, m.view.Width)
	var lines []string
	m.spans = m.spans[:0]
	var previous *cockpit.Entry
	index := 0
	// What running an edit replaces fades.
	fading := false
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser {
			index++
		}
		if previous != nil && !(dense(previous) && dense(e)) {
			lines = append(lines, m.timeline(e))
		}
		body := m.block(e, width, index, fading)
		if m.edit != nil && m.edit.id == e.ID {
			fading = true
		}
		m.spans = append(m.spans, entrySpan{id: e.ID, start: len(lines), end: len(lines) + len(body), pictures: m.cache[e.ID].pictures})
		lines = append(lines, body...)
		previous = e
	}
	m.notePictures()
	m.starters = m.starters[:0]
	if len(m.tr.Entries) == 0 {
		lines = m.welcome(width)
	}
	wasBottom := m.view.AtBottom() || m.view.TotalLineCount() == 0
	offset := m.view.YOffset
	m.lines = lines
	m.view.SetContent(strings.Join(lines, "\n"))
	grew := len(m.tr.Entries) - m.shown
	m.shown = len(m.tr.Entries)
	switch {
	case len(m.tr.Entries) == 0:
		m.view.GotoTop() // the welcome screen reads from the top
	case follow && (m.follow || wasBottom) && m.press == nil:
		// Output does not scroll the text away while it is being selected.
		m.view.GotoBottom()
		m.follow, m.unseen = true, 0
	default:
		m.view.SetYOffset(offset)
		if follow && grew > 0 {
			m.unseen += grew
		}
	}
}

// Tool calls and thinking pack tightly; prose gets air around it.
func dense(e *cockpit.Entry) bool {
	return e.Kind == cockpit.KindTool || e.Kind == cockpit.KindReasoning
}

// timeline is the row of air between two entries: the rail runs through
// it, as the web cockpit's line runs down the transcript.
func (m *uiModel) timeline(next *cockpit.Entry) string {
	return strings.Repeat(" ", gutter-2) + m.styles.rule.Render("│")
}

// rail is the line an entry's body hangs from.
func (m *uiModel) rail() string {
	return strings.Repeat(" ", gutter-2) + m.styles.rule.Render("│") + " "
}

func expandable(e *cockpit.Entry) bool {
	switch e.Kind {
	case cockpit.KindTool, cockpit.KindReasoning:
		return true
	case cockpit.KindNotice:
		return e.Detail != ""
	}
	return false
}

func (m *uiModel) isOpen(e *cockpit.Entry) bool {
	if open, ok := m.expanded[e.ID]; ok {
		return open
	}
	if m.expandAll {
		return true
	}
	return e.Kind == cockpit.KindTool && e.Tool.State == cockpit.ToolFailed && e.Tool.Error != ""
}

// block renders one entry, reusing the previous rendering when nothing that
// affects it changed.
func (m *uiModel) block(e *cockpit.Entry, width, index int, faded bool) []string {
	open := expandable(e) && m.isOpen(e)
	live := m.state != idle || m.external
	editing := m.edit != nil && m.edit.id == e.ID
	tall := m.viewedLimit(e, open)
	if cached, ok := m.cache[e.ID]; ok && cached.rev == e.Rev && cached.width == width && cached.open == open &&
		cached.live == live && cached.faded == faded && cached.editing == editing && cached.tall == tall {
		return cached.lines
	}
	lines := m.renderEntry(e, width, index, open, live, editing)
	if faded {
		for i, line := range lines {
			lines[i] = m.styles.ghost.Render(ansi.Strip(line))
		}
	}
	// The picture a call read ends its block (viewed.go), as it is.
	pictures := m.viewedRowsOf(e, width, open)
	lines = append(lines, pictures...)
	m.cache[e.ID] = block{rev: e.Rev, width: width, open: open, live: live, faded: faded, editing: editing, lines: lines,
		tall: tall, pictures: len(pictures)}
	return lines
}

// ---------------------------------------------------------------- cards

// card frames lines from the rail's column to width, as the web cockpit
// frames a prompt, a tool call or an error: a hairline above and below, the
// left edge in the style given. The right stays open: what is copied from
// it stays clean, and a line whose width the terminal measures otherwise
// breaks nothing. title goes into the top edge; joints are rows of the
// body that become edges of their own, with titles, between sections.
func (m *uiModel) card(lines []string, edge lipgloss.Style, title string, joints map[int]string, width int) []string {
	st := m.styles
	pad := strings.Repeat(" ", gutter-2)
	span := max(1, width-(gutter-2))
	top, side, bottom := "┌", "│", "└"
	if edge.GetForeground() != st.rule.GetForeground() && !st.noColor {
		// A coloured edge is a heavier one, as the web cockpit's 2px border.
		top, side, bottom = "┎", "┃", "┖"
	}
	rule := func(lead, title string) string {
		line := edge.Render(lead)
		if title != "" {
			line += st.rule.Render("─ ") + st.label.Render(title) + " "
		}
		return line + st.rule.Render(strings.Repeat("─", max(0, span-ansi.StringWidth(line))))
	}
	out := []string{pad + rule(top, title)}
	for i, line := range lines {
		if joint, ok := joints[i]; ok {
			out = append(out, pad+rule("├", joint))
			continue
		}
		out = append(out, pad+edge.Render(side)+" "+line)
	}
	return append(out, pad+rule(bottom, ""))
}

func (m *uiModel) renderEntry(e *cockpit.Entry, width, index int, open, live, editing bool) []string {
	st := m.styles
	body := width - gutter
	stamp := st.ghost.Render(fmt.Sprintf("%*s", gutter-3, localTime(e.At)))
	rail := m.rail()
	head := func(glyph string) string { return stamp + " " + glyph + " " }

	switch e.Kind {
	case cockpit.KindUser:
		node, edge := st.accent.Render("■"), st.accent
		label := st.accentLabel.Render("YOU") + "  " + st.ghost.Render(clockTime(e.At))
		switch e.State {
		case cockpit.Pending:
			label += st.faint.Render("  · sending")
		case cockpit.Undelivered:
			node, edge = st.err.Render("■"), st.err
			label += st.errLabel.Render("  · NOT DELIVERED")
		}
		if e.Forced {
			// Sent while the agent worked: it read this after its tools.
			label += st.accent.Render("  ⚡") + st.faint.Render(" forced in")
		}
		// The model that answered it; one that differs from the prompt
		// before stands out.
		if e.Model != "" {
			style := st.ghost
			if before := m.previousModel(e.ID); before != "" && before != e.Model {
				style = st.muted
				label += st.accent.Render("  ⇄")
			} else {
				label += st.ghost.Render("  ·")
			}
			label += " " + m.modelDot(e.Model) + style.Render(ansi.Truncate(e.Model, 40, "…"))
		}
		number := st.accentLabel.Render(fmt.Sprintf("%*s", gutter-3, fmt.Sprintf("%02d", index)))
		header := number + " " + node + " " + label
		// Between runs a click on a prompt's header edits it.
		switch {
		case editing:
			header = fitRight(header, st.accentLabel.Render("✎ EDITING"), width)
		case !live && e.State != cockpit.Pending && !m.compact:
			header = fitRight(header, st.faint.Render("✎ edit"), width)
		}
		text := wrapText(e.Text, st.text, body, "", "")
		// The images it brought, which its text names.
		if len(e.Images) != 0 {
			var spans []span
			for i, img := range e.Images {
				if i > 0 {
					spans = append(spans, span{"   ", st.text})
				}
				spans = append(spans, span{"▣ ", st.accent}, span{img.Label, st.muted}, span{" " + describe(img), st.faint})
			}
			text = append(text, wrapSpans(spans, body, "", "")...)
		}
		return append([]string{header}, m.card(text, edge, "", nil, width)...)

	case cockpit.KindAssistant:
		glyph, label := st.text.Render("■"), st.label.Render("AGENT")
		base := st.text
		if e.Phase == "commentary" {
			glyph, label, base = st.ghost.Render("□"), st.label.Render("AGENT · NOTE"), st.muted
		}
		lines := []string{head(glyph) + label + "  " + st.ghost.Render(clockTime(e.At))}
		for _, line := range renderMarkdown(st, e.Text, body, base) {
			lines = append(lines, rail+line)
		}
		return lines

	case cockpit.KindReasoning:
		chevron := "▸"
		if open {
			chevron = "▾"
		}
		gist := strings.ReplaceAll(firstLine(e.Text), "**", "")
		lead := head(st.ghost.Render("◇")) + st.label.Render("THINKING") + "  "
		lines := []string{fitRight(lead+st.faint.Italic(true).Render(gist), st.ghost.Render(chevron), width)}
		// A one-line summary is already shown in full.
		if open && strings.TrimSpace(strings.ReplaceAll(e.Text, "**", "")) != gist {
			for _, line := range renderMarkdown(st, e.Text, body, st.muted) {
				lines = append(lines, rail+line)
			}
		}
		return lines

	case cockpit.KindTool:
		return m.renderTool(e, width, open, live)

	case cockpit.KindNotice:
		text := st.label.Render(strings.ToUpper(e.Text))
		lead := head(st.ghost.Render("─")) + text + " "
		right := st.ghost.Render(clockTime(e.At))
		if e.Detail != "" {
			chevron := "▸"
			if open {
				chevron = "▾"
			}
			right += "  " + st.ghost.Render(chevron)
		}
		fill := max(0, width-ansi.StringWidth(lead)-ansi.StringWidth(right)-1)
		lines := []string{lead + st.rule.Render(strings.Repeat("─", fill)) + " " + right}
		if open && e.Detail != "" {
			for _, l := range renderMarkdown(st, e.Detail, body, st.muted) {
				lines = append(lines, rail+l)
			}
		}
		return lines

	case cockpit.KindError:
		lines := []string{head(st.err.Render("■")) + st.errLabel.Render("ERROR") + "  " + st.ghost.Render(clockTime(e.At))}
		return append(lines, m.card(wrapText(e.Text, st.err, body, "", ""), st.err, "", nil, width)...)
	}
	return wrapText(e.Text, st.text, width, rail, rail)
}

// modelDot is the dot before a model's name, in its provider's colour when
// the connection says which that is.
func (m *uiModel) modelDot(model string) string {
	provider := ""
	if c := m.catalog; c != nil {
		if info, ok := c.Find(model); ok {
			provider = info.Provider
		}
	}
	if provider == "" {
		provider = m.conn.Provider
	}
	if m.styles.noColor {
		return ""
	}
	return lipgloss.NewStyle().Foreground(providerColor(provider)).Render("■") + " "
}

func (m *uiModel) renderTool(e *cockpit.Entry, width int, open, live bool) []string {
	st := m.styles
	tool := e.Tool
	state := tool.State
	if !live && !tool.Terminal() {
		state = "interrupted"
	}
	glyph := map[string]string{
		cockpit.ToolQueued: st.accent.Render("□"), cockpit.ToolRunning: st.accent.Render("■"),
		cockpit.ToolDone: st.ok.Render("✓"), cockpit.ToolFailed: st.err.Render("✗"),
		cockpit.ToolCanceled: st.ghost.Render("⊘"), "interrupted": st.ghost.Render("◌"),
	}[state]
	nonzero := tool.ExitCode != nil && *tool.ExitCode != 0
	if state == cockpit.ToolDone && nonzero {
		glyph = st.warn.Render("✓")
	}

	var meta []string
	switch {
	case state == cockpit.ToolRunning || state == cockpit.ToolQueued:
		meta = append(meta, st.accent.Render(strings.ToLower(state)))
	case !tool.Started.IsZero() && !tool.Finished.IsZero():
		meta = append(meta, st.faint.Render(duration(tool.Finished.Sub(tool.Started))))
	}
	if nonzero {
		meta = append(meta, st.warn.Render(fmt.Sprintf("exit %d", *tool.ExitCode)))
	}
	switch state {
	case cockpit.ToolFailed:
		meta = append(meta, st.errLabel.Render("FAILED"))
	case cockpit.ToolCanceled:
		meta = append(meta, st.label.Render("STOPPED"))
	case "interrupted":
		meta = append(meta, st.label.Render("INTERRUPTED"))
	}
	chevron := "▸"
	if open {
		chevron = "▾"
	}
	right := strings.Join(meta, st.ghost.Render(" · ")) + "  " + st.ghost.Render(chevron)

	name := strings.ToUpper(orDefault(tool.Name, "tool"))
	input := firstLine(tool.Input)
	if strings.EqualFold(tool.Name, "bash") && input != "" {
		input = st.ghost.Render("$ ") + st.text.Render(input)
	} else {
		input = st.text.Render(input)
	}
	stamp := st.ghost.Render(fmt.Sprintf("%*s", gutter-3, localTime(e.At)))
	lead := stamp + " " + glyph + " " + st.label.Render(name) + "  "
	lines := []string{fitRight(lead+input, right, width)}

	body := max(10, width-gutter)
	if !open {
		if state == cockpit.ToolFailed && tool.Error != "" {
			lines = append(lines, strings.Repeat(" ", gutter-2)+st.rule.Render("└")+" "+st.err.Render(ansi.Truncate(firstLine(tool.Error), body, "…")))
		}
		return lines
	}
	// Open, the call is a card: each stream a section with its own edge,
	// as the web cockpit's tool body.
	var text []string
	joints := map[int]string{}
	first := ""
	section := func(title string, style lipgloss.Style, content string) {
		count := strings.Count(strings.TrimRight(content, "\n"), "\n") + 1
		unit := "LINES"
		if count == 1 {
			unit = "LINE"
		}
		heading := fmt.Sprintf("%s · %d %s", strings.ToUpper(title), count, unit)
		if first == "" {
			first = heading
		} else {
			joints[len(text)] = heading
			text = append(text, "")
		}
		for _, line := range clipLines(strings.Split(strings.TrimRight(strings.ReplaceAll(content, "\t", "    "), "\n"), "\n"), 150, 150) {
			if line == "\x00" {
				text = append(text, st.ghost.Render("   ⋯"))
				continue
			}
			for _, part := range strings.Split(ansi.Hardwrap(line, body, true), "\n") {
				text = append(text, style.Render(part))
			}
		}
	}
	if strings.Contains(tool.Input, "\n") || ansi.StringWidth(tool.Input) > body-12 {
		section("input", st.text, tool.Input)
	}
	if tool.Error != "" {
		section("error", st.err, tool.Error)
	}
	if tool.Output != "" {
		section("output", st.muted, tool.Output)
	}
	if tool.Stderr != "" {
		section("stderr", st.warn, tool.Stderr)
	}
	if tool.Output == "" && tool.Stderr == "" && tool.Error == "" {
		waiting := "no output"
		if state == cockpit.ToolRunning || state == cockpit.ToolQueued {
			waiting = "waiting for output…"
		}
		text = append(text, st.ghost.Render(waiting))
	}
	edge := st.rule
	if state == cockpit.ToolFailed {
		edge = st.err
	}
	return append(lines, m.card(text, edge, first, joints, width)...)
}

// clipLines keeps the head and tail of long output around a marker line.
func clipLines(lines []string, head, tail int) []string {
	if len(lines) <= head+tail {
		return lines
	}
	out := append([]string{}, lines[:head]...)
	out = append(out, "\x00")
	return append(out, lines[len(lines)-tail:]...)
}

// ---------------------------------------------------------------- welcome

var starters = []struct{ number, name, gist, prompt string }{
	{"01", "SURVEY", "Map the architecture, entry points and how to build and test.",
		"Map this repository: its architecture, entry points, and how to build and test it."},
	{"02", "VERIFY", "Run the tests and fix the first real failure.",
		"Run the test suite. If anything fails, find the root cause and fix it."},
	{"03", "REVIEW", "Audit uncommitted changes for bugs and risky edits.",
		"Review the uncommitted changes for bugs, races and risky edits. Report findings by severity."},
	{"04", "PROFILE", "Find the slowest step and propose a concrete fix.",
		"Find the slowest part of the build or test run and propose a concrete fix."},
}

// starterBox is where a task shows on the welcome screen: transcript lines
// [top, bottom) and columns [left, right).
type starterBox struct {
	top, bottom, left, right int
	prompt                   string
}

// starterAt is the task drawn under a transcript line and column.
func (m *uiModel) starterAt(line, x int) (starterBox, bool) {
	for _, s := range m.starters {
		if line >= s.top && line < s.bottom && x >= s.left && x < s.right {
			return s, true
		}
	}
	return starterBox{}, false
}

// wordmark is READY in block letters, four rows tall, as the web cockpit's
// heading; the caret after it is the accent.
var wordmark = [4]string{
	"█▀▀▀▄ █▀▀▀▀ ▄▀▀▀▄ █▀▀▀▄ █   █",
	"█▄▄▄▀ █▄▄▄  █▄▄▄█ █   █ ▀▄ ▄▀",
	"█  ▀▄ █     █   █ █   █   █  ",
	"▀   ▀ ▀▀▀▀▀ ▀   ▀ ▀▀▀▀    ▀  ",
}

// welcome is the empty session: a card with the workspace, what the agent
// does and tasks to start with, as the web cockpit shows it.
func (m *uiModel) welcome(width int) []string {
	st := m.styles
	pad := strings.Repeat(" ", margin)
	outer := max(20, width-margin)
	inner := outer - 4
	var body []string
	blank := func() { body = append(body, "") }
	body = append(body, fitRight(st.label.Render("SESSION · NEW"), st.ghost.Render(ansi.Truncate(m.opt.Workspace, max(8, inner-16), "…")), inner))
	blank()
	if inner >= ansi.StringWidth(wordmark[0])+4 {
		for i, row := range wordmark {
			caret := "  "
			if i > 0 {
				caret = st.accent.Render("██")
			}
			body = append(body, st.text.Render(row)+" "+caret)
		}
	} else {
		body = append(body, st.bold.Render("READY")+st.accent.Render("▮"))
	}
	blank()
	body = append(body, wrapText("The agent works in this workspace with a shell. Describe the outcome you want — it plans, runs commands and reports back as it goes.", st.muted, min(inner, 72), "", "")...)
	blank()
	// The tasks, two to a row where they fit; a click puts one in the
	// composer. Their places are noted for the pointer, in the transcript's
	// lines and columns: the card's rows start at line 1, its text at
	// column margin+2.
	columns := 1
	if inner >= 64 {
		columns = 2
	}
	boxWidth := (inner - (columns-1)*2) / columns
	textWidth := boxWidth - 4
	for i := 0; i < len(starters); i += columns {
		row := starters[i:min(i+columns, len(starters))]
		// Each task's text, wrapped; the boxes of a row are as tall as the
		// tallest.
		gists := make([][]string, len(row))
		height := 0
		for c, s := range row {
			gists[c] = wrapText(s.gist, st.faint, textWidth, "", "")
			height = max(height, len(gists[c]))
		}
		var boxes [][]string
		for c, s := range row {
			lines := []string{st.rule.Render("┌─ ") + st.accentLabel.Render(s.number) + " " + st.bold.Render(s.name) + " " +
				st.rule.Render(strings.Repeat("─", max(0, boxWidth-ansi.StringWidth(s.number+s.name)-6))+"┐")}
			for r := range height {
				g := ""
				if r < len(gists[c]) {
					g = gists[c][r]
				}
				lines = append(lines, st.rule.Render("│")+" "+g+strings.Repeat(" ", max(0, textWidth-ansi.StringWidth(g)))+" "+st.rule.Render("│"))
			}
			lines = append(lines, st.rule.Render("└"+strings.Repeat("─", boxWidth-2)+"┘"))
			boxes = append(boxes, lines)
		}
		for r := range height + 2 {
			line := ""
			for c, box := range boxes {
				if c > 0 {
					line += "  "
				}
				line += box[r]
			}
			body = append(body, line)
		}
		for c, s := range row {
			left := margin + 2 + c*(boxWidth+2)
			// The card's top edge is line 0 of the transcript; its body
			// starts at line 1.
			m.starters = append(m.starters, starterBox{top: 1 + len(body) - (height + 2), bottom: 1 + len(body), left: left, right: left + boxWidth, prompt: s.prompt})
		}
		if i+columns < len(starters) {
			blank()
		}
	}
	blank()
	body = append(body, keyHints(st, inner, "click", "use a task", "^K", "commands", "^S", "sessions", "/help", "keys"))

	// The card: a hairline with the ghost's ticks at its corners.
	tick := st.ghost
	lines := []string{pad + tick.Render("┌") + st.rule.Render(strings.Repeat("─", outer-2)) + tick.Render("┐")}
	for _, line := range body {
		lines = append(lines, pad+st.rule.Render("│")+" "+line+strings.Repeat(" ", max(0, inner-ansi.StringWidth(line)))+" "+st.rule.Render("│"))
	}
	lines = append(lines, pad+tick.Render("└")+st.rule.Render(strings.Repeat("─", outer-2))+tick.Render("┘"))
	return lines
}

// ---------------------------------------------------------------- frame

func (m *uiModel) View() string {
	if m.compact {
		return m.compactView()
	}
	if !m.ready {
		return ""
	}
	st := m.styles
	width := m.width
	hover := m.hover()
	if m.form != nil || m.picker != nil {
		m.previewArea = area{}
	}
	lines := []string{fit(m.header(), width)}
	if m.roomy() {
		lines = append(lines, "")
	}
	switch {
	case m.form != nil:
		lines = append(lines, strings.Split(m.formOverlay(), "\n")...)
	case m.picker != nil:
		lines = append(lines, strings.Split(m.overlay(), "\n")...)
	default:
		// The image the cursor is on shows large over the transcript.
		lines = append(lines, m.withPreview(strings.Split(m.transcriptView(hover), "\n"), m.top())...)
	}
	status := m.statusLine()
	if hint := m.hoverHint(hover); hint != "" && m.note.text == "" {
		status = st.ghost.Render("› ") + st.muted.Render(hint)
	}
	// Terminals that do not know how to set the pointer ignore the request.
	pointer := ""
	if m.py >= 0 {
		pointer = ansi.SetPointerShape(m.pointerShape(hover))
	}
	lines = append(lines, fit(strings.Repeat(" ", margin)+status, m.stageWidth())+m.osc52+pointer)
	lines = append(lines, m.dock(hover)...)
	lines = append(lines, fit(m.footer(), m.stageWidth()))
	// The changes panel runs down the right side, from the transcript's top
	// to the bottom of the screen, beside the stage.
	if g, ok := m.panelGeometry(); ok && g.left > 0 && m.form == nil && m.picker == nil {
		side := m.changesView(g, hover)
		for i, part := range side {
			// Screen row r is lines[r-1]: the rule under the bar is not in lines.
			at := g.top + i - 1
			if at < 1 || at >= len(lines) {
				continue
			}
			row := lines[at]
			row = fit(row, g.left) + strings.Repeat(" ", max(0, g.left-ansi.StringWidth(row)))
			lines[at] = row + part
		}
	}
	// On a wide terminal the cockpit sits in the middle; the bar's rule
	// runs across the whole width, as the web cockpit's does.
	pad := strings.Repeat(" ", m.left())
	var b strings.Builder
	for i, line := range lines {
		if i > 0 {
			b.WriteString("\n")
		}
		if i == 1 {
			b.WriteString(st.rule.Render(strings.Repeat("─", m.termWidth)) + "\n")
		}
		b.WriteString(pad + line)
	}
	return b.String()
}

func (m *uiModel) transcriptView(hover hoverTarget) string {
	st := m.styles
	g, panel := m.panelGeometry()
	if panel && g.left == 0 {
		// Too narrow for both: the panel covers the transcript.
		return strings.Join(m.changesView(g, hover), "\n")
	}
	lines := strings.Split(m.view.View(), "\n")
	total, height := m.view.TotalLineCount(), m.view.Height
	thumbStart, thumbEnd := scrollThumb(total, height, m.view.YOffset)
	out := make([]string, height)
	air := strings.Repeat(" ", margin+1)
	for i := range height {
		line := ""
		if i < len(lines) {
			line = lines[i]
		}
		bar := " "
		if total > height {
			bar = st.rule2.Render("│")
			if i >= thumbStart && i < thumbEnd {
				bar = st.faint.Render("┃")
			}
		}
		gap := max(0, m.view.Width-ansi.StringWidth(line))
		line = m.paintHover(hover, m.top()+i, line+strings.Repeat(" ", gap))
		line = m.selected(inTranscript, m.view.YOffset+i, line, 0)
		if hover.kind == hoverScrollbar && total > height {
			bar = st.accent.Render("┃")
		}
		out[i] = line + air + bar
	}
	return strings.Join(out, "\n")
}

// ---------------------------------------------------------------- the dock

// dockInner is the width of a row of the dock's box, between its edges
// and their spaces; edgeInner that of an edge between its corners.
func (m *uiModel) dockInner() int { return max(1, m.stageWidth()-2*margin-4) }
func (m *uiModel) edgeInner() int { return max(1, m.stageWidth()-2*margin-2) }

// dock is what sits under the status line: the queue and the images on a
// tray, and the composer in its box with its controls. The tray and the box
// share their edges, as the web cockpit's do.
func (m *uiModel) dock(hover hoverTarget) []string {
	first := true
	var lines []string
	for _, part := range [][]string{m.queueView(hover), m.stripView(hover)} {
		if len(part) == 0 {
			continue
		}
		// Each part's first row is its edge, with its label.
		part[0] = m.edgeRow(part[0], edgeKind(first), false)
		lines = append(lines, part...)
		first = false
	}
	focused := m.composerFocused()
	lines = append(lines, m.edgeRow(m.composerRule(), edgeKind(first), focused))
	lines = append(lines, strings.Split(m.composerView(), "\n")...)
	controls, _ := m.controls()
	if m.roomy() {
		lines = append(lines, m.paintControls(hover, m.boxRow(controls)))
		lines = append(lines, m.edgeRow("", edgeBottom, focused))
	} else {
		// Short of rows, the controls go onto the bottom edge.
		lines = append(lines, m.paintControls(hover, m.edgeRow(controls, edgeBottom, focused)))
	}
	return lines
}

// edgeKind is the first box's top edge, or the joint under a tray.
func edgeKind(first bool) string {
	if first {
		return edgeTop
	}
	return edgeJoint
}

const (
	edgeTop    = "top"
	edgeJoint  = "joint"
	edgeBottom = "bottom"
)

// composerFocused reports the composer with the keys: the ticks at its
// corners turn accent, as the web cockpit's do on focus.
func (m *uiModel) composerFocused() bool {
	return m.picker == nil && m.form == nil && m.queueFocus < 0 && !(m.changes.focused && m.changesShown())
}

// edgeRow draws a labelled row as a box's edge: the first box's top, the
// joint between a tray and the box under it, or the bottom. The row's text
// has its own rules; the corners go around it, in the accent when the box
// has the keys, as the web cockpit's ticks.
func (m *uiModel) edgeRow(row, kind string, focused bool) string {
	st := m.styles
	left, right := "├", "┤"
	switch kind {
	case edgeTop:
		left, right = "┌", "┐"
	case edgeBottom:
		left, right = "└", "┘"
	}
	corner := st.rule2
	if focused {
		corner = st.accent
	}
	inner := m.edgeInner()
	row = fit(row, inner)
	row += st.rule2.Render(strings.Repeat("─", max(0, inner-ansi.StringWidth(row))))
	return strings.Repeat(" ", margin) + corner.Render(left) + row + corner.Render(right)
}

// boxRow puts a row of the dock between the box's edges.
func (m *uiModel) boxRow(row string) string {
	st := m.styles
	inner := m.dockInner()
	row = fit(row, inner)
	return strings.Repeat(" ", margin) + st.rule2.Render("│") + " " + row + strings.Repeat(" ", max(0, inner-ansi.StringWidth(row))) + " " + st.rule2.Render("│")
}

// composerView is the textarea with the selection shown on it, in the box.
func (m *uiModel) composerView() string {
	st := m.styles
	view := m.input.View()
	rows := strings.Split(view, "\n")
	inner := m.stageWidth() - margin - 2
	for i, row := range rows {
		if m.sel != nil && m.sel.area == inComposer {
			row = m.selected(inComposer, i, row, promptWidth)
		}
		row += strings.Repeat(" ", max(0, inner-ansi.StringWidth(row)))
		rows[i] = fit(row, inner) + " " + st.rule2.Render("│")
	}
	return strings.Join(rows, "\n")
}

func (m *uiModel) overlay() string {
	height := m.view.Height
	box := m.picker.view(m.styles, min(84, m.width-2), max(4, height-1))
	boxLines := strings.Split(box, "\n")
	left := max(0, (m.width-lipgloss.Width(boxLines[0]))/2)
	top := 0
	if height-len(boxLines) >= 2 {
		top = 1
	}
	// Only rows drawn inside the transcript area are clickable.
	m.picker.listTop = m.top() + top + 4
	if m.picker.fixed {
		m.picker.listTop--
	}
	m.picker.listEnd = m.top() + min(height, top+len(boxLines)-3)
	out := make([]string, height)
	for i := range out {
		if j := i - top; j >= 0 && j < len(boxLines) {
			out[i] = strings.Repeat(" ", left) + boxLines[j]
		}
	}
	return strings.Join(out, "\n")
}

// ---------------------------------------------------------------- the bar

func (m *uiModel) header() string {
	st := m.styles
	brand := st.accent.Render("■") + " " + st.bold.Render("KOU") + st.accent.Render("-") + st.bold.Render("CONVEYOR")
	where := st.faint.Render(filepath.Base(m.opt.Workspace)) + st.ghost.Render(" / ")
	title := orDefault(m.sessionTitle(), "New session")
	if m.loading {
		title = "loading…"
	}
	id := ""
	switch {
	case m.loading:
	case m.fresh:
		id = "unsaved"
	default:
		id = shortID(m.sessionID)
	}
	if m.meta.Pinned && !m.loading {
		where += st.accent.Render("◆ ")
	}

	phase := "idle"
	switch {
	case m.loading:
		phase = "loading"
	case m.state == running:
		phase = "running"
	case m.state == stopping:
		phase = "stopping"
	case m.external:
		phase = "in use"
	case m.last.kind != "" && time.Since(m.last.at) < 5*time.Second:
		phase = m.last.kind
	}
	// The run's state, a chip with its dot: the dot blinks while it runs.
	dot := "■"
	if (phase == "running" || phase == "stopping") && m.frame/4%2 == 1 {
		dot = "□"
	}
	chip := dot + " " + strings.ToUpper(phase)
	if m.state != idle {
		chip += " " + clock(time.Since(m.started))
	}
	right := st.state[phase].Render(chip)
	if u := m.tr.Usage; u.Turns > 0 && m.width >= 90 {
		right = st.ghost.Render("ctx ") + st.faint.Render(tokens(u.Context)) + "  " +
			st.ghost.Render("↑") + st.faint.Render(tokens(u.Input)) + " " +
			st.ghost.Render("↓") + st.faint.Render(tokens(u.Output)) + "   " + right
	}
	room := m.width - margin - ansi.StringWidth(brand) - ansi.StringWidth(right) - 6 - ansi.StringWidth(where) - ansi.StringWidth(id) - 2
	left := strings.Repeat(" ", margin) + brand + "  "
	if room >= 8 {
		left += where + st.text.Render(ansi.Truncate(title, room, "…"))
		if id != "" {
			left += "  " + st.ghost.Render(id)
		}
	}
	return fitRight(left, right+" ", m.width)
}

// meter is the web cockpit's activity meter: a row of segments with the
// accent running along them.
func (m *uiModel) meter() string {
	st := m.styles
	if st.noColor {
		return spinner[m.frame%len(spinner)]
	}
	var b strings.Builder
	at := m.frame % 8
	for i := range 6 {
		switch {
		case i == at || i == at-1:
			b.WriteString(st.accent.Render("▪"))
		default:
			b.WriteString(st.rule2.Render("▪"))
		}
	}
	return b.String()
}

var spinner = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

func (m *uiModel) statusLine() string {
	st := m.styles
	width := m.stageWidth() - margin
	if value := m.input.Value(); strings.HasPrefix(value, "/") && !strings.Contains(value, " ") && m.picker == nil {
		var parts []string
		for _, c := range commands {
			if strings.HasPrefix(c.name, value) {
				parts = append(parts, st.accent.Render(c.name)+st.faint.Render(strings.TrimSpace(" "+c.args)))
			}
		}
		if len(parts) == 0 {
			return st.warn.Render("no such command") + st.faint.Render(" — /help lists them")
		}
		if len(parts) == 1 {
			for _, c := range commands {
				if strings.HasPrefix(c.name, value) {
					parts[0] += st.faint.Render("  " + c.help + " · tab completes")
				}
			}
		}
		return strings.Join(parts, "  ")
	}
	if m.note.text != "" {
		glyph, style := "›", st.text
		switch m.note.level {
		case "warn":
			glyph, style = "!", st.warn
		case "error":
			glyph, style = "✗", st.err
		}
		return style.Render(glyph + " " + m.note.text)
	}
	if m.edit != nil {
		lead := st.accent.Render("✎ ") + st.text.Render(fmt.Sprintf("editing prompt %02d", m.promptNumber(m.edit.id)))
		replacing := ""
		if n := m.replaced(); n > 0 {
			unit := "entries"
			if n == 1 {
				unit = "entry"
			}
			replacing = fmt.Sprintf(", replacing the %d %s below", n, unit)
		}
		// The least of it goes first when the line is short.
		for _, detail := range []string{
			" — enter runs it from here" + replacing + "; files keep their changes",
			" — enter runs it from here" + replacing,
			" — enter runs it from here",
		} {
			if line := lead + st.faint.Render(detail); ansi.StringWidth(line) <= width {
				return line
			}
		}
		return lead
	}
	if m.state != idle {
		activity := orDefault(m.tr.Activity, "Working")
		if m.state == stopping {
			activity = "Stopping"
		}
		left := m.meter() + " " + st.text.Render(activity)
		if running := m.tr.Running(); len(running) > 0 && running[0].Tool.Input != "" {
			left += st.ghost.Render(" · ") + st.muted.Render(firstLine(running[0].Tool.Input))
		}
		if m.diag != "" {
			left += st.ghost.Render(" · " + m.diag)
		}
		right := st.ghost.Render(clock(time.Since(m.started))) + "  " + keyHints(st, 40, "esc esc", "stop") + " "
		return fitRight(left, right, width)
	}
	if m.unseen > 0 && !m.compact {
		return st.accent.Render(fmt.Sprintf("↓ %d new below", m.unseen)) + st.faint.Render(" · pgdn or end")
	}
	if m.loading {
		return st.faint.Render("loading session…")
	}
	if m.external {
		return st.warn.Render("■ ") + st.muted.Render("running in another window or terminal — this view follows it")
	}
	parts := []string{"session " + shortID(m.sessionID)}
	if m.fresh {
		parts[0] += " (new)"
	}
	if n := len(m.tr.Entries); n > 0 {
		parts = append(parts, fmt.Sprintf("%d entries", n))
	}
	// Compact mode has no header for the token counts.
	if u := m.tr.Usage; m.compact && u.Turns > 0 {
		parts = append(parts, fmt.Sprintf("ctx %s ↑%s ↓%s", tokens(u.Context), tokens(u.Input), tokens(u.Output)))
	}
	parts = append(parts, m.runSummary())
	if m.diag != "" && m.last.kind == "failed" {
		parts = append(parts, m.diag)
	}
	line := st.ghost.Render(strings.Join(parts, " · "))
	if m.tr.Interrupted() {
		line = st.warn.Render("◌ interrupted") + st.faint.Render(" — /continue picks it up, esc esc edits the prompt · ") + line
	}
	return line
}

// composerRule is the label of the composer's top edge: what the composer
// does now, and what enter does with its text.
func (m *uiModel) composerRule() string {
	st := m.styles
	label, style := "PROMPT", st.label
	hint := ""
	switch {
	case m.queueEdit != nil:
		label, style = fmt.Sprintf("QUEUED %d", m.queueEdit.index+1), st.accentLabel
		hint = "enter puts it back · ^X forces it · esc keeps it as it was"
	case m.edit != nil:
		label, style = fmt.Sprintf("EDIT %02d", m.promptNumber(m.edit.id)), st.accentLabel
	case m.historyPos >= 0:
		label = fmt.Sprintf("HISTORY %d/%d", m.historyPos+1, len(m.history))
	case m.state != idle && m.queueFocus < 0:
		// While the agent works, what is written waits for it.
		hint = "enter queues · ^X forces in"
	}
	left := st.rule2.Render("─ ") + style.Render(label) + " "
	if hint != "" {
		left += st.ghost.Render(hint) + " "
	}
	if lines := m.input.LineCount(); lines > 1 {
		left += st.ghost.Render(fmt.Sprintf("line %d/%d ", m.input.Line()+1, lines))
	}
	return left
}

// control is a part of the controls row the pointer can act on, at
// columns [from, to) of the screen.
type control struct {
	kind     hoverKind
	from, to int
	// meter is the column of the effort meter's first bar.
	meter int
}

// controls renders the composer's controls: the model and the effort at
// the left, the run button at the right, and says where each is. Inside
// the box they sit on a row of their own; on a short terminal they go onto
// its bottom edge, among its rules.
func (m *uiModel) controls() (string, []control) {
	st := m.styles
	onEdge := !m.roomy()
	x := promptWidth
	var hits []control
	row := ""
	put := func(text string) {
		row += text
		x += ansi.StringWidth(text)
	}
	add := func(kind hoverKind, text string, meter int) {
		hits = append(hits, control{kind: kind, from: x, to: x + ansi.StringWidth(text), meter: x + meter})
		put(text)
	}
	gap, inner := "   ", m.dockInner()
	if onEdge {
		x = margin + 1 // after the corner
		put(st.rule2.Render("─ "))
		gap, inner = " "+st.rule2.Render("─")+" ", m.edgeInner()
	}
	if model := m.modelControl(); model != "" {
		add(hoverModel, model, 0)
		put(gap)
	}
	effort, meter := m.effortControl()
	add(hoverEffort, effort, meter)
	// The button, at the right.
	button := m.runButton()
	tail := ""
	if onEdge {
		tail = st.rule2.Render("─")
		button += " "
	}
	fill := max(2, inner-ansi.StringWidth(row)-ansi.StringWidth(button)-ansi.StringWidth(tail))
	if onEdge {
		put(" " + st.rule2.Render(strings.Repeat("─", fill-2)) + " ")
	} else {
		put(strings.Repeat(" ", fill))
	}
	add(hoverRun, button, 0)
	put(tail)
	return row, hits
}

// runButton is what enter does, as a button: run, queue or stop, and
// beside queue, force in.
func (m *uiModel) runButton() string {
	st := m.styles
	typed := strings.TrimSpace(m.input.Value()) != ""
	key := func(k string) string { return st.ghost.Render(" " + k) }
	switch {
	case m.queueEdit != nil:
		return st.button.Render("PUT IT BACK ↵")
	case m.edit != nil:
		return st.button.Render("RUN FROM HERE ↵")
	case m.state == stopping:
		return st.buttonOff.Render("STOPPING…")
	case m.state != idle && typed:
		return st.chipAccent.Render("⚡ FORCE ^X") + " " + st.button.Render("QUEUE ↵")
	case m.state != idle:
		return st.chip.Foreground(colorErr).Render("■ STOP") + key("esc esc")
	case typed && m.runBlocked() == "":
		return st.button.Render("RUN ↵")
	}
	return st.buttonOff.Render("RUN ↵")
}

// clickRun does what the run button says.
func (m *uiModel) clickRun(x int) tea.Cmd {
	switch {
	case m.state == stopping:
		return nil
	case m.state != idle && strings.TrimSpace(m.input.Value()) != "":
		// The force chip comes before the queue button.
		for _, c := range m.controlAt(x) {
			if c.kind == hoverRun && x < c.from+ansi.StringWidth(m.styles.chipAccent.Render("⚡ FORCE ^X")) {
				return m.forceComposer()
			}
		}
		return m.submit()
	case m.state != idle:
		return m.stop()
	case strings.TrimSpace(m.input.Value()) == "":
		return nil
	}
	return m.submit()
}

// controlAt is the controls under a column of the controls row.
func (m *uiModel) controlAt(x int) []control {
	_, hits := m.controls()
	var at []control
	for _, c := range hits {
		if x >= c.from && x < c.to {
			at = append(at, c)
		}
	}
	return at
}

// effortAt is the screen row and column of the effort meter's first bar.
func (m *uiModel) effortAt() (row, col int) {
	_, hits := m.controls()
	for _, c := range hits {
		if c.kind == hoverEffort {
			return m.controlsRow(), c.meter
		}
	}
	return m.controlsRow(), 0
}

func (m *uiModel) footer() string {
	st := m.styles
	if m.toast != "" {
		toast := st.toast.Render("✓ " + m.toast)
		return strings.Repeat(" ", max(0, (m.stageWidth()-ansi.StringWidth(toast))/2)) + toast
	}
	// In compact mode the terminal selects and copies, and ^F leads back.
	copyHint, layout := []string{"drag", "copy"}, []string{"^F", "inline"}
	if m.compact {
		copyHint, layout = nil, []string{"^F", "fullscreen"}
	}
	var hints []string
	changes := []string{"^G", "changes"}
	width := m.stageWidth() - margin
	pad := strings.Repeat(" ", margin)
	switch {
	case m.changes.focused && m.changesShown():
		hints = []string{"↑↓", "file", "←→", "fold", "pgup pgdn", "diff", "[ ]", "prompt"}
		if m.changesLive() && !m.changes.follow {
			hints = append(hints, "f", "follow the run")
		}
		hints = append(hints, "tab", "back to the prompt", "^G", "close")
		if _, covers := m.panelColumns(); !covers {
			hints = append(hints, "< >", "width") // the first to go where there is no room
		}
		return pad + keyHints(st, width, hints...)
	case m.queueFocus >= 0:
		return pad + keyHints(st, width, "↑↓", "select", "enter", "edit", "^X", "force in", "⌫", "drop", "⇧↑↓", "move", "esc", "back to the prompt")
	case m.queueEdit != nil:
		hints = []string{"enter", "put it back", "^X", "force it in", "esc", "keep as it was", "^J", "new line"}
	case m.edit != nil:
		hints = []string{"enter", "run from here", "esc", "keep as it was", "^J", "new line", "^T", "effort"}
	case m.state != idle:
		lead := []string{"enter", "queue", "^X", "force in"}
		if strings.TrimSpace(m.input.Value()) == "" {
			lead = nil
			if m.queued() != 0 {
				lead = []string{"↑", "queue"}
			}
		}
		hints = append(append(append(lead, "esc esc", "stop", "^K", "commands"), changes...), layout...)
		hints = append(append(append(hints, "^O", "details", "^T", "effort"), copyHint...), "^Y", "copy answer", "^C", "stop")
	default:
		hints = []string{"enter", "run", "^J", "new line", "^V", "image"}
		if q := m.queues[m.sessionID]; q != nil && len(q.Items) != 0 && strings.TrimSpace(m.input.Value()) == "" {
			hints = []string{"enter", "send the next queued", "↑", "queue"}
		}
		if m.lastPrompt() != nil && !m.external {
			hints = append(hints, "esc esc", "edit")
		}
		hints = append(append(append(append(hints, "^T", "effort", "^K", "commands"), changes...), layout...), "^S", "sessions", "^R", "history", "^O", "details")
		hints = append(append(hints, copyHint...), "^C", "quit")
	}
	return pad + keyHints(st, width, hints...)
}

// ---------------------------------------------------------------- text helpers

// fitRight puts right at the end of a line of the given width, shortening
// left when both do not fit.
func fitRight(left, right string, width int) string {
	space := width - ansi.StringWidth(right)
	if ansi.StringWidth(left)+1 > space {
		left = ansi.Truncate(left, max(0, space-1), "…")
	}
	return left + strings.Repeat(" ", max(1, space-ansi.StringWidth(left))) + right
}

func fit(line string, width int) string {
	if ansi.StringWidth(line) > width {
		return ansi.Truncate(line, width, "…")
	}
	return line
}

func firstLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return strings.TrimSpace(s[:i]) + " …"
	}
	return s
}

func localTime(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.Local().Format("15:04")
}

func clockTime(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.Local().Format("15:04:05")
}
