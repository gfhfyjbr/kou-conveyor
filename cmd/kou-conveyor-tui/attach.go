package main

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/png"
	"os"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/atotto/clipboard"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Images in the composer. ctrl+v pastes an image (see clipboard.go), which
// the composer's text refers to by a label, "[Image 1]" for the first; the
// prompt brings the images whose labels its text still holds. Above the
// composer a strip shows them small, and with the cursor right after a label
// the image shows large over the transcript, while typing goes on as ever;
// backspace there takes the label, and its image, out at once. Images the
// prompt is sent with come back with it: when it is edited, when the runner
// never got it, and when it is taken out of the queue to edit.

// attachment is an image in the composer.
type attachment struct {
	image cockpit.Image // label, media type and bytes, as the runner gets them
	info  cockpit.ImageInfo
	// thumb and large are the image as the terminal shows it, small in the
	// strip and large over the transcript; nil until they are made.
	thumb, large *picture
	gone         bool // disposed of: pictures made late are dropped
}

const (
	// thumbSide and largeSide bound the pixels of the pictures sent to the
	// terminal, and thumbCols the cells of a thumbnail's width.
	thumbSide = 256
	largeSide = 1600
	thumbCols = 16
)

type (
	// imagesMsg brings images read from the clipboard or from files, ready
	// for the composer; or the clipboard's text, for a ctrl+v that found
	// no image.
	imagesMsg struct {
		images []preparedImage
		text   *string
		err    error
		quiet  bool // say nothing when nothing came
	}
	preparedImage struct {
		image        cockpit.Image
		thumb, large *picture
	}
	// picturesMsg brings the pictures of attachments that came back with
	// a prompt.
	picturesMsg struct {
		attachments  []*attachment
		thumb, large []*picture
	}
)

// readClipboard reads the clipboard's text; tests replace it.
var readClipboard = clipboard.ReadAll

// ---------------------------------------------------------------- pasting

// pasteClipboard reads the clipboard for ctrl+v, or for a paste with nothing
// in it. An image goes to the composer; otherwise ctrl+v pastes the text.
func (m *uiModel) pasteClipboard(orText bool) tea.Cmd {
	ctx := m.ctx
	return func() tea.Msg {
		data, err := readClipboardImage(ctx)
		switch {
		case errors.Is(err, errNoClipboardImage) && orText:
			text, err := readClipboard()
			return imagesMsg{text: &text, err: err}
		case errors.Is(err, errNoClipboardImage):
			return imagesMsg{quiet: true}
		case err != nil:
			return imagesMsg{err: err}
		}
		return prepareImages([][]byte{data})
	}
}

// pasteFiles attaches image files, as a paste of their paths asks.
func pasteFiles(paths []string) tea.Cmd {
	return func() tea.Msg {
		var data [][]byte
		for _, path := range paths {
			if info, err := os.Stat(path); err == nil && info.Size() > cockpit.MaxImageSource {
				return imagesMsg{err: fmt.Errorf("%s is too large to attach", path)}
			}
			read, err := os.ReadFile(path)
			if err != nil {
				return imagesMsg{err: err}
			}
			data = append(data, read)
		}
		return prepareImages(data)
	}
}

// prepareImages makes pasted bytes images for the model and pictures for
// the terminal.
func prepareImages(data [][]byte) imagesMsg {
	var msg imagesMsg
	for _, one := range data {
		img, err := cockpit.PrepareImage(one)
		if err != nil {
			msg.err = err
			continue
		}
		thumb, large := makePictures(img)
		msg.images = append(msg.images, preparedImage{img, thumb, large})
	}
	return msg
}

// makePictures renders an image for the terminal: a thumbnail and a large
// picture, PNG both, as the protocol takes them.
func makePictures(img cockpit.Image) (thumb, large *picture) {
	source, _, err := image.Decode(bytes.NewReader(img.Data))
	if err != nil {
		return nil, nil
	}
	small := fitImage(source, thumbSide)
	thumb = &picture{png: encodePNG(small), small: small}
	b := source.Bounds()
	large = &picture{small: small}
	if img.MediaType == "image/png" && max(b.Dx(), b.Dy()) <= largeSide {
		large.png = img.Data
	} else {
		large.png = encodePNG(fitImage(source, largeSide))
	}
	return thumb, large
}

// fitImage scales an image down to at most side pixels a side.
func fitImage(src image.Image, side int) image.Image {
	b := src.Bounds()
	if b.Dx() <= side && b.Dy() <= side {
		return src
	}
	scale := min(float64(side)/float64(b.Dx()), float64(side)/float64(b.Dy()))
	return cockpit.Scale(src, max(1, int(float64(b.Dx())*scale)), max(1, int(float64(b.Dy())*scale)))
}

func encodePNG(img image.Image) []byte {
	var out bytes.Buffer
	if png.Encode(&out, img) != nil {
		return nil
	}
	return out.Bytes()
}

// imagesPasted puts pasted images in the composer at the cursor, each by
// its label.
func (m *uiModel) imagesPasted(msg imagesMsg) tea.Cmd {
	if msg.text != nil {
		if msg.err == nil && *msg.text != "" {
			m.paste(*msg.text)
			return nil
		}
		return m.notify("the clipboard holds no image — ctrl+v pastes a copied image or screenshot", "info")
	}
	if len(msg.images) == 0 {
		if msg.err == nil || msg.quiet {
			return nil
		}
		return m.notify("cannot attach the image: "+msg.err.Error(), "warn")
	}
	var labels []string
	for _, prepared := range msg.images {
		if len(m.shownImages()) >= cockpit.MaxImages {
			return m.notify(fmt.Sprintf("a prompt takes at most %d images", cockpit.MaxImages), "warn")
		}
		prepared.image.Label = cockpit.ImageLabel(cockpit.NextImageNumber(m.input.Value(), m.imageData()))
		m.images = append(m.images, &attachment{
			image: prepared.image, info: prepared.image.Info(), thumb: prepared.thumb, large: prepared.large,
		})
		insert := prepared.image.Label
		if len(labels) > 0 {
			insert = " " + insert
		}
		m.input.InsertString(insert)
		labels = append(labels, prepared.image.Label)
	}
	m.resize()
	what := strings.Join(labels, ", ") + " attached"
	if len(labels) == 1 {
		what += " · " + describe(m.images[len(m.images)-1].info)
	}
	if msg.err != nil {
		// Some of the files were no images a model takes.
		return m.notify(what+" — another could not be: "+msg.err.Error(), "warn")
	}
	return m.notify(what+" — ⌫ right after a label removes it", "info")
}

// describe says what an image is: 1920×1080 PNG · 240 KB.
func describe(info cockpit.ImageInfo) string {
	var parts []string
	if info.Width > 0 {
		parts = append(parts, fmt.Sprintf("%d×%d", info.Width, info.Height))
	}
	if kind := strings.ToUpper(strings.TrimPrefix(info.MediaType, "image/")); kind != "" {
		if len(parts) > 0 {
			parts[0] += " " + kind
		} else {
			parts = append(parts, kind)
		}
	}
	if info.Size > 0 {
		parts = append(parts, size(int64(info.Size)))
	}
	return strings.Join(parts, " · ")
}

// ---------------------------------------------------------------- the composer's images

// imageData is the composer's images as the runner gets them.
func (m *uiModel) imageData() []cockpit.Image {
	images := make([]cockpit.Image, len(m.images))
	for i, a := range m.images {
		images[i] = a.image
	}
	return images
}

// composerImages are the images text brings from the composer.
func (m *uiModel) composerImages(text string) []cockpit.Image {
	return cockpit.Referenced(text, m.imageData())
}

// shownImages returns the composer's images its text refers to.
func (m *uiModel) shownImages() []*attachment {
	value := m.input.Value()
	var shown []*attachment
	for _, a := range m.images {
		if strings.Contains(value, a.image.Label) && !slices.ContainsFunc(shown, func(b *attachment) bool { return b.image.Label == a.image.Label }) {
			shown = append(shown, a)
		}
	}
	return shown
}

// attachImages makes attachments of images that come back to the composer,
// and the command that renders their pictures.
func (m *uiModel) attachImages(images []cockpit.Image) ([]*attachment, tea.Cmd) {
	if len(images) == 0 {
		return nil, nil
	}
	attachments := make([]*attachment, len(images))
	for i, img := range images {
		attachments[i] = &attachment{image: img, info: img.Info()}
	}
	return attachments, func() tea.Msg {
		msg := picturesMsg{attachments: attachments}
		for _, a := range attachments {
			thumb, large := makePictures(a.image)
			msg.thumb, msg.large = append(msg.thumb, thumb), append(msg.large, large)
		}
		return msg
	}
}

// pictured gives attachments the pictures made for them.
func (m *uiModel) pictured(msg picturesMsg) {
	for i, a := range msg.attachments {
		if !a.gone && a.thumb == nil {
			a.thumb, a.large = msg.thumb[i], msg.large[i]
		}
	}
}

// setImages makes images the composer's, as they come back with a prompt.
func (m *uiModel) setImages(images []cockpit.Image) tea.Cmd {
	m.clearImages()
	attachments, cmd := m.attachImages(images)
	m.images = attachments
	return cmd
}

// dispose drops attachments and their pictures.
func (m *uiModel) dispose(attachments []*attachment) {
	for _, a := range attachments {
		a.gone = true
		m.graphics.forget(a.thumb)
		m.graphics.forget(a.large)
	}
}

// clearImages drops the composer's images, once the text that referred to
// them is gone.
func (m *uiModel) clearImages() {
	m.dispose(m.images)
	m.images = nil
}

// attachments are all the attachments the cockpit holds: the composer's,
// and those of the text an edit set aside.
func (m *uiModel) attachments() []*attachment {
	all := slices.Clone(m.images)
	if m.edit != nil {
		all = append(all, m.edit.images...)
	}
	if m.queueEdit != nil {
		all = append(all, m.queueEdit.images...)
	}
	return append(all, m.draftImages...)
}

// picturesLost forgets which pictures the terminal has: they stay on the
// screen that is left, and the one that comes gets them anew.
func (m *uiModel) picturesLost() {
	for _, a := range m.attachments() {
		a.thumb.lost()
		a.large.lost()
	}
}

// leaveScreen deletes the pictures from the screen the terminal leaves, and
// waits for that to reach it before the switch. fullscreen says the screen
// left is the alternate one: the main screen keeps the pictures of calls in
// its scrollback.
func (m *uiModel) leaveScreen(fullscreen bool) {
	for _, a := range m.attachments() {
		m.graphics.forget(a.thumb)
		m.graphics.forget(a.large)
	}
	m.leaveViewed(fullscreen)
	m.graphics.drain(2 * time.Second)
}

// releasePictures deletes the pictures from the terminal as the cockpit
// exits.
func (m *uiModel) releasePictures() {
	for _, a := range m.attachments() {
		m.graphics.forget(a.thumb)
		m.graphics.forget(a.large)
	}
	m.releaseViewed()
	m.graphics.stop(2 * time.Second)
}

// sentKept is how many prompts' images rememberImages keeps: a prompt comes
// back from the runner, or is edited before the session holds it, soon
// after it was sent; later the session has its images.
const sentKept = 8

// rememberImages remembers the images of a prompt this cockpit sent, by
// message ID, for when the prompt comes back to the composer.
func (m *uiModel) rememberImages(messageID string, images []cockpit.Image) {
	if len(images) == 0 {
		return
	}
	if m.sent == nil {
		m.sent = make(map[string][]cockpit.Image)
	}
	m.sent[messageID] = images
	m.sentOrder = append(m.sentOrder, messageID)
	for len(m.sentOrder) > sentKept {
		delete(m.sent, m.sentOrder[0])
		m.sentOrder = m.sentOrder[1:]
	}
}

// promptImages are the images a prompt of the transcript brought: those
// this cockpit sent it with, or those the session keeps.
func (m *uiModel) promptImages(e *cockpit.Entry) []cockpit.Image {
	if e == nil || len(e.Images) == 0 {
		return nil
	}
	messageID := strings.TrimPrefix(e.ID, "input:")
	if images, ok := m.sent[messageID]; ok {
		return images
	}
	images, err := cockpit.PromptImages(m.opt.SessionDir, m.sessionID, messageID)
	if err != nil {
		return nil
	}
	return images
}

// ---------------------------------------------------------------- the cursor

// beforeCursor is the text of the cursor's line before it.
func (m *uiModel) beforeCursor() string {
	lines := strings.Split(m.input.Value(), "\n")
	row := m.input.Line()
	if row < 0 || row >= len(lines) {
		return ""
	}
	info := m.input.LineInfo()
	runes := []rune(lines[row])
	return string(runes[:clamp(info.StartColumn+info.ColumnOffset, 0, len(runes))])
}

// labelAtCursor returns the image whose label ends right at the cursor.
func (m *uiModel) labelAtCursor() *attachment {
	if len(m.images) == 0 {
		return nil
	}
	before := m.beforeCursor()
	for _, a := range m.images {
		if strings.HasSuffix(before, a.image.Label) {
			return a
		}
	}
	return nil
}

// previewing returns the image shown large: the one whose label the cursor
// is right after, while the composer has the keys.
func (m *uiModel) previewing() *attachment {
	if m.picker != nil || m.form != nil || m.queueFocus >= 0 || m.changes.focused || !m.input.Focused() || !m.ready {
		return nil
	}
	return m.labelAtCursor()
}

// eraseLabel takes the label right before the cursor out of the composer at
// once, and its image with it. It reports false where there is none.
func (m *uiModel) eraseLabel() bool {
	a := m.labelAtCursor()
	if a == nil {
		return false
	}
	for range utf8.RuneCountInString(a.image.Label) {
		m.input, _ = m.input.Update(tea.KeyMsg{Type: tea.KeyBackspace})
	}
	m.resize()
	return true
}

// cursorAfter puts the composer's cursor right after the first place its
// text has label, which shows its image large.
func (m *uiModel) cursorAfter(label string) bool {
	for row, line := range strings.Split(m.input.Value(), "\n") {
		if at := strings.Index(line, label); at >= 0 {
			m.blurQueue()
			m.blurChanges()
			for guard := 0; m.input.Line() > row && guard < 100_000; guard++ {
				m.input.CursorUp()
			}
			for guard := 0; m.input.Line() < row && guard < 100_000; guard++ {
				m.input.CursorDown()
			}
			m.input.SetCursor(utf8.RuneCountInString(line[:at+len(label)]))
			return true
		}
	}
	return false
}

// ---------------------------------------------------------------- the strip

// thumbRows is how tall the thumbnails above the composer are; without
// pictures, or on a short screen, the strip is a line of labels.
func (m *uiModel) thumbRows() int {
	switch {
	case m.graphics.mode == graphicsText:
		return 0
	case m.height >= 28:
		return 3
	case m.height >= 18:
		return 2
	}
	return 0
}

// stripRows is how many screen rows the strip of images takes.
func (m *uiModel) stripRows() int {
	if !m.ready || len(m.images) == 0 || len(m.shownImages()) == 0 {
		return 0
	}
	return 1 + m.thumbRows()
}

// stripHit is the columns of an image in the strip, for the pointer.
type stripHit struct {
	label    string
	from, to int
}

// stripView renders the strip as part of the dock's tray: its first row is
// the label of an edge, saying what the images do, and the thumbnails with
// their labels are rows of the box; or the labels alone go on the edge.
func (m *uiModel) stripView(hover hoverTarget) []string {
	rows := m.stripRows()
	m.strip = m.strip[:0]
	if rows == 0 {
		return nil
	}
	st := m.styles
	shown := m.shownImages()
	active := m.previewing()
	label := func(a *attachment) string {
		if a == active || hover.kind == hoverAttachment && hover.id == a.image.Label {
			return st.accentLabel.Render(a.image.Label)
		}
		return st.text.Render(a.image.Label)
	}
	edge := m.edgeInner()
	lead := st.rule2.Render("─ ") + st.label.Render(fmt.Sprintf("IMAGES %d", len(shown))) + " "
	hint := "the cursor right after a label shows it · ⌫ there removes it · ^V adds one"
	if rows == 1 {
		// The labels go on the edge, which starts after the box's corner.
		line, x := lead, margin+1+ansi.StringWidth(lead)
		for i, a := range shown {
			item := st.accent.Render("▣ ") + label(a) + " " + st.faint.Render(describe(a.info))
			w := ansi.StringWidth(item)
			if x+w+3 > margin+1+edge {
				line += st.faint.Render(fmt.Sprintf("+%d more ", len(shown)-i))
				break
			}
			m.strip = append(m.strip, stripHit{a.image.Label, x, x + w})
			line += item + "   "
			x += w + 3
		}
		return []string{fit(line, edge)}
	}
	thumbs := rows - 1
	head := lead + st.ghost.Render("· "+hint) + " "
	if ansi.StringWidth(head) > edge-4 {
		head = lead
	}
	lines := []string{fit(head, edge)}
	inner := m.dockInner()
	body := make([]string, thumbs)
	x := promptWidth
	for i, a := range shown {
		cols, pictureRows := m.graphics.fit(a.info.Width, a.info.Height, thumbCols, thumbs)
		var picture []string
		if a.thumb != nil && cols > 0 {
			m.graphics.place(a.thumb, cols, pictureRows)
			picture = m.graphics.draw(a.thumb, cols, pictureRows)
		}
		if picture == nil {
			cols = 2 * thumbs
		}
		text := []string{label(a), st.faint.Render(describe(a.info))}
		if thumbs >= 3 {
			dims := strings.SplitN(describe(a.info), " · ", 2)
			text = []string{label(a), st.muted.Render(dims[0])}
			if len(dims) > 1 {
				text = append(text, st.faint.Render(dims[1]))
			}
		}
		textWidth := 0
		for _, t := range text {
			textWidth = max(textWidth, ansi.StringWidth(t))
		}
		width := cols + 1 + textWidth
		if x+width > promptWidth+inner {
			body[0] += st.faint.Render(fmt.Sprintf("+%d more", len(shown)-i))
			break
		}
		m.strip = append(m.strip, stripHit{a.image.Label, x, x + width})
		for row := range thumbs {
			cell := strings.Repeat(" ", cols)
			switch {
			case picture == nil && row == 0:
				cell = st.faint.Render(ansi.Truncate("▣ …", cols, ""))
				cell += strings.Repeat(" ", max(0, cols-ansi.StringWidth(cell)))
			case row < len(picture):
				cell = picture[row]
			}
			t := ""
			if row < len(text) {
				t = text[row]
			}
			body[row] += cell + " " + t + strings.Repeat(" ", textWidth-ansi.StringWidth(t)) + "   "
		}
		x += width + 3
	}
	for i := range body {
		lines = append(lines, m.boxRow(fit(body[i], inner)))
	}
	return lines
}

// stripTop is the screen row of the strip's rule.
func (m *uiModel) stripTop() int { return m.statusRow() + 1 + m.queueRows() }

// stripAt returns the label of the image under the pointer in the strip.
func (m *uiModel) stripAt(x, y int) (string, bool) {
	rows := m.stripRows()
	if rows == 0 || y < m.stripTop() || y >= m.stripTop()+rows || rows > 1 && y == m.stripTop() {
		return "", false
	}
	for _, hit := range m.strip {
		if x >= hit.from && x < hit.to {
			return hit.label, true
		}
	}
	return "", false
}

// ---------------------------------------------------------------- the preview

// previewBox renders the image the cursor is on, large, in a box of at most
// width×height cells.
func (m *uiModel) previewBox(a *attachment, width, height int) []string {
	st := m.styles
	if width < 24 || height < 5 {
		return nil
	}
	title := " " + st.accentLabel.Render(a.image.Label) + " " + st.muted.Render(describe(a.info)) + " "
	hint := " ⌫ removes it · the cursor elsewhere closes it "
	var body []string
	cols, rows := m.graphics.fit(a.info.Width, a.info.Height, width-4, height-2)
	if m.graphics.mode != graphicsText && a.large != nil && cols > 0 {
		m.graphics.place(a.large, cols, rows)
		body = m.graphics.draw(a.large, cols, rows)
	}
	inner := cols
	if body == nil {
		switch {
		case m.graphics.mode != graphicsText && a.large == nil && !a.gone && len(a.image.Data) != 0:
			body = []string{st.faint.Render("preparing the picture…")}
		default:
			body = []string{
				st.muted.Render("This terminal shows no pictures here: kitty and Ghostty do,"),
				st.muted.Render("and terminals with true colour in blocks. The model sees it."),
			}
		}
		inner = 0
		for _, line := range body {
			inner = max(inner, ansi.StringWidth(line))
		}
	}
	inner = min(max(inner, ansi.StringWidth(title)+2, ansi.StringWidth(hint)-2, 20), width-4)
	for i, line := range body {
		body[i] = fit(line, inner)
	}
	// A hairline box with the accent at its corners, as the dock's.
	border, corner := st.rule2, st.accent
	top := corner.Render("┌") + border.Render("─") + fit(title, inner) + border.Render(strings.Repeat("─", max(0, inner+1-ansi.StringWidth(fit(title, inner))))) + corner.Render("┐")
	bottomHint := hint
	if ansi.StringWidth(hint) > inner {
		bottomHint = ""
	}
	bottom := corner.Render("└") + border.Render(strings.Repeat("─", max(0, inner+1-ansi.StringWidth(bottomHint)))) + st.faint.Render(bottomHint) + border.Render("─") + corner.Render("┘")
	lines := []string{top}
	for _, line := range body {
		w := ansi.StringWidth(line)
		left := max(0, (inner-w)/2)
		lines = append(lines, border.Render("│")+" "+strings.Repeat(" ", left)+line+"\x1b[m"+strings.Repeat(" ", max(0, inner-w-left))+" "+border.Render("│"))
	}
	return append(lines, bottom)
}

// area is a rectangle of the screen: rows [top, bottom), columns [left, right).
type area struct{ top, left, bottom, right int }

func (a area) contains(x, y int) bool {
	return y >= a.top && y < a.bottom && x >= a.left && x < a.right
}

// inPreview reports whether a cell is under the large picture.
func (m *uiModel) inPreview(x, y int) bool { return m.previewArea.contains(x, y) }

// withPreview lays the large picture of the image the cursor is on over
// lines, the screen rows from top on, which the composer does not share.
func (m *uiModel) withPreview(lines []string, top int) []string {
	m.previewArea = area{}
	a := m.previewing()
	if a == nil {
		return lines
	}
	box := m.previewBox(a, min(m.width-4, 144), len(lines)-2)
	row, left := overlay(lines, box, m.width)
	if row >= 0 {
		m.previewArea = area{top: top + row, left: left, bottom: top + row + len(box), right: left + ansi.StringWidth(box[0])}
	}
	return lines
}

// overlay lays box over lines, centered, and returns the rows and columns
// it covers.
func overlay(lines, box []string, width int) (top, left int) {
	if len(box) == 0 || len(box) > len(lines) {
		return -1, -1
	}
	boxWidth := ansi.StringWidth(box[0])
	top, left = (len(lines)-len(box))/2, max(0, (width-boxWidth)/2)
	for i, row := range box {
		line := lines[top+i]
		if pad := left + boxWidth - ansi.StringWidth(line); pad > 0 {
			line += strings.Repeat(" ", pad)
		}
		lines[top+i] = ansi.Truncate(line, left, "") + "\x1b[m" + row + "\x1b[m" + ansi.TruncateLeft(line, left+boxWidth, "")
	}
	return top, left
}

// ---------------------------------------------------------------- keys

// pasteKey handles a paste: paths of image files attach the files, and a
// paste with nothing in it looks for an image on the clipboard.
func (m *uiModel) pasteKey(text string) tea.Cmd {
	if text == "" {
		return m.pasteClipboard(false)
	}
	if paths := imagePaths(text); len(paths) != 0 {
		return pasteFiles(paths)
	}
	m.paste(text)
	return nil
}
