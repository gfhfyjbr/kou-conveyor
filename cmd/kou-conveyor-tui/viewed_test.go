package main

import (
	"bytes"
	"fmt"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/ansi/kitty"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// viewedCall returns the ViewImage call of the transcript.
func viewedCall(t *testing.T, m *uiModel) *cockpit.Entry {
	t.Helper()
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindTool && e.Tool.Name == "ViewImage" {
			return e
		}
	}
	t.Fatal("no ViewImage call")
	return nil
}

// lookAt runs a prompt that has the agent look at a picture, and waits for
// the run and for the picture.
func lookAt(t *testing.T, m *uiModel) *cockpit.Entry {
	t.Helper()
	m.input.SetValue("look at the screen")
	_, cmd := m.Update(key("enter"))
	var call *cockpit.Entry
	drive(t, m, cmd, func() bool {
		for _, e := range m.tr.Entries {
			if e.Kind == cockpit.KindTool && e.Tool.Name == "ViewImage" {
				call = e
			}
		}
		return m.state == idle && call != nil && m.viewedReady(call) && (!m.compact || m.scroll.next == len(m.tr.Entries) && printedAll(m)())
	})
	return call
}

// awaitPictures has the model make the pictures on screen, and waits for
// them.
func awaitPictures(t *testing.T, m *uiModel, done func() bool) {
	t.Helper()
	_, cmd := m.Update(tea.FocusMsg{})
	drive(t, m, cmd, done)
}

// spanOf is where an entry is in the transcript as rendered.
func spanOf(t *testing.T, m *uiModel, id string) entrySpan {
	t.Helper()
	for _, s := range m.spans {
		if s.id == id {
			return s
		}
	}
	t.Fatalf("%s is not rendered", id)
	return entrySpan{}
}

func TestViewedPicturesShowUnderTheirCalls(t *testing.T) {
	m := testModel(t)
	m.graphics = newGraphics(graphicsBlocks, false, &bytes.Buffer{})
	call := lookAt(t, m)
	// A folded call shows the picture small, in half blocks: 480×320 in
	// cells twice as tall as wide is 48×16 cells, halved to fit 8 rows.
	s := spanOf(t, m, call.ID)
	if s.pictures != 8 || s.end-s.start != 9 {
		t.Fatalf("span %+v", s)
	}
	for i := s.start + 1; i < s.end; i++ {
		row := ansi.Strip(m.lines[i])
		if !strings.HasPrefix(row, "      │ "+strings.Repeat("▀", 24)) {
			t.Fatalf("row %d = %q", i, row)
		}
	}
	if last := ansi.Strip(m.lines[s.end-1]); !strings.HasSuffix(last, " 480×320 PNG · "+size(int64(call.Tool.Image.Size))) {
		t.Fatalf("the picture's last row = %q", last)
	}
	shown := strings.Join(screen(t, m), "\n") // fails on a line wider than the terminal
	if !strings.Contains(shown, "VIEWIMAGE") || !strings.Contains(shown, "▀▀▀▀") {
		t.Fatalf("screen:\n%s", shown)
	}
	// Selecting across it copies the text around it, not the picture.
	copied := m.selectedText(&selection{area: inTranscript, anchor: cell{s.start, 0}, head: cell{s.end, 90}, moved: true})
	if !strings.Contains(copied, "VIEWIMAGE") || strings.Contains(copied, "▀") || strings.Contains(copied, "480×320") {
		t.Fatalf("copied %q", copied)
	}
	// A click on the picture opens the call, which shows it large.
	m.view.SetYOffset(s.start)
	m.click(12, transcriptTop+s.end-1-m.view.YOffset)
	if !m.isOpen(call) {
		t.Fatal("the click did not open the call")
	}
	s = spanOf(t, m, call.ID)
	if s.pictures != 18 || !strings.Contains(ansi.Strip(m.lines[s.end-1]), strings.Repeat("▀", 54)) {
		t.Fatalf("open span %+v: %q", s, ansi.Strip(m.lines[s.end-1]))
	}
	// A click on the large picture folds it again.
	m.view.SetYOffset(s.end - 3)
	m.click(12, transcriptTop+s.end-1-m.view.YOffset)
	if m.isOpen(call) || spanOf(t, m, call.ID).pictures != 8 {
		t.Fatal("the click on the picture did not fold the call")
	}
}

func TestViewedPicturesKeepTheirRows(t *testing.T) {
	m := testModel(t)
	m.graphics = newGraphics(graphicsBlocks, false, &bytes.Buffer{})
	call := lookAt(t, m)
	made := spanOf(t, m, call.ID)
	// Before the picture is made, the call keeps the rows it will take.
	delete(m.viewed.images, call.ID)
	clear(m.cache)
	m.refreshKeep()
	waiting := spanOf(t, m, call.ID)
	if waiting.end-waiting.start != made.end-made.start || strings.TrimSpace(ansi.Strip(m.lines[waiting.start+1])) != "│ ⋯" {
		t.Fatalf("waiting: %+v %q, made: %+v", waiting, ansi.Strip(m.lines[waiting.start+1]), made)
	}
	awaitPictures(t, m, func() bool { return m.viewedReady(call) })
	if !strings.Contains(m.lines[waiting.start+1], "▀") {
		t.Fatal("the picture did not come")
	}
}

func TestViewedPicturesInKitty(t *testing.T) {
	m := testModel(t)
	var out bytes.Buffer
	m.graphics = newGraphics(graphicsKitty, false, &out)
	m.graphics.next = 0x123456
	call := lookAt(t, m)
	v := m.viewed.images[call.ID]
	if v.small == nil || v.small.id != 0x123456 || v.small.cols != 24 || v.small.rows != 8 || v.large != nil {
		t.Fatalf("small %+v, large %+v", v.small, v.large)
	}
	// A PNG that fits goes as it is.
	picture, _ := call.Tool.Picture()
	if !bytes.Equal(v.small.png, picture.Data) {
		t.Fatal("the picture was encoded anew")
	}
	for _, want := range []string{"\x1b_Ga=t,f=100,t=d,i=1193046,q=2,m=", "\x1b_Ga=p,U=1,i=1193046,p=1,c=24,r=8,q=2\x1b\\"} {
		if !strings.Contains(out.String(), want) {
			t.Fatalf("the terminal did not get %q", want)
		}
	}
	view := m.View()
	corner := "\x1b[38;2;18;52;86m" + string(kitty.Placeholder) + string(kitty.Diacritic(0)) + string(kitty.Diacritic(0))
	if !strings.Contains(view, corner) || !strings.Contains(view, string(kitty.Placeholder)+string(kitty.Diacritic(7))+string(kitty.Diacritic(23))) {
		t.Fatal("no placeholders for the picture")
	}
	// The pointer over it lights the row up, and leaves the picture as it
	// is: the placeholders keep the image's colour.
	s := spanOf(t, m, call.ID)
	m.view.SetYOffset(s.start)
	m.px, m.py = 20, transcriptTop+s.start+2-m.view.YOffset
	if h := m.hover(); h.kind != hoverFold || h.id != call.ID {
		t.Fatalf("hover = %+v", h)
	}
	row := strings.Split(m.View(), "\n")[m.py]
	if !strings.Contains(row, "\x1b[38;2;18;52;86m") || strings.Count(row, string(kitty.Placeholder)) != 24 {
		t.Fatalf("the lit row = %q", row)
	}
	m.px, m.py = -1, -1
	// Open, the call shows a picture of its own, large.
	out.Reset()
	m.toggleAt(s.start)
	awaitPictures(t, m, func() bool { return v.large != nil })
	if v.large.id != 0x123457 || v.large.cols != 54 || v.large.rows != 18 ||
		!strings.Contains(out.String(), "\x1b_Ga=p,U=1,i=1193047,p=1,c=54,r=18,q=2\x1b\\") {
		t.Fatalf("large %+v, sent %.200q", v.large, out.String())
	}
	// Nothing is sent again while nothing changes.
	out.Reset()
	m.refreshKeep()
	m.View()
	if out.Len() != 0 {
		t.Fatalf("sent again: %q", out.String())
	}
	// Another session lets go of them.
	m.newSession()
	m.Update(tea.FocusMsg{})
	for _, id := range []int{0x123456, 0x123457} {
		if want := fmt.Sprintf("\x1b_Ga=d,d=I,i=%d,q=2\x1b\\", id); !strings.Contains(out.String(), want) {
			t.Fatalf("%x was not deleted: %q", id, out.String())
		}
	}
	if len(m.viewed.images) != 0 {
		t.Fatalf("pictures kept: %d", len(m.viewed.images))
	}
}

func TestViewedPicturesFollowTheScreens(t *testing.T) {
	m := testModel(t)
	var out bytes.Buffer
	m.graphics = newGraphics(graphicsKitty, false, &out)
	call := lookAt(t, m)
	v := m.viewed.images[call.ID]
	first := v.small.id
	// The alternate screen takes its pictures with it; the other gets them
	// anew once it is there.
	out.Reset()
	_, cmd := m.Update(key("ctrl+f"))
	if !m.compact || !strings.Contains(out.String(), fmt.Sprintf("a=d,d=I,i=%d,", first)) {
		t.Fatalf("compact %v, sent %q", m.compact, out.String())
	}
	drive(t, m, cmd, func() bool { return !m.switching && v.small.id != 0 })
	second := v.small.id
	if second == first || !strings.Contains(out.String(), fmt.Sprintf("a=t,f=100,t=d,i=%d,", second)) {
		t.Fatalf("after the switch: %x, sent %.300q", second, out.String())
	}
	// The main screen keeps its own, which the scrollback shows, as it is
	// left.
	out.Reset()
	_, cmd = m.Update(key("ctrl+f"))
	if m.compact || strings.Contains(out.String(), "a=d") {
		t.Fatalf("compact %v, sent %q", m.compact, out.String())
	}
	drive(t, m, cmd, func() bool { return !m.switching && v.small.id != 0 })
	third := v.small.id
	if third == second {
		t.Fatal("the alternate screen did not get the picture anew")
	}
	// Back inline, the call is in the scrollback already, with the picture
	// the main screen kept: nothing is sent for it, nor deleted as the
	// cockpit exits.
	out.Reset()
	drive(t, m, m.setLayout("compact"), func() bool { return !m.switching })
	if !strings.Contains(out.String(), fmt.Sprintf("a=d,d=I,i=%d,", third)) || strings.Contains(out.String(), "a=t") ||
		strings.Contains(out.String(), fmt.Sprintf("i=%d,", second)) || v.small.id != 0 {
		t.Fatalf("back inline: id %x, sent %q", v.small.id, out.String())
	}
	out.Reset()
	m.releasePictures()
	if strings.Contains(out.String(), "a=d") {
		t.Fatalf("the scrollback's picture was deleted: %q", out.String())
	}
}

func TestCompactPrintsThePicturesCallsRead(t *testing.T) {
	m, scrollback := compactModel(t)
	m.graphics = newGraphics(graphicsBlocks, false, &bytes.Buffer{})
	lookAt(t, m)
	got := scrollback()
	at := 0
	for _, want := range []string{"VIEWIMAGE", strings.Repeat("▀", 24), "480×320 PNG", "echo: look at the screen"} {
		i := strings.Index(got[at:], want)
		if i < 0 {
			t.Fatalf("%q is missing or out of order in:\n%s", want, got)
		}
		at += i + len(want)
	}
	if strings.Contains(got, "⋯") {
		t.Fatalf("a call went to the scrollback before its picture:\n%s", got)
	}
}

func TestTextTerminalsDescribeWhatCallsRead(t *testing.T) {
	m := testModel(t)
	call := lookAt(t, m)
	if s := spanOf(t, m, call.ID); s.pictures != 0 || s.end-s.start != 1 || len(m.viewed.images) != 0 {
		t.Fatalf("span %+v, %d pictures", s, len(m.viewed.images))
	}
	m.toggleAt(spanOf(t, m, call.ID).start)
	if shown := strings.Join(screen(t, m), "\n"); !strings.Contains(shown, "480×320 image/png") || strings.Contains(shown, "▀") {
		t.Fatalf("screen:\n%s", shown)
	}
}

func TestScrollbackPicturesKeepTheirCells(t *testing.T) {
	m, _ := compactModel(t)
	var out bytes.Buffer
	m.graphics = newGraphics(graphicsKitty, false, &out)
	call := lookAt(t, m)
	printed := m.viewed.images[call.ID].small
	if printed == nil || printed.id == 0 || !m.scroll.printed[call.ID] {
		t.Fatalf("picture %+v, printed %v", printed, m.scroll.printed[call.ID])
	}
	// A resize draws everything anew but what the scrollback holds: its
	// picture keeps its placement, which the cells printed refer to.
	id, cols, rows := printed.id, printed.cols, printed.rows
	out.Reset()
	for _, size := range [][2]int{{60, 20}, {140, 50}} {
		m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		m.View()
	}
	if strings.Contains(out.String(), fmt.Sprintf("i=%d,", id)) || printed.id != id || printed.cols != cols || printed.rows != rows {
		t.Fatalf("the scrollback's picture changed: %+v, sent %q", printed, out.String())
	}
}
