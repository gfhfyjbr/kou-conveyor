package main

import (
	"cmp"
	"errors"
	"io/fs"
	"strconv"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Editing a prompt rewinds its session: the session goes back to how it was
// before the prompt, and the edited prompt runs from there under the same ID
// (see cockpit.RewindSession). The prompt goes to the composer to be edited;
// what the composer held waits until the edit runs or is cancelled.

type editState struct {
	id     string        // the prompt's entry
	draft  string        // the composer's text before the edit
	images []*attachment // and its images
}

// runBlocked says why no run can start now, or "".
func (m *uiModel) runBlocked() string {
	switch {
	case m.loading:
		return "the session is still loading"
	case m.state != idle:
		return "a run is in progress — esc esc stops it"
	case m.external:
		return "this session is running in another window or terminal"
	}
	return ""
}

// escape handles esc outside of an edit: esc esc stops a run, and between
// runs edits the last prompt. Pressed while a run stops, it opens the prompt
// once the run has stopped. twice is set when both escapes arrived at once.
func (m *uiModel) escape(twice bool) tea.Cmd {
	var action string
	switch {
	case m.state == running:
		action = "stop"
	case m.state == stopping:
		action = "edit-after-stop"
	case m.runBlocked() == "" && m.lastPrompt() != nil:
		action = "edit"
	default:
		return nil
	}
	// The second esc counts for what the first announced: a run that ends in
	// between turns a stop into an edit, which takes another esc esc.
	kind := "edit"
	if action == "stop" {
		kind = "stop"
	}
	if !twice && (m.escKind != kind || time.Since(m.escArmed) >= 1500*time.Millisecond) {
		m.escArmed, m.escKind = time.Now(), kind
		return m.notify(map[string]string{
			"stop":            "press esc again to stop the run",
			"edit":            "press esc again to edit the last prompt",
			"edit-after-stop": "press esc again to edit the prompt once the run stops",
		}[action], "info")
	}
	m.escArmed, m.escKind = time.Time{}, ""
	switch action {
	case "stop":
		return m.stop()
	case "edit":
		return m.editLast()
	}
	m.editAfterStop = true
	return m.notify("the prompt opens for editing once the run has stopped", "info")
}

// lastPrompt returns the newest prompt of the transcript, or nil.
func (m *uiModel) lastPrompt() *cockpit.Entry {
	for i := len(m.tr.Entries) - 1; i >= 0; i-- {
		if e := m.tr.Entries[i]; e.Kind == cockpit.KindUser {
			return e
		}
	}
	return nil
}

// promptNumber is the number shown beside a prompt, or 0.
func (m *uiModel) promptNumber(id string) int {
	n := 0
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser {
			n++
			if e.ID == id {
				return n
			}
		}
	}
	return 0
}

func (m *uiModel) editLast() tea.Cmd {
	e := m.lastPrompt()
	if e == nil {
		return m.notify("there is no prompt to edit yet", "info")
	}
	return m.beginEdit(e.ID)
}

// editPrompt edits prompt n, as numbered beside the prompts, or the last.
func (m *uiModel) editPrompt(arg string) tea.Cmd {
	if arg == "" {
		return m.editLast()
	}
	n, err := strconv.Atoi(strings.TrimPrefix(arg, "#"))
	if err != nil || n < 1 {
		return m.notify("/edit takes a prompt number, as shown beside each prompt", "warn")
	}
	for _, e := range m.tr.Entries {
		if e.Kind != cockpit.KindUser {
			continue
		}
		if n--; n == 0 {
			return m.beginEdit(e.ID)
		}
	}
	return m.notify("there is no prompt "+arg, "warn")
}

func (m *uiModel) beginEdit(id string) tea.Cmd {
	if why := m.runBlocked(); why != "" {
		return m.notify(why, "warn")
	}
	e := m.tr.Entry(id)
	if e == nil || e.Kind != cockpit.KindUser {
		return nil
	}
	if m.edit == nil {
		draft := m.input.Value()
		// A prompt the runner never got came back to the composer; editing
		// it takes it from there.
		if draft == e.Text {
			draft = ""
		}
		m.edit = &editState{draft: draft, images: m.images}
		m.images = nil
	}
	m.edit.id = id
	m.historyPos = -1
	m.input.SetValue(e.Text)
	// The prompt's images come with it.
	cmd := m.setImages(m.promptImages(e))
	m.input.Focus()
	m.note = notice{}
	m.resize()
	m.refreshKeep()
	m.reveal(id)
	return cmd
}

// cancelEdit keeps the prompt as it was and gives the composer back what it
// held before.
func (m *uiModel) cancelEdit() {
	if m.edit == nil {
		return
	}
	m.input.SetValue(m.edit.draft)
	m.clearImages()
	m.images = m.edit.images
	m.edit = nil
	m.resize()
	m.refreshKeep()
}

// reveal scrolls the transcript to show an entry's first line.
func (m *uiModel) reveal(id string) {
	for _, s := range m.spans {
		if s.id != id {
			continue
		}
		if s.start < m.view.YOffset || s.start >= m.view.YOffset+m.view.Height {
			m.view.SetYOffset(max(0, s.start-1))
			m.scrolled()
		}
		return
	}
}

// replaced counts the entries after the prompt being edited: what running
// the edit takes out of the session.
func (m *uiModel) replaced() int {
	for i, e := range m.tr.Entries {
		if e.ID == m.edit.id {
			return len(m.tr.Entries) - i - 1
		}
	}
	return 0
}

// runEdit runs the edited prompt in place of the one being edited.
func (m *uiModel) runEdit(text string) tea.Cmd {
	at := -1
	for i, e := range m.tr.Entries {
		if e.ID == m.edit.id {
			at = i
			break
		}
	}
	if at < 0 {
		m.edit = nil
		return m.notify("the prompt being edited is gone; the text stays in the composer", "warn")
	}
	// The session goes back to before the first of the prompts from here on
	// that the runner has; a prompt it never got exists only on screen.
	rewind := ""
	for _, e := range m.tr.Entries[at:] {
		if e.Kind == cockpit.KindUser && e.State == "" {
			rewind = strings.TrimPrefix(e.ID, "input:")
			break
		}
	}
	dir := m.opt.SessionDir
	var tr *cockpit.Transcript
	var err error
	switch {
	case rewind != "":
		tr, err = cockpit.LoadSessionBefore(dir, m.sessionID, rewind)
	case m.fresh:
		tr = cockpit.NewTranscript()
	default:
		tr, err = cockpit.LoadSession(dir, m.sessionID)
		if errors.Is(err, fs.ErrNotExist) {
			tr, err = cockpit.NewTranscript(), nil
		}
	}
	switch {
	case errors.Is(err, cockpit.ErrPromptGone):
		return m.promptGone()
	case errors.Is(err, fs.ErrNotExist):
		return m.notify("this session was deleted elsewhere — ctrl+n starts a new one, keeping the prompt", "error")
	case err != nil:
		return m.notify("cannot edit the prompt: "+err.Error(), "error")
	}
	// The edited prompt runs again with the model it ran with, unless one was
	// chosen for the session.
	model := cmp.Or(m.chosen[m.sessionID], m.tr.Entries[at].Model)
	return m.start(text, startOptions{fromComposer: true, transcript: tr, rewind: rewind, images: m.composerImages(text), model: model})
}

// promptGone ends an edit whose prompt another window rewound away, and shows
// the session as it is now. The edited text stays in the composer.
func (m *uiModel) promptGone() tea.Cmd {
	if m.edit != nil {
		m.dispose(m.edit.images)
	}
	m.edit = nil
	return tea.Batch(m.load(m.sessionID), m.notify("that prompt changed in another window; your text stays in the composer", "warn"))
}
