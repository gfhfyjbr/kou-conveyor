package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Files in prompts. A $ links a file or a folder of the workspace, as in
// the browser cockpit (cockpit.FileLinks): "$cmd/main.go",
// "$cmd/main.go:120-160" for those lines, "$cmd/" for a folder's entries,
// $"a b.txt" for a path with spaces. Typing one lists what it can complete
// to at the foot of the transcript: ↑↓ choose, tab completes, and so does
// enter once a choice was made with the arrows (otherwise enter runs the
// prompt as it is), esc closes, and a folder completed lists what is in
// it. A tray above the composer says what its text links. The runner reads
// what the model sees of each link as the prompt runs — the lines asked
// for, else the file's beginning, the whole of a small one — and the model
// reads the rest itself; under a sent prompt its files say what the model
// saw of each, which a click there shows.

// linkToken is the $ reference the cursor is in: where its $ is in the
// text before the cursor, the path typed so far, and whether it is quoted.
type linkToken struct {
	start  int
	query  string
	quoted bool
}

// linkList is what the reference being typed completes to, while it shows.
type linkList struct {
	token     *linkToken
	items     []cockpit.FileMatch
	cursor    int
	top       int  // the first item in view
	moved     bool // a choice was made with the arrows
	seq       int  // of the latest request for matches
	dismissed string
}

type (
	// linkMatchesMsg brings what a reference completes to.
	linkMatchesMsg struct {
		seq     int
		matches []cockpit.FileMatch
	}
	// linkedMsg brings what the model saw of the files prompts linked, by
	// message ID.
	linkedMsg struct {
		session string
		files   map[string][]cockpit.LinkedFile
	}
)

const (
	// linkMatches is how many matches a reference lists, and linkRows how
	// many show at once.
	linkMatches = 50
	linkRows    = 8
	// linkedHead and linkedTail are the lines of a file shown under its
	// prompt, around the lines left out.
	linkedHead, linkedTail = 40, 10
)

// files lists the workspace's files, for completing references.
func (m *uiModel) files() *cockpit.FileIndex {
	if m.fileIndex == nil {
		m.fileIndex = cockpit.NewFileIndex(m.opt.Workspace, m.opt.SessionDir, m.opt.LogDir)
	}
	return m.fileIndex
}

// ---------------------------------------------------------------- completing

// textBeforeCursor is the composer's text before the cursor.
func (m *uiModel) textBeforeCursor() string {
	lines := strings.Split(m.input.Value(), "\n")
	row := m.input.Line()
	if row <= 0 || row >= len(lines) {
		return m.beforeCursor()
	}
	return strings.Join(lines[:row], "\n") + "\n" + m.beforeCursor()
}

// afterCursor is the text of the cursor's line after it.
func (m *uiModel) afterCursor() string {
	lines := strings.Split(m.input.Value(), "\n")
	row := m.input.Line()
	if row < 0 || row >= len(lines) {
		return ""
	}
	info := m.input.LineInfo()
	runes := []rune(lines[row])
	return string(runes[clamp(info.StartColumn+info.ColumnOffset, 0, len(runes)):])
}

// linkToken returns the reference the cursor is in, while the composer has
// the keys.
func (m *uiModel) linkToken() *linkToken {
	if !m.input.Focused() || m.picker != nil || m.form != nil || m.queueFocus >= 0 || m.changes.focused {
		return nil
	}
	start, query, quoted, ok := cockpit.LinkQuery(m.textBeforeCursor())
	if !ok {
		return nil
	}
	return &linkToken{start: start, query: query, quoted: quoted}
}

// followLinks follows the composer: what was typed opens the list of what
// the reference at the cursor completes to, and a cursor that moves takes
// an open list along, or closes it once it leaves the reference.
func (m *uiModel) followLinks(typed bool) tea.Cmd {
	l := &m.links
	token := m.linkToken()
	switch {
	case token == nil:
		m.closeLinks()
		return nil
	case typed && l.dismissed == m.input.Value():
		return nil
	case !typed && (len(l.items) == 0 || l.token != nil && *l.token == *token):
		return nil
	}
	l.dismissed, l.token = "", token
	l.seq++
	seq, query, index := l.seq, token.query, m.files()
	return func() tea.Msg {
		matches, _ := index.Complete(context.Background(), query, linkMatches)
		return linkMatchesMsg{seq: seq, matches: matches}
	}
}

// linkMatched shows what the reference being typed completes to: nothing,
// for a $ that names nothing, such as $HOME.
func (m *uiModel) linkMatched(msg linkMatchesMsg) {
	l := &m.links
	if msg.seq != l.seq || m.linkToken() == nil {
		return
	}
	l.items, l.cursor, l.top, l.moved = msg.matches, 0, 0, false
}

func (m *uiModel) closeLinks() {
	l := &m.links
	l.items, l.token = nil, nil
	l.seq++ // what is on its way is not wanted
}

// linkKey takes the keys of the list while it shows.
func (m *uiModel) linkKey(key string) (tea.Cmd, bool) {
	l := &m.links
	if !m.linksShown() {
		return nil, false
	}
	switch key {
	case "esc":
		l.dismissed = m.input.Value()
		m.closeLinks()
		return nil, true
	case "up", "down":
		step := 1
		if key == "up" {
			step = -1
		}
		l.cursor = (l.cursor + step + len(l.items)) % len(l.items)
		l.moved = true
		return nil, true
	case "tab":
		return m.acceptLink(l.cursor), true
	case "enter":
		if l.moved {
			return m.acceptLink(l.cursor), true
		}
	}
	return nil, false
}

// acceptLink puts item n in place of the reference at the cursor: a file
// with a space after it, a folder so that completing goes on inside it.
func (m *uiModel) acceptLink(n int) tea.Cmd {
	l := &m.links
	token := m.linkToken()
	if n < 0 || n >= len(l.items) || token == nil {
		m.closeLinks()
		return nil
	}
	item := l.items[n]
	typed := utf8.RuneCountInString(m.textBeforeCursor()[token.start:])
	// What the reference has after the cursor goes too.
	after := m.afterCursor()
	rest := len([]rune(after[:len(after)-len(strings.TrimLeftFunc(after, func(r rune) bool { return !unicode.IsSpace(r) }))]))
	if token.quoted {
		rest = 0
		if closing := strings.IndexByte(after, '"'); closing >= 0 {
			rest = utf8.RuneCountInString(after[:closing+1])
		}
	}
	for range typed {
		m.input, _ = m.input.Update(tea.KeyMsg{Type: tea.KeyBackspace})
	}
	for range rest {
		m.input, _ = m.input.Update(tea.KeyMsg{Type: tea.KeyDelete})
	}
	label := cockpit.LinkLabel(item.Path)
	if item.Directory {
		m.input.InsertString(label)
		if strings.HasSuffix(label, `"`) {
			// Inside the quotes, where the folder's entries complete.
			m.input, _ = m.input.Update(tea.KeyMsg{Type: tea.KeyLeft})
		}
	} else {
		m.input.InsertString(label + " ")
	}
	m.resize()
	m.closeLinks()
	if item.Directory {
		return m.followLinks(true)
	}
	return nil
}

// linksBox draws the list at most width columns wide and room rows tall,
// the item under the cursor marked.
func (m *uiModel) linksBox(width, room int) []string {
	l := &m.links
	shown := min(len(l.items), linkRows, room-2)
	if shown <= 0 || width < 24 {
		return nil
	}
	// The item under the cursor stays in view.
	l.top = clamp(l.top, max(0, l.cursor-shown+1), min(l.cursor, len(l.items)-shown))
	st := m.styles
	inner := width - 2
	border, corner := st.rule2, st.accent
	title := st.label.Render("FILES") + " " + st.ghost.Render(fmt.Sprintf("%d", len(l.items)))
	if len(l.items) == linkMatches {
		title += st.ghost.Render("+")
	}
	title = " " + title + " "
	top := corner.Render("┌") + border.Render("─") + title + border.Render(strings.Repeat("─", max(0, inner-1-ansi.StringWidth(title)))) + corner.Render("┐")
	lines := []string{top}
	for i := l.top; i < l.top+shown; i++ {
		item := l.items[i]
		glyph, marker := "▤", " "
		if item.Directory {
			glyph = "▢"
		}
		path := strings.TrimSuffix(item.Path, "/")
		dir, name := "", path
		if cut := strings.LastIndexByte(path, '/'); cut >= 0 {
			dir, name = path[:cut+1], path[cut+1:]
		}
		if item.Directory {
			name += "/"
		}
		nameStyle, glyphStyle := st.bold, st.ghost
		if i == l.cursor {
			marker, nameStyle, glyphStyle = st.accent.Render("▌"), st.accentLabel, st.accent
		}
		row := marker + glyphStyle.Render(glyph) + " " + st.faint.Render(cockpit.Clean(dir)) + nameStyle.Render(cockpit.Clean(name))
		row = fit(row, inner)
		lines = append(lines, border.Render("│")+row+strings.Repeat(" ", max(0, inner-ansi.StringWidth(row)))+border.Render("│"))
	}
	hint := " ↑↓ choose · tab completes · esc closes "
	if l.moved {
		hint = " ↑↓ choose · tab or enter completes · esc closes "
	}
	if ansi.StringWidth(hint) > inner-2 {
		hint = ""
	}
	bottom := corner.Render("└") + border.Render(strings.Repeat("─", max(0, inner-1-ansi.StringWidth(hint)))) + st.faint.Render(hint) + border.Render("─") + corner.Render("┘")
	return append(lines, bottom)
}

// linksShown reports whether the list shows: it has items, and the cursor
// is in the reference they complete.
func (m *uiModel) linksShown() bool {
	return len(m.links.items) != 0 && m.linkToken() != nil
}

// withLinks lays the list over the foot of lines, the transcript's rows
// from screen row top on, over the composer's columns.
func (m *uiModel) withLinks(lines []string, top int) []string {
	m.linksArea = area{}
	if !m.linksShown() {
		return lines
	}
	width := m.stageWidth() - 2*margin
	box := m.linksBox(width, len(lines))
	if len(box) == 0 {
		return lines
	}
	row := len(lines) - len(box)
	boxWidth := ansi.StringWidth(box[0])
	for i, b := range box {
		line := lines[row+i]
		if pad := margin + boxWidth - ansi.StringWidth(line); pad > 0 {
			line += strings.Repeat(" ", pad)
		}
		lines[row+i] = ansi.Truncate(line, margin, "") + "\x1b[m" + b + "\x1b[m" + ansi.TruncateLeft(line, margin+boxWidth, "")
	}
	m.linksArea = area{top: top + row, left: margin, bottom: top + row + len(box), right: margin + boxWidth}
	return lines
}

// linkAt returns the item of the list at a cell of the screen.
func (m *uiModel) linkAt(x, y int) (int, bool) {
	a := m.linksArea
	if !a.contains(x, y) || y == a.top || y == a.bottom-1 {
		return 0, false
	}
	n := m.links.top + y - a.top - 1
	return n, n >= 0 && n < len(m.links.items)
}

// ---------------------------------------------------------------- the tray

// composerLinks are the links of the composer's text, as the runner will
// get them; a text is read again when it changes, or a moment later.
func (m *uiModel) composerLinks() []cockpit.FileLink {
	text := m.input.Value()
	if !strings.Contains(text, "$") {
		return nil
	}
	c := &m.linkCache
	if c.text != text || time.Since(c.at) > 2*time.Second {
		c.text, c.at, c.links = text, time.Now(), cockpit.FileLinks(text, m.opt.Workspace)
	}
	return c.links
}

// linksRows is how many rows the tray of links takes: one, while the
// composer's text links something.
func (m *uiModel) linksRows() int {
	if !m.ready || len(m.composerLinks()) == 0 {
		return 0
	}
	return 1
}

// linksView renders the tray of links, which goes on an edge of the dock:
// each link with what the model will see of it.
func (m *uiModel) linksView() []string {
	links := m.composerLinks()
	if len(links) == 0 || !m.ready {
		return nil
	}
	st := m.styles
	room := m.edgeInner()
	line := st.rule2.Render("─ ") + st.label.Render(fmt.Sprintf("FILES %d", len(links))) + " "
	for i, link := range links {
		glyph := "▤"
		if link.Directory {
			glyph = "▢"
		}
		item := st.accent.Render(glyph) + " " + st.text.Render(cockpit.Clean(link.Label)) + " " + st.faint.Render(linkDetail(link))
		if ansi.StringWidth(line)+ansi.StringWidth(item)+12 > room && i > 0 {
			line += st.faint.Render(fmt.Sprintf("+%d more ", len(links)-i))
			break
		}
		line += item + "   "
	}
	return []string{line}
}

// linkDetail says what a link of the composer names.
func linkDetail(link cockpit.FileLink) string {
	switch {
	case link.Directory:
		return "folder"
	case link.StartLine > 0 && link.EndLine == link.StartLine:
		return "line " + strconv.Itoa(link.StartLine)
	case link.StartLine > 0 && link.EndLine > 0:
		return fmt.Sprintf("lines %d–%d", link.StartLine, link.EndLine)
	case link.StartLine > 0:
		return fmt.Sprintf("from line %d", link.StartLine)
	}
	return size(link.Size)
}

// ---------------------------------------------------------------- sent prompts

// linkedKey is where what the model saw of a prompt's files is kept.
func (m *uiModel) linkedKey(e *cockpit.Entry) string {
	return m.sessionID + "/" + strings.TrimPrefix(e.ID, "input:")
}

// linkedLines are the lines of a prompt's card that say what it linked:
// each file with what the model saw of it, and open, those lines.
func (m *uiModel) linkedLines(e *cockpit.Entry, width int, open bool) []string {
	st := m.styles
	var spans []span
	for i, file := range e.Files {
		if i > 0 {
			spans = append(spans, span{" · ", st.ghost})
		}
		glyph, style := "▤ ", st.accent
		if file.Directory {
			glyph = "▢ "
		}
		if file.Error != "" {
			style = st.err
		}
		spans = append(spans, span{glyph, style}, span{file.Label, st.muted}, span{" " + file.Describe(), st.faint})
	}
	chevron := " ▸"
	if open {
		chevron = " ▾"
	}
	lines := wrapSpans(append(spans, span{chevron, st.ghost}), width, "", "")
	if !open {
		return lines
	}
	seen, ok := m.linked[m.linkedKey(e)]
	if !ok {
		return append(lines, st.ghost.Render("loading what the model saw…"))
	}
	if seen == nil {
		return append(lines, st.warn.Render("what the model saw is not in the session file"))
	}
	for _, file := range seen {
		glyph := "▤ "
		if file.Directory {
			glyph = "▢ "
		}
		lines = append(lines, "", st.accent.Render(glyph)+st.text.Render(fit(cockpit.Clean(file.Path), width-40))+
			st.ghost.Render(" · as the model saw it · "+file.Describe()))
		if file.Content == "" {
			continue
		}
		rows := strings.Split(file.Content, "\n")
		number := len(strconv.Itoa(file.From + len(rows) - 1))
		clipped := clipLines(rows, linkedHead, linkedTail)
		for i, row := range clipped {
			if row == "\x00" {
				lines = append(lines, st.ghost.Render(fmt.Sprintf("%*s", number, "⋯")))
				continue
			}
			// Past the lines left out, the numbers count from the end.
			n := file.From + i
			if i > linkedHead {
				n = file.From + len(rows) - (len(clipped) - i)
			}
			lead := ""
			if !file.Directory {
				lead = st.ghost.Render(fmt.Sprintf("%*d", number, n)) + "  "
			}
			text := strings.ReplaceAll(cockpit.Clean(row), "\t", "    ")
			lines = append(lines, lead+st.muted.Render(ansi.Truncate(text, max(4, width-number-2), "…")))
		}
	}
	return lines
}

// loadLinked reads what the model saw of the files of the prompts given,
// those not read yet nor being read.
func (m *uiModel) loadLinked(entries ...*cockpit.Entry) tea.Cmd {
	if m.fresh {
		return nil
	}
	var messages []string
	for _, e := range entries {
		if e == nil || e.Kind != cockpit.KindUser || len(e.Files) == 0 || e.State != "" {
			continue
		}
		key := m.linkedKey(e)
		if _, ok := m.linked[key]; !ok && !m.linking[key] {
			messages = append(messages, strings.TrimPrefix(e.ID, "input:"))
			if m.linking == nil {
				m.linking = make(map[string]bool)
			}
			m.linking[key] = true
		}
	}
	if len(messages) == 0 {
		return nil
	}
	dir, session := m.opt.SessionDir, m.sessionID
	return func() tea.Msg {
		files := make(map[string][]cockpit.LinkedFile, len(messages))
		for _, message := range messages {
			got, err := cockpit.PromptFiles(dir, session, message)
			if err != nil {
				got = nil
			}
			files[message] = got
		}
		return linkedMsg{session: session, files: files}
	}
}

// linkedLoaded keeps what the model saw, and draws the prompts anew.
func (m *uiModel) linkedLoaded(msg linkedMsg) {
	if m.linked == nil {
		m.linked = make(map[string][]cockpit.LinkedFile)
	}
	for message, files := range msg.files {
		m.linked[msg.session+"/"+message] = files
		delete(m.linking, msg.session+"/"+message)
		if msg.session == m.sessionID {
			delete(m.cache, "input:"+message)
		}
	}
	m.refreshKeep()
}

// openedPrompts are the prompts whose files show open.
func (m *uiModel) openedPrompts() []*cockpit.Entry {
	if m.tr == nil || !m.expandAll && len(m.expanded) == 0 {
		return nil
	}
	var opened []*cockpit.Entry
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser && len(e.Files) != 0 && m.isOpen(e) {
			opened = append(opened, e)
		}
	}
	return opened
}
