package main

import (
	"fmt"
	"slices"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The queue. What is written while the agent works waits above the composer
// (see cockpit.Queue). Enter queues it: it runs as a prompt of its own once
// the run ends, however long that takes. Ctrl+X forces it: the running agent
// reads it once the tool calls it is making finish, after their results,
// and never in the middle of a response. A run that is stopped or fails
// pauses the queue; enter on an empty composer then sends the next prompt.
// ↑ on an empty composer gives the queue the keys, to edit, force, drop or
// move a prompt.

// forceKeys are how terminals that report modifiers send ctrl+enter, which
// forces like ctrl+x: CSI u (kitty, WezTerm, Ghostty and others) and xterm's
// modifyOtherKeys.
var forceKeys = []string{"\x1b[13;5u", "\x1b[27;5;13~"}

// queueEdit is a queued prompt taken into the composer to be edited. The
// queue holds its place, and does not move on without it.
type queueEdit struct {
	session string
	item    cockpit.QueueItem
	index   int           // where it goes back among the queued prompts
	draft   string        // what the composer held before
	images  []*attachment // and its images
}

// queueRowsMax is how many prompts the queue shows at most.
const queueRowsMax = 4

// flow is the forced prompts' mark in motion: on their way to the agent.
var flow = []string{"›  ", "›› ", "›››", " ››", "  ›", "   "}

// queue returns the queue of the session in view.
func (m *uiModel) queue() *cockpit.Queue {
	if m.queues == nil {
		m.queues = make(map[string]*cockpit.Queue)
	}
	q := m.queues[m.sessionID]
	if q == nil {
		q = &cockpit.Queue{}
		m.queues[m.sessionID] = q
	}
	return q
}

// queued is how many rows of prompts the queue has: its own, and the one in
// the composer being edited.
func (m *uiModel) queued() int {
	n := 0
	if q := m.queues[m.sessionID]; q != nil {
		n = len(q.Items)
	}
	if e := m.queueEdit; e != nil && e.session == m.sessionID {
		n++
	}
	return n
}

// keep clears the composer of a prompt it hands on, and remembers it in the
// prompt history.
func (m *uiModel) keep(text string) {
	m.input.Reset()
	m.historyPos, m.draft = -1, ""
	// A draft set aside for history is gone, and its images with it.
	m.dispose(m.draftImages)
	m.draftImages = nil
	m.history = appendHistory(m.history, text, m.sessionID, m.opt.model)
	if merged, err := saveHistory(m.opt.historyFile, m.history); err == nil {
		m.history = merged
	}
}

// enqueue keeps the composer's text for the agent at work: queued, it runs
// once the run ends; forced, the agent reads it after its tool calls.
func (m *uiModel) enqueue(text string, force bool) tea.Cmd {
	switch {
	case m.external:
		return m.notify("this session is running in another window or terminal", "warn")
	case strings.TrimSpace(cockpit.Clean(text)) == "":
		return m.notify("the prompt has no printable text", "warn")
	}
	q := m.queue()
	// It runs with the model chosen now, whatever is chosen before it runs.
	item := q.Add(text, m.nextModel())
	q.SetImages(item.ID, m.composerImages(text))
	m.keep(text)
	m.clearImages() // they wait with the prompt
	m.resize()
	if force {
		return m.force(item.ID)
	}
	if q.Paused {
		return m.notify("queued — the queue is paused: enter on an empty prompt sends the next", "info")
	}
	return m.notify(fmt.Sprintf("queued #%d — it runs when the agent finishes · ctrl+x forces it in sooner", q.Queued()), "info")
}

// force hands a queued prompt to the running agent. Between runs it runs
// the prompt instead.
func (m *uiModel) force(id string) tea.Cmd {
	q := m.queue()
	item, _, ok := q.Get(id)
	if !ok || item.Forced {
		return nil
	}
	switch {
	case m.state == idle:
		if block := m.runBlocked(); block != "" {
			return m.notify(block, "warn")
		}
		q.Remove(id)
		cmd := m.start(item.Text, startOptions{messageID: item.ID, images: item.Images, model: item.Model})
		if m.state == idle {
			q.Insert(0, item) // it did not start
		}
		m.clampQueueFocus()
		m.resize()
		return cmd
	case m.state == stopping:
		return m.notify("the run is stopping — the message waits in the queue", "warn")
	case m.compaction != nil:
		return m.notify("the conversation is being compacted — the message runs after the summary", "info")
	case !m.job.CanSteer():
		return m.notify("this runner takes no messages while it runs (make build updates it) — the message waits for the run to end", "warn")
	}
	q.Force(id)
	if err := m.job.Steer(item.ID, item.Text, item.Images...); err != nil {
		q.Unforce(id)
		return m.notify(err.Error()+" — the message waits for the run to end", "warn")
	}
	m.clampQueueFocus()
	m.resize()
	// The running agent reads it with the model it runs with.
	if running := m.runModel(); item.Model != "" && running != "" && item.Model != running {
		return m.notify("⚡ forced — the agent reads it "+m.forcedWhen()+" with "+running+"; "+item.Model+" takes the prompts that run next", "info")
	}
	return m.notify("⚡ forced — the agent reads it "+m.forcedWhen(), "info")
}

// forceComposer forces the composer's text; with nothing in it, the next
// queued prompt. Between runs, the text just runs.
func (m *uiModel) forceComposer() tea.Cmd {
	if e := m.queueEdit; e != nil {
		// The prompt being edited goes first, forced.
		text := m.input.Value()
		images := m.composerImages(text)
		m.queueEdit = nil
		m.input.SetValue(e.draft)
		m.clearImages()
		m.images = e.images
		m.input.CursorEnd()
		m.resize()
		if strings.TrimSpace(cockpit.Clean(text)) == "" {
			return m.notify("the message was dropped from the queue", "info")
		}
		e.item.Text, e.item.Images = text, images
		m.queue().Insert(0, e.item)
		return m.force(e.item.ID)
	}
	text := strings.TrimSpace(m.input.Value())
	switch {
	case text == "":
		if q := m.queues[m.sessionID]; q != nil && q.Queued() > 0 {
			return m.force(q.Items[q.Forced()].ID)
		}
		if m.state == idle {
			return nil
		}
		return m.notify("ctrl+x forces a message into the running agent — write it first", "info")
	case strings.HasPrefix(text, "/") || m.edit != nil || m.state == idle:
		return m.submit()
	}
	return m.enqueue(text, true)
}

// forcedWhen says when the running agent reads a forced prompt.
func (m *uiModel) forcedWhen() string {
	if running := m.tr.Running(); len(running) != 0 {
		if input := firstLine(running[0].Tool.Input); input != "" {
			return "after " + ansi.Truncate(input, 32, "…")
		}
		return "after its tool calls"
	}
	return "after the step it is on"
}

// queueDelivered takes the forced prompts the runner recorded out of the
// queue: the transcript shows them now, where the agent read them.
func (m *uiModel) queueDelivered() tea.Cmd {
	q := m.queues[m.sessionID]
	if q == nil || q.Forced() == 0 {
		return nil
	}
	delivered := q.Delivered(m.tr)
	if len(delivered) == 0 {
		return nil
	}
	m.clampQueueFocus()
	m.resize()
	return m.showToast("⚡ the agent has it: " + cockpit.Headline(delivered[len(delivered)-1].Text, 48))
}

// settleQueue takes the end of a run into account: forced prompts the agent
// never read go back to the head of the queue, a run that did not end on its
// own pauses it, and one that did runs the next prompt.
func (m *uiModel) settleQueue(kind string) tea.Cmd {
	q := m.queues[m.sessionID]
	if q == nil || len(q.Items) == 0 {
		return nil
	}
	q.Settle(kind)
	m.clampQueueFocus()
	m.resize()
	if q.Paused {
		return m.notify(fmt.Sprintf("the queue is paused as the run %s — enter on an empty prompt sends the next, ↑ edits it", kind), "warn")
	}
	return m.runNext()
}

// runNext runs the next queued prompt, unless the queue waits: for the user,
// for a run, or for a prompt being edited.
func (m *uiModel) runNext() tea.Cmd {
	q := m.queues[m.sessionID]
	if q == nil || m.queueEdit != nil || m.state != idle || m.editAfterStop || m.loading {
		return nil
	}
	item, ok := q.Next()
	if !ok {
		return nil
	}
	left := q.Queued()
	cmd := m.start(item.Text, startOptions{messageID: item.ID, images: item.Images, model: item.Model})
	if m.state == idle {
		// It could not start: it waits for the user, first in line.
		q.Insert(0, item)
		q.Paused = true
	} else if left > 0 {
		cmd = tea.Batch(cmd, m.notify(fmt.Sprintf("running the next queued prompt · %d more after it", left), "info"))
	}
	m.clampQueueFocus()
	m.resize()
	return cmd
}

// resumeQueue lifts the pause and runs the next prompt.
func (m *uiModel) resumeQueue() tea.Cmd {
	q := m.queues[m.sessionID]
	if q == nil || len(q.Items) == 0 {
		return m.notify("the queue is empty", "info")
	}
	if block := m.runBlocked(); block != "" && m.state == idle {
		return m.notify(block, "warn")
	}
	q.Paused = false
	if m.state != idle {
		return m.notify("the queue runs when the agent finishes", "info")
	}
	return m.runNext()
}

// editQueued takes a queued prompt into the composer. Enter puts it back
// where it was, esc as it was; meanwhile the queue waits for it.
func (m *uiModel) editQueued(id string) tea.Cmd {
	q := m.queue()
	item, index, ok := q.Get(id)
	switch {
	case !ok:
		return nil
	case item.Forced:
		return m.notify("the agent has this message already", "info")
	}
	if m.edit != nil {
		m.cancelEdit()
	}
	if m.queueEdit != nil {
		m.endQueueEdit(m.queueEdit.item.Text)
	}
	q.Remove(id)
	m.queueEdit = &queueEdit{session: m.sessionID, item: item, index: index, draft: m.input.Value(), images: m.images}
	m.images = nil
	m.blurQueue()
	m.input.SetValue(item.Text)
	cmd := m.setImages(item.Images)
	m.input.CursorEnd()
	m.resize()
	return cmd
}

// endQueueEdit puts the prompt being edited back in its place with text;
// without text, it leaves the queue. What the composer held comes back.
func (m *uiModel) endQueueEdit(text string) tea.Cmd {
	e := m.queueEdit
	if e == nil {
		return nil
	}
	images := m.composerImages(text)
	m.queueEdit = nil
	m.input.SetValue(e.draft)
	m.clearImages()
	m.images = e.images
	m.input.CursorEnd()
	q := m.queues[e.session]
	var cmd tea.Cmd
	if strings.TrimSpace(cockpit.Clean(text)) == "" {
		cmd = m.notify("the message was dropped from the queue", "info")
	} else {
		e.item.Text, e.item.Images = text, images
		q.Insert(e.index, e.item)
	}
	m.resize()
	if e.session == m.sessionID && m.state == idle && !q.Paused {
		cmd = tea.Batch(cmd, m.runNext())
	}
	return cmd
}

// focusQueue gives the queue the keys, on the prompt at index. The composer
// goes dim meanwhile.
func (m *uiModel) focusQueue(index int) {
	m.queueFocus = index
	m.clampQueueFocus()
	if m.queueFocus >= 0 {
		m.blurChanges()
		m.input.Blur()
	}
}

// blurQueue gives the composer the keys back.
func (m *uiModel) blurQueue() {
	if m.queueFocus >= 0 {
		m.queueFocus = -1
		m.input.Focus()
	}
}

func (m *uiModel) clampQueueFocus() {
	q := m.queues[m.sessionID]
	switch {
	case m.queueFocus < 0:
	case q == nil || len(q.Items) == 0:
		m.blurQueue()
	default:
		m.queueFocus = min(m.queueFocus, len(q.Items)-1)
	}
}

// queueKey handles a key while the queue has the keys. It reports false for
// a key the queue has no use for, which goes to the composer and takes the
// keys back: typing just goes on there.
func (m *uiModel) queueKey(msg tea.KeyMsg) (tea.Cmd, bool) {
	m.clampQueueFocus()
	q := m.queues[m.sessionID]
	if m.queueFocus < 0 || q == nil {
		return nil, false
	}
	item := q.Items[m.queueFocus]
	move := func(step int) {
		if item.Forced {
			return
		}
		_, index, _ := q.Get(item.ID)
		q.Move(item.ID, max(0, index+step))
		_, index, _ = q.Get(item.ID)
		m.queueFocus = q.Forced() + index
	}
	switch msg.String() {
	case "up":
		m.queueFocus = max(0, m.queueFocus-1)
	case "down":
		if m.queueFocus == len(q.Items)-1 {
			m.blurQueue()
		} else {
			m.queueFocus++
		}
	case "home":
		m.queueFocus = 0
	case "end":
		m.queueFocus = len(q.Items) - 1
	case "shift+up", "alt+up":
		move(-1)
	case "shift+down", "alt+down":
		move(1)
	case "enter":
		return m.editQueued(item.ID), true
	case "ctrl+x":
		return m.force(item.ID), true
	case "backspace", "delete":
		if item.Forced {
			return m.notify("the agent has this message already — it cannot be taken back", "info"), true
		}
		q.Remove(item.ID)
		m.clampQueueFocus()
		m.resize()
		return m.notify("dropped from the queue: "+cockpit.Headline(item.Text, 48), "info"), true
	case "esc", "alt+esc", "tab":
		m.blurQueue()
	default:
		m.blurQueue()
		return nil, false
	}
	return nil, true
}

// queueRows is how many screen rows the queue takes: a rule and its prompts.
func (m *uiModel) queueRows() int {
	n := m.queued()
	if n == 0 || !m.ready {
		return 0
	}
	return 1 + min(n, m.queueRoom())
}

// queueRoom is how many prompts the queue shows.
func (m *uiModel) queueRoom() int {
	switch {
	case m.height < 14:
		return 1
	case m.height < 24:
		return 2
	}
	return queueRowsMax
}

// queueRow is a row of the queue: a prompt, or the place of one being edited.
type queueRow struct {
	item    cockpit.QueueItem
	index   int // in the queue's items; -1 for the one being edited
	number  int // among the queued prompts, from 1; 0 for a forced one
	editing bool
}

func (m *uiModel) queueList() []queueRow {
	var rows []queueRow
	q := m.queues[m.sessionID]
	number := 0
	if q != nil {
		for i, item := range q.Items {
			if !item.Forced {
				number++
			}
			n := number
			if item.Forced {
				n = 0
			}
			rows = append(rows, queueRow{item: item, index: i, number: n})
		}
	}
	if e := m.queueEdit; e != nil && e.session == m.sessionID {
		forced := 0
		if q != nil {
			forced = q.Forced()
		}
		at := min(forced+e.index, len(rows))
		rows = slices.Insert(rows, at, queueRow{item: e.item, index: -1, editing: true})
		for i := at; i < len(rows); i++ {
			rows[i].number = i - forced + 1
		}
	}
	return rows
}

// queueWindow is the part of the queue on screen: the first prompts, or the
// ones around the selected one.
func (m *uiModel) queueWindow(rows []queueRow) (int, int) {
	room := m.queueRoom()
	if len(rows) <= room {
		return 0, len(rows)
	}
	start := 0
	if m.queueFocus >= room {
		start = m.queueFocus - room + 1
	}
	return start, start + room
}

// queueView renders the queue: a rule that says what it waits for, then its
// prompts, forced ones first.
func (m *uiModel) queueView(hover hoverTarget) []string {
	if m.queueRows() == 0 {
		return nil
	}
	st := m.styles
	width := m.width
	q := m.queues[m.sessionID]
	if q == nil {
		q = &cockpit.Queue{}
	}
	rows := m.queueList()
	start, end := m.queueWindow(rows)
	waiting := q.Paused || m.state == idle && m.queueEdit == nil

	// The rule.
	left := st.rule.Render("── ") + st.label.Render(fmt.Sprintf("QUEUE %d", len(rows))) + " "
	var detail string
	switch {
	case waiting:
		left += st.warn.Render("⏸ PAUSED") + " "
		detail = "enter on an empty prompt sends the next"
	case q.Forced() > 0 && len(rows) > q.Forced():
		detail = fmt.Sprintf("⚡ %d in after the running tools · %d when the agent finishes", q.Forced(), len(rows)-q.Forced())
	case q.Forced() > 0:
		detail = "⚡ goes in after the running tools"
	case m.state == idle:
		detail = "waits for the prompt being edited"
	default:
		detail = "runs when the agent finishes"
	}
	left += st.faint.Render(detail) + " "
	var right string
	switch {
	case m.queueFocus >= 0:
		right = st.accentLabel.Render(" esc ") + st.faint.Render("back ")
	case end-start < len(rows):
		right = st.faint.Render(fmt.Sprintf(" +%d more · ↑ ", len(rows)-(end-start)))
	case m.input.Value() == "" && m.queueEdit == nil:
		right = st.faint.Render(" ↑ edits ")
	}
	fill := max(0, width-ansi.StringWidth(left)-ansi.StringWidth(right)-2)
	lines := []string{fit(left+st.rule.Render(strings.Repeat("─", fill))+right+st.rule.Render("──"), width)}

	for i := start; i < end; i++ {
		lines = append(lines, m.queueLine(rows[i], i, q, waiting, hover))
	}
	return lines
}

func (m *uiModel) queueLine(row queueRow, at int, q *cockpit.Queue, waiting bool, hover hoverTarget) string {
	st := m.styles
	width := m.width
	selected := m.queueFocus >= 0 && row.index == m.queueFocus
	bar := " "
	if selected {
		bar = st.accent.Render("▌")
	}
	var mark, text, right string
	switch {
	case row.editing:
		mark = st.accent.Render(fmt.Sprintf("%3s", "✎"))
		text = st.muted.Italic(true).Render("editing in the composer — enter puts it back, esc keeps it as it was")
	case row.item.Forced:
		mark = st.accent.Render(fmt.Sprintf("%3s", "⚡"))
		text = st.text.Render(firstLine(row.item.Text))
		motion := flow[(m.frame/2)%len(flow)]
		if st.noColor {
			motion = "»"
		}
		right = st.accent.Render(motion) + " " + st.faint.Render(m.forcedWhen())
	default:
		mark = st.faint.Render(fmt.Sprintf("%3d", row.number))
		text = st.text.Render(firstLine(row.item.Text))
		switch {
		case selected:
			right = keyHints(st, 60, "enter", "edit", "^X", "force", "⌫", "drop", "⇧↑↓", "move")
		case row.number == 1 && !waiting:
			right = st.faint.Render("next")
		}
		// A prompt queued with another model than the session's next says so.
		if model := row.item.Model; model != "" && model != m.nextModel() && !selected {
			right = strings.TrimSpace(st.faint.Render(ansi.Truncate(model, 24, "…")) + " " + right)
		}
	}
	if selected && row.item.Forced {
		right = st.faint.Render("the agent has it — it cannot be taken back")
	}
	line := bar + mark + "  " + text
	if right != "" {
		line = fitRight(line, right+" ", width)
	} else {
		line = fit(line, width)
	}
	switch {
	case selected:
		line = m.lightUp(line+strings.Repeat(" ", max(0, width-ansi.StringWidth(line))), 0, width)
	case hover.kind == hoverQueue && hover.id == row.item.ID:
		line = m.lightUp(line+strings.Repeat(" ", max(0, width-ansi.StringWidth(line))), 0, width)
	}
	return line
}

// queueTop is the screen row of the queue's rule.
func (m *uiModel) queueTop() int { return m.statusRow() + 1 }

// queueRowAt returns the prompt on a screen row of the queue, and its index
// in the queue's items.
func (m *uiModel) queueRowAt(y int) (cockpit.QueueItem, int, bool) {
	rows := m.queueList()
	start, end := m.queueWindow(rows)
	i := start + y - m.queueTop() - 1
	if y <= m.queueTop() || i >= end || i < start || rows[i].editing {
		return cockpit.QueueItem{}, -1, false
	}
	return rows[i].item, rows[i].index, true
}

// queueCommand handles /queue: the queue takes the keys, and resume, pause
// and clear act on it.
func (m *uiModel) queueCommand(arg string) tea.Cmd {
	q := m.queues[m.sessionID]
	switch arg {
	case "":
		if q == nil || len(q.Items) == 0 {
			return m.notify("the queue is empty — while the agent works, enter queues a message and ctrl+x forces one in", "info")
		}
		m.focusQueue(len(q.Items) - 1)
		return nil
	case "resume", "run":
		return m.resumeQueue()
	case "pause":
		if q == nil || len(q.Items) == 0 {
			return m.notify("the queue is empty", "info")
		}
		q.Paused = true
		return m.notify("the queue is paused — /queue resume runs it again", "info")
	case "clear":
		if q == nil || q.Queued() == 0 {
			return m.notify("the queue is empty", "info")
		}
		n := q.Queued()
		forced := slices.DeleteFunc(slices.Clone(q.Items), func(item cockpit.QueueItem) bool { return !item.Forced })
		q.Clear()
		q.Items = forced
		m.blurQueue()
		m.resize()
		return m.notify("dropped "+plural(n, "queued message", "queued messages"), "info")
	}
	return m.notify("/queue takes resume, pause or clear", "warn")
}
