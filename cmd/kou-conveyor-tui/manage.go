package main

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"uuid"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type branchedMsg struct {
	gen    int
	branch cockpit.Branch
	whole  bool // a duplicate rather than a branch before a prompt
	err    error
}

// sessionTitle is the title the open session is shown under.
func (m *uiModel) sessionTitle() string {
	if m.meta.Title != "" {
		return m.meta.Title
	}
	return m.tr.Title()
}

func (m *uiModel) unsaved() tea.Cmd {
	return m.notify("the session is saved with its first run; run a prompt first", "warn")
}

// rename titles the open session. Without a title the command waits in the
// composer with the current one to edit; "-" restores the first prompt.
func (m *uiModel) rename(arg string) tea.Cmd {
	if m.fresh {
		return m.unsaved()
	}
	if arg == "" {
		m.input.SetValue("/rename " + m.sessionTitle())
		m.input.CursorEnd()
		m.resize()
		return m.notify("edit the title and press enter · /rename - restores the first prompt", "info")
	}
	title := arg
	if arg == "-" {
		title = ""
	}
	if err := cockpit.RenameSession(m.opt.SessionDir, m.sessionID, title); err != nil {
		return m.notify("cannot rename: "+err.Error(), "error")
	}
	m.meta = cockpit.LoadMeta(m.opt.SessionDir, m.sessionID)
	return tea.Batch(m.title(), m.notify("title: "+orDefault(m.sessionTitle(), "untitled"), "info"))
}

func (m *uiModel) togglePin() tea.Cmd {
	if m.fresh {
		return m.unsaved()
	}
	if err := cockpit.PinSession(m.opt.SessionDir, m.sessionID, !m.meta.Pinned); err != nil {
		return m.notify("cannot pin: "+err.Error(), "error")
	}
	m.meta.Pinned = !m.meta.Pinned
	if m.meta.Pinned {
		return m.notify("pinned: the session stays at the top of the list", "info")
	}
	return m.notify("unpinned", "info")
}

// confirmDelete asks before deleting a session; keeping it is the default.
func (m *uiModel) confirmDelete(id, title string) tea.Cmd {
	if id == m.sessionID && (m.state != idle || m.external) {
		return m.notify("stop the run before deleting the session", "warn")
	}
	if id == m.sessionID && m.fresh {
		return m.unsaved()
	}
	m.closePicker()
	m.picker = newPicker("confirm", "delete session", "", m.styles)
	m.picker.setItems([]pickerItem{
		{title: "Keep the session", action: func(m *uiModel) tea.Cmd { m.closePicker(); return nil }},
		{
			title: "Delete " + strconv.Quote(orDefault(title, "untitled session")), detail: "cannot be undone",
			action: func(m *uiModel) tea.Cmd {
				m.closePicker()
				return m.deleteSession(id)
			},
		},
	})
	m.picker.hints = []string{"↑↓", "move", "enter", "choose", "esc", "keep"}
	m.picker.fixed = true
	m.input.Blur()
	return nil
}

func (m *uiModel) deleteSession(id string) tea.Cmd {
	if id == m.sessionID && (m.state != idle || m.external) {
		return m.notify("stop the run before deleting the session", "warn")
	}
	if err := cockpit.DeleteSession(m.opt.SessionDir, id); err != nil {
		if errors.Is(err, cockpit.ErrSessionBusy) {
			return m.notify("the session is running in another window or terminal", "error")
		}
		return m.notify("cannot delete: "+err.Error(), "error")
	}
	if id == m.sessionID {
		return tea.Batch(m.newSession(), m.notify("session deleted", "info"))
	}
	return m.notify("session deleted", "info")
}

// fork branches the open session before prompt n, or duplicates it.
func (m *uiModel) fork(arg string) tea.Cmd {
	switch {
	case m.fresh:
		return m.unsaved()
	case m.state != idle || m.external:
		return m.notify("stop the run before branching the session", "warn")
	}
	messageID := ""
	if arg != "" {
		n, err := strconv.Atoi(strings.TrimPrefix(arg, "#"))
		if err != nil || n < 1 {
			return m.notify("/fork takes a prompt number, as shown beside each prompt", "warn")
		}
		for _, e := range m.tr.Entries {
			if e.Kind != cockpit.KindUser {
				continue
			}
			if n--; n == 0 {
				if e.State != "" {
					return m.notify("that prompt never reached the runner", "warn")
				}
				messageID = strings.TrimPrefix(e.ID, "input:")
				break
			}
		}
		if messageID == "" {
			return m.notify("there is no prompt "+arg, "warn")
		}
	}
	gen, ctx, dir, id := m.loadGen, m.ctx, m.opt.SessionDir, m.sessionID
	return func() tea.Msg {
		branch, err := cockpit.BranchSession(ctx, dir, id, messageID)
		return branchedMsg{gen: gen, branch: branch, whole: messageID == "", err: err}
	}
}

func (m *uiModel) branched(msg branchedMsg) tea.Cmd {
	if msg.err != nil {
		return m.notify("cannot branch: "+msg.err.Error(), "error")
	}
	if msg.gen != m.loadGen || m.state != idle || m.loading {
		// The view moved on meanwhile; the branch waits in the list.
		if msg.branch.SessionID == "" {
			return nil
		}
		cmds := []tea.Cmd{m.notify("branch "+shortID(msg.branch.SessionID)+" created · ctrl+s opens it", "info")}
		if m.picker != nil && m.picker.kind == "sessions" {
			cmds = append(cmds, m.relist(msg.branch.SessionID))
		}
		return tea.Batch(cmds...)
	}
	var cmds []tea.Cmd
	if msg.branch.SessionID != "" {
		m.closePicker()
		cmds = append(cmds, m.load(msg.branch.SessionID))
	} else {
		cmds = append(cmds, m.newSession())
	}
	text := "session duplicated"
	if !msg.whole {
		m.input.SetValue(msg.branch.Prompt)
		m.input.CursorEnd()
		m.resize()
		text = "branched · edit the prompt and press enter"
	}
	return tea.Batch(append(cmds, m.notify(text, "info"))...)
}

// export writes the open session as Markdown. The default name never
// replaces an existing file.
func (m *uiModel) export(arg string) tea.Cmd {
	if len(m.tr.Entries) == 0 {
		return m.notify("nothing to export yet", "warn")
	}
	title := m.sessionTitle()
	name := cockpit.ExportName(title, m.sessionID)
	path := arg
	switch {
	case path == "":
		path = filepath.Join(m.opt.Workspace, name)
		for n := 2; ; n++ {
			_, err := os.Stat(path)
			if errors.Is(err, fs.ErrNotExist) {
				break
			}
			if err != nil {
				return m.notify("cannot export: "+err.Error(), "error")
			}
			path = filepath.Join(m.opt.Workspace, fmt.Sprintf("%s-%d.md", strings.TrimSuffix(name, ".md"), n))
		}
	default:
		if strings.HasPrefix(path, "~/") {
			if home, err := os.UserHomeDir(); err == nil {
				path = filepath.Join(home, path[2:])
			}
		}
		if abs, err := filepath.Abs(path); err == nil {
			path = abs
		}
		if info, err := os.Stat(path); err == nil && info.IsDir() {
			path = filepath.Join(path, name)
		}
	}
	doc := cockpit.ExportMarkdown(title, m.sessionID, m.tr, time.Now())
	if err := os.WriteFile(path, []byte(doc), 0o600); err != nil {
		return m.notify("cannot export: "+err.Error(), "error")
	}
	return m.notify("exported to "+path, "info")
}

// continueRun asks the agent to pick up where an interrupted run stopped.
func (m *uiModel) continueRun() tea.Cmd {
	if !m.tr.Interrupted() {
		return m.notify("the last run finished; there is nothing to continue", "info")
	}
	return m.submitText(cockpit.ContinuePrompt, false)
}

// compactContext summarizes the session's conversation in a run of its own.
// The runner replaces the conversation with the summary, so the context it
// took is free again and the next prompt starts from the summary; focus
// tells the summary what matters most.
func (m *uiModel) compactContext(focus string) tea.Cmd {
	switch {
	case m.loading:
		return m.notify("the session is still loading", "warn")
	case m.state != idle:
		return m.notify("a run is in progress — /compact once it has finished", "warn")
	case m.external:
		return m.notify("this session is running in another window or terminal", "warn")
	case m.fresh:
		return m.notify("nothing to compact yet: the session starts with its first prompt", "info")
	case !m.tr.Compactable() && len(m.tr.Entries) > 0:
		// A cleared view knows nothing of the session; the runner does.
		return m.notify("nothing to compact: the agent has not answered since the last compaction", "info")
	}
	job, err := cockpit.Start(m.ctx, m.opt.Options, cockpit.Request{
		SessionID: m.sessionID, Compact: true, Instructions: focus, Model: m.opt.model, Thinking: m.thinking,
	})
	switch {
	case errors.Is(err, cockpit.ErrSessionBusy):
		return m.notify("this session is running in another window or terminal", "error")
	case errors.Is(err, cockpit.ErrSessionGone):
		return m.notify("this session was deleted elsewhere — ctrl+n starts a new one", "error")
	case err != nil:
		return m.notify(err.Error(), "error")
	}
	mark := len(m.tr.Entries)
	m.compaction = &mark
	m.runMessage = "" // a compaction changes no files
	m.tr.Begin(uuid.New().String(), "Compacting context")
	return m.attach(job)
}

// compacted says how a compaction run that finished went: the transcript
// shows its summary, or the runner found nothing to summarize.
func (m *uiModel) compacted(mark int) tea.Cmd {
	for _, e := range m.tr.Entries[min(mark, len(m.tr.Entries)):] {
		if strings.HasPrefix(e.ID, "compaction:") {
			if e.Detail == "" {
				return m.notify(e.Text, "warn")
			}
			read := "click its notice to read the summary"
			if m.compact {
				read = "^F shows the summary"
			}
			return m.notify("context compacted — the next prompt starts from the summary; "+read, "info")
		}
	}
	return m.notify("nothing to compact: the agent has not answered since the last compaction", "info")
}

// sessionsKey handles the actions of the sessions list. It reports false for
// keys the list itself handles.
func (m *uiModel) sessionsKey(msg tea.KeyMsg) (tea.Cmd, bool) {
	p := m.picker
	key := msg.String()
	if p.editing != "" {
		switch key {
		case "enter":
			id, title := p.editing, strings.TrimSpace(p.query.Value())
			p.stopEditing()
			if err := cockpit.RenameSession(m.opt.SessionDir, id, title); err != nil {
				return m.notify("cannot rename: "+err.Error(), "error"), true
			}
			if id == m.sessionID {
				m.meta = cockpit.LoadMeta(m.opt.SessionDir, id)
			}
			return tea.Batch(m.relist(id), m.title()), true
		case "esc":
			p.stopEditing()
			p.refilter()
			return nil, true
		}
		var cmd tea.Cmd
		p.query, cmd = p.query.Update(msg)
		return cmd, true
	}
	armed := p.armed
	if key != "ctrl+d" {
		p.armed = ""
	}
	item := p.selected()
	if item == nil || item.id == "" {
		return nil, false
	}
	switch key {
	case "ctrl+e":
		p.edit(item.id, item.title)
		return nil, true
	case "ctrl+t":
		if err := cockpit.PinSession(m.opt.SessionDir, item.id, !item.pinned); err != nil {
			return m.notify("cannot pin: "+err.Error(), "error"), true
		}
		if item.id == m.sessionID {
			m.meta.Pinned = !item.pinned
		}
		return m.relist(item.id), true
	case "ctrl+b":
		if item.id == m.sessionID && (m.state != idle || m.external) {
			return m.notify("stop the run before duplicating the session", "warn"), true
		}
		gen, ctx, dir, id := m.loadGen, m.ctx, m.opt.SessionDir, item.id
		return func() tea.Msg {
			branch, err := cockpit.BranchSession(ctx, dir, id, "")
			return branchedMsg{gen: gen, branch: branch, whole: true, err: err}
		}, true
	case "ctrl+d":
		if armed == item.id && time.Since(p.armedAt) < 4*time.Second {
			p.armed = ""
			id := item.id
			return tea.Batch(m.deleteSession(id), m.relist("")), true
		}
		if item.id == m.sessionID && (m.state != idle || m.external) {
			return m.notify("stop the run before deleting the session", "warn"), true
		}
		p.armed, p.armedAt = item.id, time.Now()
		return nil, true
	}
	return nil, false
}

// relist reloads the sessions list and keeps the cursor on a session.
func (m *uiModel) relist(id string) tea.Cmd {
	m.pickerGen++
	gen, dir := m.pickerGen, m.opt.SessionDir
	return func() tea.Msg {
		list, err := cockpit.ListSessions(dir)
		return sessionsMsg{gen: gen, list: list, err: err, keep: id}
	}
}
