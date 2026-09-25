package main

import (
	"bytes"
	"image"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Pictures the agent looked at. A ViewImage call shows the picture it read
// under its line, as the model got it: small while the call is folded, large
// once it is open, and a click on either folds or unfolds the call. The rows
// a picture takes are known from the transcript before the picture is made,
// so nothing moves when it comes. Pictures are made in the background, a few
// at a time, for the calls in view and a screen above and below them; in
// compact mode for the calls on their way to the scrollback, which wait for
// theirs. Without pictures, the call's output describes the image.

const (
	// viewedCols and viewedRows bound a folded call's picture, in cells; an
	// open call's takes the transcript's width and up to viewedTall rows,
	// fewer on a short screen.
	viewedCols = 64
	viewedRows = 8
	viewedTall = 30
	// viewedSide bounds the pixels of a folded call's picture as the
	// terminal gets it; an open call's gets up to largeSide.
	viewedSide = 640
	// viewedJobs is how many pictures are made at once.
	viewedJobs = 2
)

// viewedImage is the picture a call read, as the terminal shows it.
type viewedImage struct {
	// small is the folded call's picture, and large the open call's in the
	// kitty protocol (half blocks draw small at any size); nil until made.
	small, large *picture
	making       [2]bool // being made: small, large
	failed       bool    // the image could not be read
}

// viewedPictures are the pictures of the calls of the session on screen.
type viewedPictures struct {
	session string                  // the session they are of
	images  map[string]*viewedImage // by the call's entry
	jobs    int                     // pictures being made
	gen     int                     // pictures made for another session are dropped
	// lost is set when the terminal left the screen that had the pictures;
	// the transcript is drawn anew, and sends them again, once it is on
	// the other.
	lost bool
	// spans are the entries with pictures, and lines the rows the
	// pictures take, of the transcript as last rendered.
	spans []entrySpan
	lines map[int]bool
}

// viewedMsg brings a picture made for a call.
type viewedMsg struct {
	gen   int
	id    string // the call's entry
	large bool
	pic   *picture // nil when the image could not be read
}

// showsPicture reports a call that read a picture the terminal can show.
func (m *uiModel) showsPicture(e *cockpit.Entry) bool {
	return m.graphics.mode != graphicsText && e.Kind == cockpit.KindTool && e.Tool != nil && e.Tool.Image != nil
}

// viewedOf returns the picture of a call, making room for it.
func (m *uiModel) viewedOf(id string) *viewedImage {
	if m.viewed.images == nil {
		m.viewed.images = make(map[string]*viewedImage)
	}
	v := m.viewed.images[id]
	if v == nil {
		v = &viewedImage{}
		m.viewed.images[id] = v
	}
	return v
}

// viewedLarge reports whether an open call shows its large picture.
func (m *uiModel) viewedLarge(open bool) bool { return open && m.graphics.mode == graphicsKitty }

// shown is the picture a call shows: the one made for how it is, or the
// other while that one is made.
func (v *viewedImage) shown(large bool) *picture {
	if large && v.large != nil || v.small == nil {
		return v.large
	}
	return v.small
}

// viewedLimit is how many rows a call's picture may take, or 0 for a call
// without one.
func (m *uiModel) viewedLimit(e *cockpit.Entry, open bool) int {
	switch {
	case !m.showsPicture(e):
		return 0
	case open:
		return clamp(m.height*3/5, viewedRows, viewedTall)
	}
	return viewedRows
}

// viewedRowsOf renders the picture of a call, which ends its block: rows of
// the rail with the picture beside it, or with room kept while it is made.
func (m *uiModel) viewedRowsOf(e *cockpit.Entry, width int, open bool) []string {
	if !m.showsPicture(e) {
		return nil
	}
	st := m.styles
	info := *e.Tool.Image
	body := max(10, width-gutter)
	rail := strings.Repeat(" ", gutter-2) + st.rule.Render("│") + " "
	v := m.viewedOf(e.ID)
	if v.failed {
		return []string{rail + fit(st.faint.Render("▣ "+describe(info)+" · the picture cannot be shown here"), body)}
	}
	w, h := info.Width, info.Height
	if w <= 0 || h <= 0 {
		w, h = 4, 3
	}
	cols, rows := min(body, viewedCols), m.viewedLimit(e, open)
	if open {
		cols = body
	}
	cols, rows = m.graphics.fit(w, h, cols, rows)
	// Pictures go to the screen the terminal is on: none while it switches
	// screens, and one in the scrollback keeps the cells it was printed with.
	printed := m.compact && m.scroll.tr == m.tr && m.scroll.printed[e.ID]
	var drawn []string
	if p := v.shown(m.viewedLarge(open)); p != nil && !printed && !m.switching && !m.viewed.lost {
		m.graphics.place(p, cols, rows)
		drawn = m.graphics.draw(p, cols, rows)
	}
	caption := st.faint.Render(describe(info))
	lines := make([]string, rows)
	for i := range rows {
		cell := strings.Repeat(" ", cols)
		switch {
		case i < len(drawn):
			cell = drawn[i]
		case i == 0:
			cell = st.faint.Render("⋯") + strings.Repeat(" ", cols-1)
		}
		lines[i] = rail + cell
		if i == rows-1 && cols+1+ansi.StringWidth(caption) <= body {
			lines[i] += "\x1b[m " + caption
		}
	}
	return lines
}

// notePictures takes down where the transcript as rendered has pictures.
func (m *uiModel) notePictures() {
	m.viewed.spans = m.viewed.spans[:0]
	clear(m.viewed.lines)
	for _, s := range m.spans {
		if s.pictures == 0 {
			continue
		}
		if m.viewed.lines == nil {
			m.viewed.lines = make(map[int]bool)
		}
		m.viewed.spans = append(m.viewed.spans, s)
		for line := s.end - s.pictures; line < s.end; line++ {
			m.viewed.lines[line] = true
		}
	}
}

// pictureLine reports a row of the transcript that is a picture's: it is
// neither selected nor copied.
func (m *uiModel) pictureLine(line int) bool { return m.viewed.lines[line] }

// viewedReady reports a call whose picture is made, or cannot be: in
// compact mode it waits for it before it goes to the scrollback.
func (m *uiModel) viewedReady(e *cockpit.Entry) bool {
	if !m.showsPicture(e) {
		return true
	}
	v := m.viewed.images[e.ID]
	if v == nil {
		return false
	}
	if m.viewedLarge(expandable(e) && m.isOpen(e)) {
		return v.failed || v.large != nil
	}
	return v.failed || v.small != nil
}

// ---------------------------------------------------------------- making them

// viewedUpdate follows the pictures after each update: those a screen switch
// or a suspension left behind are sent again once the terminal is back,
// and those about to show are made.
func (m *uiModel) viewedUpdate(msg tea.Msg) tea.Cmd {
	if _, ok := msg.(tea.ResumeMsg); ok && !m.compact {
		// Suspended, the terminal left the alternate screen and its pictures.
		m.leaveViewed(true)
	}
	if m.viewed.lost && !m.switching {
		m.viewed.lost = false
		for id := range m.viewed.images {
			delete(m.cache, id) // drawn without their pictures meanwhile
		}
		m.refreshKeep()
	}
	return m.wantPictures()
}

// wantPictures starts making the pictures that are about to show.
func (m *uiModel) wantPictures() tea.Cmd {
	if m.graphics.mode == graphicsText || !m.ready {
		return nil
	}
	m.pruneViewed()
	var cmds []tea.Cmd
	want := func(id string) bool {
		if e := m.tr.Entry(id); e != nil {
			if cmd := m.makeViewed(e); cmd != nil {
				cmds = append(cmds, cmd)
			}
		}
		return m.viewed.jobs < viewedJobs
	}
	if m.compact {
		for _, e := range m.tr.Entries {
			if m.showsPicture(e) && !(m.scroll.tr == m.tr && m.scroll.printed[e.ID]) && !want(e.ID) {
				break
			}
		}
		return tea.Batch(cmds...)
	}
	// Those in view first, then those a screen above or below.
	top, height := m.view.YOffset, m.view.Height
	for _, near := range []bool{false, true} {
		for _, s := range m.viewed.spans {
			inView := s.end > top && s.start < top+height
			nearby := s.end > top-height && s.start < top+2*height
			if (near && !inView && nearby || !near && inView) && !want(s.id) {
				return tea.Batch(cmds...)
			}
		}
	}
	return tea.Batch(cmds...)
}

// makeViewed starts making the picture a call shows, if it is still to be
// made and there is room for another job.
func (m *uiModel) makeViewed(e *cockpit.Entry) tea.Cmd {
	v := m.viewedOf(e.ID)
	large := m.viewedLarge(expandable(e) && m.isOpen(e))
	slot := 0
	if large {
		slot = 1
	}
	if v.failed || v.making[slot] || large && v.large != nil || !large && v.small != nil || m.viewed.jobs >= viewedJobs {
		return nil
	}
	img, ok := e.Tool.Picture()
	if !ok {
		v.failed = true
		delete(m.cache, e.ID)
		m.refreshKeep()
		return nil
	}
	v.making[slot] = true
	m.viewed.jobs++
	gen, id, mode := m.viewed.gen, e.ID, m.graphics.mode
	return func() (msg tea.Msg) {
		defer func() {
			// An image that trips a decoder is one the terminal cannot show.
			if recover() != nil {
				msg = viewedMsg{gen: gen, id: id, large: large}
			}
		}()
		return viewedMsg{gen: gen, id: id, large: large, pic: viewedPicture(img, large, mode)}
	}
}

// viewedPicture renders an image a call read for the terminal: a PNG of at
// most viewedSide pixels a side, or largeSide for an open call, for the
// kitty protocol; a small copy for half blocks. A PNG that fits goes as it
// is.
func viewedPicture(img cockpit.Image, large bool, mode graphicsMode) *picture {
	side := viewedSide
	if large {
		side = largeSide
	}
	if config, format, err := image.DecodeConfig(bytes.NewReader(img.Data)); err == nil && mode == graphicsKitty &&
		format == "png" && max(config.Width, config.Height) <= side {
		return &picture{png: img.Data}
	}
	source, _, err := image.Decode(bytes.NewReader(img.Data))
	if err != nil {
		return nil
	}
	if mode != graphicsKitty {
		return &picture{small: fitImage(source, thumbSide)}
	}
	encoded := encodePNG(fitImage(source, side))
	if encoded == nil {
		return nil
	}
	return &picture{png: encoded}
}

// viewedMade takes in a picture made for a call, and draws the call anew.
func (m *uiModel) viewedMade(msg viewedMsg) {
	m.viewed.jobs = max(0, m.viewed.jobs-1)
	v := m.viewed.images[msg.id]
	if msg.gen != m.viewed.gen || v == nil {
		return
	}
	switch {
	case msg.pic == nil:
		v.failed = true
	case msg.large:
		v.making[1], v.large = false, msg.pic
	default:
		v.making[0], v.small = false, msg.pic
	}
	delete(m.cache, msg.id)
	m.refreshKeep()
}

// ---------------------------------------------------------------- letting them go

// pruneViewed lets go of the pictures of calls that left the screen: all of
// them for another session, those a rewind took away otherwise.
func (m *uiModel) pruneViewed() {
	if m.viewed.session != m.sessionID {
		for _, v := range m.viewed.images {
			m.forgetViewed(v)
		}
		clear(m.viewed.images)
		m.viewed.session = m.sessionID
		m.viewed.gen++
		return
	}
	for id, v := range m.viewed.images {
		if m.tr.Entry(id) == nil {
			m.forgetViewed(v)
			delete(m.viewed.images, id)
		}
	}
}

// forgetViewed deletes a call's pictures from the terminal; in compact mode
// they stay, as the scrollback shows them.
func (m *uiModel) forgetViewed(v *viewedImage) {
	if !m.compact {
		m.graphics.forget(v.small)
		m.graphics.forget(v.large)
	}
}

// leaveViewed forgets which pictures the terminal has as it leaves a
// screen: the alternate screen's (fullscreen) are deleted, while the main
// screen keeps its own in the scrollback. The screen that comes gets them
// anew.
func (m *uiModel) leaveViewed(fullscreen bool) {
	for id, v := range m.viewed.images {
		for _, p := range []*picture{v.small, v.large} {
			if fullscreen {
				m.graphics.forget(p)
			} else {
				p.lost()
			}
		}
		delete(m.cache, id)
	}
	m.viewed.lost = len(m.viewed.images) != 0
}

// releaseViewed deletes the pictures from the terminal as the cockpit exits,
// unless the scrollback shows them.
func (m *uiModel) releaseViewed() {
	for _, v := range m.viewed.images {
		m.forgetViewed(v)
	}
}
