package main

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Layout, top to bottom: header, rule, transcript, status line, composer
// rule, composer, key hints. The transcript takes whatever is left.
const (
	gutter        = 8 // "HH:MM ● "
	chrome        = 5 // rows that are not transcript or composer
	minInputs     = 3 // composer rows, when the terminal has room for them
	maxInputs     = 12
	transcriptTop = 2 // screen row where the transcript starts
)

func (m *uiModel) layout() {
	if !m.ready {
		return
	}
	m.input.SetWidth(max(8, m.width))
	m.resize()
}

// resize fits the composer to its content and gives the rest to the
// transcript.
func (m *uiModel) resize() {
	if !m.ready {
		return
	}
	textWidth := max(1, m.width-3)
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
	queue := m.queueRows() + m.stripRows()
	highest := clamp(m.height*2/5, 1, max(1, m.height-chrome-queue-1))
	rows = clamp(rows, min(lowest, highest), min(maxInputs, max(lowest, highest)))
	if rows != m.input.Height() {
		m.input.SetHeight(rows)
	}
	height := max(1, m.height-chrome-queue-m.input.Height())
	// The changes panel takes the transcript's right side, when both fit.
	width := m.width - 2
	if panel, covers := m.panelColumns(); panel > 0 && !covers {
		width -= panel
	}
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
	clear(m.starters)
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

func (m *uiModel) timeline(next *cockpit.Entry) string {
	if next.Kind == cockpit.KindUser {
		return ""
	}
	return strings.Repeat(" ", gutter-2) + m.styles.rule.Render("│")
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
			lines[i] = m.styles.faint.Render(ansi.Strip(line))
		}
	}
	// The picture a call read ends its block (viewed.go), as it is.
	pictures := m.viewedRowsOf(e, width, open)
	lines = append(lines, pictures...)
	m.cache[e.ID] = block{rev: e.Rev, width: width, open: open, live: live, faded: faded, editing: editing, lines: lines,
		tall: tall, pictures: len(pictures)}
	return lines
}

func (m *uiModel) renderEntry(e *cockpit.Entry, width, index int, open, live, editing bool) []string {
	st := m.styles
	body := width - gutter
	stamp := st.faint.Render(fmt.Sprintf("%5s", localTime(e.At)))
	rail := strings.Repeat(" ", gutter-2) + st.rule.Render("│") + " "
	head := func(glyph string) string { return stamp + " " + glyph + " " }

	switch e.Kind {
	case cockpit.KindUser:
		bar := st.accent.Render("▌")
		label := st.accentLabel.Render("YOU") + "  " + st.faint.Render(clockTime(e.At))
		switch e.State {
		case cockpit.Pending:
			label += st.faint.Render("  · sending")
		case cockpit.Undelivered:
			bar = st.err.Render("▌")
			label += st.errLabel.Render("  · NOT DELIVERED")
		}
		if e.Forced {
			// Sent while the agent worked: it read this after its tools.
			label += st.accent.Render("  ⚡") + st.faint.Render(" forced in")
		}
		// The model that answered it; one that differs from the prompt
		// before stands out.
		if e.Model != "" {
			style := st.faint
			if before := m.previousModel(e.ID); before != "" && before != e.Model {
				style = st.muted
				label += st.accent.Render("  ⇄")
			} else {
				label += st.faint.Render("  ·")
			}
			label += " " + style.Render(ansi.Truncate(e.Model, 40, "…"))
		}
		number := st.accentLabel.Render(fmt.Sprintf("%5s", fmt.Sprintf("%02d", index)))
		header := number + " " + bar + " " + label
		// Between runs a click on a prompt's header edits it.
		switch {
		case editing:
			header = fitRight(header, st.accentLabel.Render("✎ EDITING"), width)
		case !live && e.State != cockpit.Pending && !m.compact:
			header = fitRight(header, st.faint.Render("✎ edit"), width)
		}
		lines := []string{header}
		prefix := strings.Repeat(" ", gutter-2) + bar + " "
		lines = append(lines, wrapText(e.Text, st.text, width, prefix, prefix)...)
		// The images it brought, which its text names.
		if len(e.Images) != 0 {
			var spans []span
			for i, img := range e.Images {
				if i > 0 {
					spans = append(spans, span{"   ", st.text})
				}
				spans = append(spans, span{"▣ ", st.accent}, span{img.Label, st.muted}, span{" " + describe(img), st.faint})
			}
			lines = append(lines, wrapSpans(spans, width, prefix, prefix)...)
		}
		return lines

	case cockpit.KindAssistant:
		glyph, label := st.text.Render("●"), st.label.Render("AGENT")
		base := st.text
		if e.Phase == "commentary" {
			glyph, label, base = st.faint.Render("○"), st.label.Render("AGENT · NOTE"), st.muted
		}
		lines := []string{head(glyph) + label}
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
		lead := head(st.faint.Render("◇")) + st.label.Render("thinking") + "  "
		lines := []string{fitRight(lead+st.muted.Italic(true).Render(gist), st.faint.Render(chevron), width)}
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
		lead := head(st.faint.Render("─")) + text + " "
		fill := max(0, width-ansi.StringWidth(lead)-2)
		line := lead + st.rule.Render(strings.Repeat("─", fill))
		if e.Detail != "" {
			chevron := "▸"
			if open {
				chevron = "▾"
			}
			line = lead + st.rule.Render(strings.Repeat("─", max(0, fill-2))) + " " + st.faint.Render(chevron)
		}
		lines := []string{line}
		if open && e.Detail != "" {
			for _, l := range renderMarkdown(st, e.Detail, body, st.muted) {
				lines = append(lines, rail+l)
			}
		}
		return lines

	case cockpit.KindError:
		lines := []string{head(st.err.Render("✗")) + st.errLabel.Render("ERROR")}
		return append(lines, wrapText(e.Text, st.err, width, rail, rail)...)
	}
	return wrapText(e.Text, st.text, width, rail, rail)
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
		cockpit.ToolCanceled: st.faint.Render("⊘"), "interrupted": st.faint.Render("◌"),
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
		meta = append(meta, st.err.Render("failed"))
	case cockpit.ToolCanceled:
		meta = append(meta, st.faint.Render("stopped"))
	case "interrupted":
		meta = append(meta, st.faint.Render("interrupted"))
	}
	chevron := "▸"
	if open {
		chevron = "▾"
	}
	right := strings.Join(meta, st.faint.Render(" · ")) + "  " + st.faint.Render(chevron)

	name := strings.ToUpper(orDefault(tool.Name, "tool"))
	input := firstLine(tool.Input)
	if strings.EqualFold(tool.Name, "bash") && input != "" {
		input = st.faint.Render("$ ") + st.text.Render(input)
	} else {
		input = st.text.Render(input)
	}
	stamp := st.faint.Render(fmt.Sprintf("%5s", localTime(e.At)))
	lead := stamp + " " + glyph + " " + st.label.Render(name) + "  "
	lines := []string{fitRight(lead+input, right, width)}

	rail := strings.Repeat(" ", gutter-2) + st.rule.Render("│") + " "
	body := max(10, width-gutter)
	if !open {
		if state == cockpit.ToolFailed && tool.Error != "" {
			lines = append(lines, strings.Repeat(" ", gutter-2)+st.rule.Render("└")+" "+st.err.Render(ansi.Truncate(firstLine(tool.Error), body, "…")))
		}
		return lines
	}
	section := func(title string, style lipgloss.Style, text string) {
		count := strings.Count(strings.TrimRight(text, "\n"), "\n") + 1
		unit := "lines"
		if count == 1 {
			unit = "line"
		}
		lines = append(lines, rail+st.faint.Render(fmt.Sprintf("── %s · %d %s", title, count, unit)))
		for _, line := range clipLines(strings.Split(strings.TrimRight(strings.ReplaceAll(text, "\t", "    "), "\n"), "\n"), 150, 150) {
			if line == "\x00" {
				lines = append(lines, rail+st.faint.Render("   ⋯"))
				continue
			}
			for _, part := range strings.Split(ansi.Hardwrap(line, body, true), "\n") {
				lines = append(lines, rail+style.Render(part))
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
		lines = append(lines, rail+st.faint.Render(waiting))
	}
	return lines
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

var starters = []struct{ number, name, gist, prompt string }{
	{"01", "SURVEY", "map the architecture, entry points and how to build and test",
		"Map this repository: its architecture, entry points, and how to build and test it."},
	{"02", "VERIFY", "run the tests and fix the first real failure",
		"Run the test suite. If anything fails, find the root cause and fix it."},
	{"03", "REVIEW", "audit uncommitted changes for bugs and risky edits",
		"Review the uncommitted changes for bugs, races and risky edits. Report findings by severity."},
}

func (m *uiModel) welcome(width int) []string {
	st := m.styles
	pad := strings.Repeat(" ", gutter-2)
	lines := []string{
		"",
		pad + st.accent.Render("■ ") + st.bold.Render("READY"),
		"",
	}
	lines = append(lines, wrapText("The agent works in this workspace with a shell. Describe the outcome you want — it plans, runs commands and reports back as it goes.", st.muted, width, pad, pad)...)
	lines = append(lines, pad+st.faint.Render(ansi.Truncate(m.opt.Workspace, max(10, width-gutter), "…")), "")
	if m.starters == nil {
		m.starters = make(map[int]string)
	}
	for _, s := range starters {
		m.starters[len(lines)] = s.prompt
		lines = append(lines, pad+st.accentLabel.Render(s.number)+"  "+st.label.Render(s.name)+"  "+
			st.faint.Render(ansi.Truncate(s.gist, max(10, width-gutter-14), "…")))
	}
	lines = append(lines, "", pad+keyHints(st, width-gutter, "click", "use a task", "ctrl+k", "commands", "ctrl+s", "sessions", "/help", "keys"))
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
	rule := st.rule.Render(strings.Repeat("─", width))
	var b strings.Builder
	hover := m.hover()
	if m.form != nil || m.picker != nil {
		m.previewArea = area{}
	}
	b.WriteString(fit(m.header(), width) + "\n" + rule + "\n")
	switch {
	case m.form != nil:
		b.WriteString(m.formOverlay())
	case m.picker != nil:
		b.WriteString(m.overlay())
	default:
		// The image the cursor is on shows large over the transcript.
		b.WriteString(strings.Join(m.withPreview(strings.Split(m.transcriptView(hover), "\n"), transcriptTop), "\n"))
	}
	status := m.statusLine()
	if hint := m.hoverHint(hover); hint != "" && m.note.text == "" {
		status = st.faint.Render("› ") + st.muted.Render(hint)
	}
	// Terminals that do not know how to set the pointer ignore the request.
	pointer := ""
	if m.px >= 0 {
		pointer = ansi.SetPointerShape(m.pointerShape(hover))
	}
	b.WriteString("\n" + fit(status, width) + m.osc52 + pointer)
	for _, line := range m.queueView(hover) {
		b.WriteString("\n" + line)
	}
	for _, line := range m.stripView(hover) {
		b.WriteString("\n" + line)
	}
	b.WriteString("\n" + m.paintEffort(hover, fit(m.composerRule(), width)))
	b.WriteString("\n" + m.composerView())
	b.WriteString("\n" + fit(m.footer(), width))
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
	for i := range height {
		line := ""
		if i < len(lines) {
			line = lines[i]
		}
		bar := " "
		if total > height {
			bar = st.rule.Render("│")
			if i >= thumbStart && i < thumbEnd {
				bar = st.muted.Render("┃")
			}
		}
		gap := max(0, m.view.Width-ansi.StringWidth(line))
		line = m.paintHover(hover, transcriptTop+i, line+strings.Repeat(" ", gap))
		line = m.selected(inTranscript, m.view.YOffset+i, line, 0)
		if hover.kind == hoverScrollbar && total > height {
			bar = st.accent.Render("┃")
		}
		out[i] = line + " " + bar
	}
	if panel {
		for i, side := range m.changesView(g, hover) {
			out[i] += side
		}
	}
	return strings.Join(out, "\n")
}

// composerView is the textarea with the selection shown on it.
func (m *uiModel) composerView() string {
	view := m.input.View()
	if m.sel == nil || m.sel.area != inComposer {
		return view
	}
	rows := strings.Split(view, "\n")
	for i, row := range rows {
		rows[i] = m.selected(inComposer, i, row, promptWidth)
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
	m.picker.listTop = transcriptTop + top + 4
	if m.picker.fixed {
		m.picker.listTop--
	}
	m.picker.listEnd = transcriptTop + min(height, top+len(boxLines)-3)
	out := make([]string, height)
	for i := range out {
		if j := i - top; j >= 0 && j < len(boxLines) {
			out[i] = strings.Repeat(" ", left) + boxLines[j]
		}
	}
	return strings.Join(out, "\n")
}

func (m *uiModel) header() string {
	st := m.styles
	brand := st.accent.Render("■") + " " + st.bold.Render("KOU") + st.accent.Render("-") + st.bold.Render("CONVEYOR")
	where := st.faint.Render(filepath.Base(m.opt.Workspace)) + st.faint.Render(" / ")
	title := orDefault(m.sessionTitle(), "new session")
	if m.loading {
		title = "loading…"
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
	badge := strings.ToUpper(phase)
	if m.state != idle {
		badge += " " + clock(time.Since(m.started))
	}
	right := st.badge[phase].Render(badge)
	if u := m.tr.Usage; u.Turns > 0 && m.width >= 90 {
		right = st.faint.Render("ctx ") + st.muted.Render(tokens(u.Context)) + "  " +
			st.faint.Render("↑") + st.muted.Render(tokens(u.Input)) + " " +
			st.faint.Render("↓") + st.muted.Render(tokens(u.Output)) + "  " + right
	}
	room := m.width - ansi.StringWidth(brand) - ansi.StringWidth(right) - 4 - ansi.StringWidth(where)
	left := brand + "  "
	if room >= 8 {
		left += where + st.text.Render(ansi.Truncate(title, room, "…"))
	}
	return fitRight(left, right, m.width)
}

var spinner = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

func (m *uiModel) statusLine() string {
	st := m.styles
	width := m.width
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
		left := st.accent.Render(spinner[m.frame%len(spinner)]) + " " + st.text.Render(activity)
		if running := m.tr.Running(); len(running) > 0 && running[0].Tool.Input != "" {
			left += st.faint.Render(" · ") + st.muted.Render(firstLine(running[0].Tool.Input))
		}
		if m.diag != "" {
			left += st.faint.Render(" · " + m.diag)
		}
		right := st.faint.Render(clock(time.Since(m.started)) + "  esc esc stops")
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
	line := st.faint.Render(strings.Join(parts, " · "))
	if m.tr.Interrupted() {
		line = st.warn.Render("◌ interrupted") + st.faint.Render(" — /continue picks it up, esc esc edits the prompt · ") + line
	}
	return line
}

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
	left := st.rule.Render("── ") + style.Render(label) + " "
	if hint != "" {
		left += st.faint.Render(hint) + " "
	}
	if lines := m.input.LineCount(); lines > 1 {
		left += st.faint.Render(fmt.Sprintf("line %d/%d ", m.input.Line()+1, lines))
	}
	effort, _ := m.effortControl()
	right := m.modelControl() + effort
	// The label's hints give way to the controls where the rule is short.
	if ansi.StringWidth(left)+ansi.StringWidth(right) > m.width {
		left = ansi.Truncate(left, max(0, m.width-ansi.StringWidth(right)-1), "") + " "
	}
	fill := max(0, m.width-ansi.StringWidth(left)-ansi.StringWidth(right))
	return left + st.rule.Render(strings.Repeat("─", fill)) + right
}

func (m *uiModel) footer() string {
	st := m.styles
	if m.toast != "" {
		toast := st.toast.Render("✓ " + m.toast)
		return strings.Repeat(" ", max(0, (m.width-ansi.StringWidth(toast))/2)) + toast
	}
	// In compact mode the terminal selects and copies, and ^F leads back.
	copyHint, layout := []string{"drag", "copy"}, []string{"^F", "inline"}
	if m.compact {
		copyHint, layout = nil, []string{"^F", "fullscreen"}
	}
	var hints []string
	changes := []string{"^G", "changes"}
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
		return keyHints(st, m.width, hints...)
	case m.queueFocus >= 0:
		return keyHints(st, m.width, "↑↓", "select", "enter", "edit", "^X", "force in", "⌫", "drop", "⇧↑↓", "move", "esc", "back to the prompt")
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
	return keyHints(st, m.width, hints...)
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
