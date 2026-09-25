package main

import (
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
)

// pickerItem is one row of an overlay list.
type pickerItem struct {
	title   string
	detail  string // right-aligned, dim
	search  string // extra text the filter matches
	current bool
	pinned  bool
	id      string // the session a sessions row stands for
	action  func(*uiModel) tea.Cmd
}

// picker is a filterable overlay list: the command palette, sessions,
// prompt history and the shortcut sheet all share it.
type picker struct {
	kind, title, empty string
	items              []pickerItem
	shown              []int
	cursor, offset     int
	query              textinput.Model
	loading            bool
	rows               int // list rows at the last render, for paging and mouse hits
	listTop, listEnd   int // screen rows of the list that were visible at the last render
	hints              []string
	fixed              bool // a short list of choices, with no filter

	// A sessions picker edits a title in its query field; the filter waits.
	editing, filter string
	// armed is the session a first ctrl+d marked for deletion.
	armed   string
	armedAt time.Time
}

func newPicker(kind, title, empty string, st styles) *picker {
	q := textinput.New()
	q.Prompt = "❯ "
	q.PromptStyle = st.accent
	q.Placeholder = "type to filter"
	q.PlaceholderStyle = st.faint
	q.Focus()
	return &picker{kind: kind, title: title, empty: empty, query: q}
}

func (p *picker) setItems(items []pickerItem) {
	p.items = items
	p.loading = false
	p.refilter()
}

// edit turns the query field into an editor for a session's title.
func (p *picker) edit(id, title string) {
	p.editing, p.filter = id, p.query.Value()
	p.query.Prompt = "✎ "
	p.query.Placeholder = "title · empty restores the first prompt"
	p.query.SetValue(title)
	p.query.CursorEnd()
}

// stopEditing brings the filter back.
func (p *picker) stopEditing() {
	p.editing = ""
	p.query.Prompt = "❯ "
	p.query.Placeholder = "type to filter"
	p.query.SetValue(p.filter)
	p.query.CursorEnd()
}

// selectID moves the cursor to the item for a session, if it is shown.
func (p *picker) selectID(id string) {
	for i, n := range p.shown {
		if p.items[n].id == id {
			p.cursor = i
			return
		}
	}
}

// selectCurrent moves the cursor to the item marked current, if any.
func (p *picker) selectCurrent() {
	for i, n := range p.shown {
		if p.items[n].current {
			p.cursor = i
			return
		}
	}
}

func (p *picker) refilter() {
	query := strings.ToLower(strings.TrimSpace(p.query.Value()))
	type scored struct{ index, score int }
	var matches []scored
	for i, item := range p.items {
		score := fuzzyScore(query, strings.ToLower(item.title))
		if extra := fuzzyScore(query, strings.ToLower(item.search+" "+item.detail)) / 2; extra > score {
			score = extra
		}
		if score > 0 {
			matches = append(matches, scored{i, score})
		}
	}
	if query != "" {
		sort.SliceStable(matches, func(a, b int) bool { return matches[a].score > matches[b].score })
	}
	p.shown = p.shown[:0]
	for _, m := range matches {
		p.shown = append(p.shown, m.index)
	}
	p.cursor, p.offset = 0, 0
}

// fuzzyScore ranks substring matches (earlier and at word starts is better)
// above in-order character matches; 0 means no match.
func fuzzyScore(query, text string) int {
	if query == "" {
		return 1
	}
	if at := strings.Index(text, query); at >= 0 {
		bonus := 0
		if at == 0 || text[at-1] == ' ' || text[at-1] == '/' {
			bonus = 50
		}
		return 200 - min(at, 100) + bonus
	}
	pos, gaps := 0, 0
	for _, r := range query {
		next := strings.IndexRune(text[pos:], r)
		if next < 0 {
			return 0
		}
		gaps += next
		pos += next + len(string(r))
	}
	return max(1, 60-gaps)
}

func (p *picker) move(delta int) {
	if len(p.shown) == 0 {
		return
	}
	p.cursor = min(max(p.cursor+delta, 0), len(p.shown)-1)
}

func (p *picker) selected() *pickerItem {
	if p.cursor < 0 || p.cursor >= len(p.shown) {
		return nil
	}
	return &p.items[p.shown[p.cursor]]
}

// update applies a key and reports whether it chose the selection or closed
// the picker.
func (p *picker) update(msg tea.KeyMsg) (cmd tea.Cmd, choose, closed bool) {
	switch msg.String() {
	case "up", "ctrl+p":
		p.move(-1)
	case "down", "ctrl+n", "tab":
		p.move(1)
	case "shift+tab":
		p.move(-1)
	case "pgup":
		p.move(-max(1, p.rows-1))
	case "pgdown":
		p.move(max(1, p.rows-1))
	case "home":
		p.move(-len(p.shown))
	case "end":
		p.move(len(p.shown))
	case "enter":
		return nil, true, false
	case "esc", "ctrl+c":
		return nil, false, true
	default:
		if p.fixed {
			break
		}
		before := p.query.Value()
		p.query, cmd = p.query.Update(msg)
		if p.query.Value() != before {
			p.refilter()
		}
	}
	return cmd, false, false
}

func (p *picker) view(st styles, width, height int) string {
	inner := max(10, width-4)
	var b strings.Builder
	count := ""
	if !p.loading && !p.fixed {
		count = st.faint.Render(strconv.Itoa(len(p.shown)))
		if len(p.shown) != len(p.items) {
			count = st.faint.Render(strconv.Itoa(len(p.shown)) + "/" + strconv.Itoa(len(p.items)))
		}
	}
	title := st.accentLabel.Render(strings.ToUpper(p.title))
	b.WriteString(title + strings.Repeat(" ", max(1, inner-lipgloss.Width(title)-lipgloss.Width(count))) + count + "\n")
	top := 4 // rows above the list, border included
	if !p.fixed {
		p.query.Width = inner - 3
		b.WriteString(p.query.View() + "\n")
	} else {
		top = 3
	}
	b.WriteString(st.rule.Render(strings.Repeat("─", inner)) + "\n")

	p.rows = max(1, height-top-3)
	switch {
	case p.loading:
		b.WriteString(st.faint.Render("loading…") + "\n")
		p.rows = 1
	case len(p.shown) == 0:
		b.WriteString(st.faint.Render(p.emptyText()) + "\n")
		p.rows = 1
	default:
		p.rows = min(p.rows, len(p.shown))
		// The window can grow or the list shrink between renders.
		p.offset = max(0, min(p.offset, len(p.shown)-p.rows))
		if p.cursor < p.offset {
			p.offset = p.cursor
		}
		if p.cursor >= p.offset+p.rows {
			p.offset = p.cursor - p.rows + 1
		}
		for i := p.offset; i < p.offset+p.rows; i++ {
			b.WriteString(p.row(st, p.items[p.shown[i]], i == p.cursor, inner) + "\n")
		}
	}
	b.WriteString(st.rule.Render(strings.Repeat("─", inner)) + "\n")
	switch {
	case p.editing != "":
		b.WriteString(keyHints(st, inner, "enter", "save the title", "esc", "cancel"))
	case p.armed != "":
		b.WriteString(st.err.Render("ctrl+d again deletes the session") + st.faint.Render(" · any other key keeps it"))
	case p.hints != nil:
		b.WriteString(keyHints(st, inner, p.hints...))
	default:
		b.WriteString(keyHints(st, inner, "↑↓", "move", "enter", "choose", "esc", "close"))
	}
	return st.box.Width(inner + 2).Render(b.String())
}

func (p *picker) emptyText() string {
	if strings.TrimSpace(p.query.Value()) != "" {
		return "nothing matches"
	}
	return p.empty
}

func (p *picker) row(st styles, item pickerItem, selected bool, width int) string {
	mark := " "
	if item.current {
		mark = "●"
	}
	pin := ""
	if item.pinned {
		pin = "◆ "
	}
	detail := item.detail
	room := width - 3 - ansi.StringWidth(detail) - 2
	if room < 12 {
		detail, room = "", width-3
	}
	title := ansi.Truncate(item.title, room-ansi.StringWidth(pin), "…")
	gap := strings.Repeat(" ", max(1, width-3-ansi.StringWidth(pin+title)-ansi.StringWidth(detail)))
	if selected {
		return st.accent.Render("▌") + st.selected.Render(mark+" "+pin+title+gap+detail)
	}
	return " " + st.accent.Render(mark) + " " + st.accent.Render(pin) + st.text.Render(title) + gap + st.faint.Render(detail)
}

// rowAt maps a screen row to a list index, or -1 when no list row was drawn
// there.
func (p *picker) rowAt(y int) int {
	i := y - p.listTop
	if y >= p.listEnd || i < 0 || i >= p.rows || p.offset+i >= len(p.shown) || len(p.items) == 0 {
		return -1
	}
	return p.offset + i
}

func keyHints(st styles, width int, pairs ...string) string {
	var parts []string
	used := 0
	for i := 0; i+1 < len(pairs); i += 2 {
		part := st.key.Render(pairs[i]) + " " + st.keyHint.Render(pairs[i+1])
		w := lipgloss.Width(part)
		if used > 0 && used+3+w > width {
			break
		}
		if used > 0 {
			used += 3
		}
		used += w
		parts = append(parts, part)
	}
	return strings.Join(parts, "   ")
}
