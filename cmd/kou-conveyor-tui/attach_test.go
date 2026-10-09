package main

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/ansi/kitty"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// testPNG encodes a width×height PNG.
func testPNG(t *testing.T, width, height int) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, width, height))
	for y := range height {
		for x := range width {
			img.SetNRGBA(x, y, color.NRGBA{uint8(x * 7), uint8(y * 5), 180, 255})
		}
	}
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		t.Fatal(err)
	}
	return out.Bytes()
}

// clipboardImage makes the clipboard hold data for the test.
func clipboardImage(t *testing.T, data []byte) {
	t.Helper()
	saved := readClipboardImage
	readClipboardImage = func(context.Context) ([]byte, error) {
		if data == nil {
			return nil, errNoClipboardImage
		}
		return data, nil
	}
	t.Cleanup(func() { readClipboardImage = saved })
}

// pasteImage pastes an image with ctrl+v and waits for it to arrive.
func pasteImage(t *testing.T, m *uiModel, data []byte) {
	t.Helper()
	clipboardImage(t, data)
	before := len(m.images)
	_, cmd := m.Update(tea.KeyMsg{Type: tea.KeyCtrlV})
	drive(t, m, cmd, func() bool { return len(m.images) > before })
}

func frame(t *testing.T, m *uiModel) string { return strings.Join(screen(t, m), "\n") }

func TestCtrlVPastesAnImage(t *testing.T) {
	m := testModel(t)
	typeText(m, "what is on ")
	pasteImage(t, m, testPNG(t, 64, 48))
	if m.input.Value() != "what is on [Image 1]" || len(m.images) != 1 {
		t.Fatalf("composer %q with %d images", m.input.Value(), len(m.images))
	}
	a := m.images[0]
	if a.image.Label != "[Image 1]" || a.image.MediaType != "image/png" || a.info.Width != 64 || a.thumb == nil || a.large == nil {
		t.Fatalf("attachment = %+v", a.info)
	}
	// The cursor is right after the label: the image shows large, over the
	// transcript, and small above the composer.
	shown := frame(t, m)
	if m.previewing() != a || !strings.Contains(shown, "┌─ [Image 1] 64×48 PNG") || !strings.Contains(shown, "⌫ removes it") {
		t.Fatalf("no preview:\n%s", shown)
	}
	if m.stripRows() != 1 || !strings.Contains(shown, "IMAGES 1 ▣ [Image 1] 64×48 PNG") {
		t.Fatalf("no strip:\n%s", shown)
	}
	// Typing goes on, and moves the cursor off the label.
	typeText(m, "?")
	if m.previewing() != nil || strings.Contains(frame(t, m), "⌫ removes it") {
		t.Fatal("the preview stayed with the cursor elsewhere")
	}
	// The prompt runs with its image.
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if m.input.Value() != "" || len(m.images) != 0 {
		t.Fatalf("composer kept %q and %d images", m.input.Value(), len(m.images))
	}
	prompt := m.lastPrompt()
	if prompt == nil || len(prompt.Images) != 1 || prompt.Images[0].Label != "[Image 1]" || prompt.Images[0].Width != 64 {
		t.Fatalf("prompt = %+v", prompt)
	}
	if answer := m.tr.Entries[len(m.tr.Entries)-1].Text; answer != "echo: what is on [Image 1]? · 1 images" {
		t.Fatalf("answer = %q", answer)
	}
	sent, err := cockpit.PromptImages(m.opt.SessionDir, m.sessionID, strings.TrimPrefix(prompt.ID, "input:"))
	if err != nil || len(sent) != 1 || !bytes.Equal(sent[0].Data, a.image.Data) {
		t.Fatalf("the session holds %d images, %v", len(sent), err)
	}
	// The transcript names the image under the prompt.
	if shown := frame(t, m); !strings.Contains(shown, "▣ [Image 1] 64×48 PNG") {
		t.Fatalf("the prompt does not show its image:\n%s", shown)
	}
}

func TestCtrlVWithoutAnImagePastesText(t *testing.T) {
	m := testModel(t)
	clipboardImage(t, nil)
	saved := readClipboard
	readClipboard = func() (string, error) { return "copied text", nil }
	t.Cleanup(func() { readClipboard = saved })
	_, cmd := m.Update(tea.KeyMsg{Type: tea.KeyCtrlV})
	drive(t, m, cmd, func() bool { return m.input.Value() != "" })
	if m.input.Value() != "copied text" || len(m.images) != 0 {
		t.Fatalf("composer %q with %d images", m.input.Value(), len(m.images))
	}
	// An empty paste, as ⌘V gives for an image in some terminals, looks for
	// an image and otherwise does nothing.
	m.input.Reset()
	_, cmd = m.Update(tea.KeyMsg{Type: tea.KeyRunes, Paste: true})
	msg, ok := cmd().(imagesMsg)
	if !ok || !msg.quiet || m.imagesPasted(msg) != nil || m.input.Value() != "" {
		t.Fatalf("an empty paste gave %#v", msg)
	}
}

func TestBackspaceTakesAnImageOutAtOnce(t *testing.T) {
	m := testModel(t)
	typeText(m, "a ")
	pasteImage(t, m, testPNG(t, 8, 8))
	pasteImage(t, m, testPNG(t, 9, 9))
	if m.input.Value() != "a [Image 1][Image 2]" {
		t.Fatalf("composer = %q", m.input.Value())
	}
	m.Update(key("backspace"))
	if m.input.Value() != "a [Image 1]" || len(m.shownImages()) != 1 || m.previewing() != m.images[0] {
		t.Fatalf("after ⌫: %q, %d shown", m.input.Value(), len(m.shownImages()))
	}
	// A label typed again brings its image back; the next pasted one gets a
	// number of its own.
	pasteImage(t, m, testPNG(t, 10, 10))
	if m.input.Value() != "a [Image 1][Image 3]" {
		t.Fatalf("composer = %q", m.input.Value())
	}
	m.Update(key("left"))
	m.Update(key("backspace"))
	if m.input.Value() != "a [Image 1][Image ]" {
		t.Fatalf("⌫ inside a label erased %q", m.input.Value())
	}
	// ctrl+c clears the composer and its images.
	m.Update(key("ctrl+c"))
	if m.input.Value() != "" || len(m.images) != 0 {
		t.Fatalf("ctrl+c left %q and %d images", m.input.Value(), len(m.images))
	}
}

func TestPastingPathsOfImagesAttachesThem(t *testing.T) {
	dir := t.TempDir()
	first, second := filepath.Join(dir, "Screen Shot 1.png"), filepath.Join(dir, "b.png")
	os.WriteFile(first, testPNG(t, 30, 20), 0o600)
	os.WriteFile(second, testPNG(t, 20, 30), 0o600)
	m := testModel(t)
	// Dragging files onto a terminal pastes their paths, shell-escaped.
	paste := strings.ReplaceAll(first, " ", `\ `) + " '" + second + "'"
	_, cmd := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Paste: true, Runes: []rune(paste)})
	drive(t, m, cmd, func() bool { return len(m.images) == 2 })
	if m.input.Value() != "[Image 1] [Image 2]" || m.images[1].info.Height != 30 {
		t.Fatalf("composer = %q", m.input.Value())
	}
	// Anything else is text.
	m.input.Reset()
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Paste: true, Runes: []rune(first + " and more")})
	if !strings.HasSuffix(m.input.Value(), " and more") {
		t.Fatalf("composer = %q", m.input.Value())
	}
	if got := imagePaths("file://" + strings.ReplaceAll(first, " ", "%20")); !reflect.DeepEqual(got, []string{first}) {
		t.Fatalf("file URL = %q", got)
	}
	if imagePaths(filepath.Join(dir, "missing.png")) != nil || imagePaths("relative.png") != nil {
		t.Fatal("paths of no image were taken")
	}
}

func TestKittyGraphicsDrawThePictures(t *testing.T) {
	m := testModel(t)
	var out bytes.Buffer
	m.graphics = newGraphics(graphicsKitty, false, &out)
	m.graphics.next = 0x123456
	pasteImage(t, m, testPNG(t, 200, 100))
	shown := strings.Join(screen(t, m), "\n")
	sent := out.String()
	thumb, large := m.images[0].thumb, m.images[0].large
	if min(thumb.id, large.id) != 0x123456 || max(thumb.id, large.id) != 0x123457 {
		t.Fatalf("image IDs %x and %x", thumb.id, large.id)
	}
	// A 2:1 picture, in cells twice as tall as wide, is 20×5 cells: the
	// thumbnail fits three rows, and the preview is twice its size.
	if thumb.cols != 12 || thumb.rows != 3 || large.cols != 40 || large.rows != 10 {
		t.Fatalf("thumbnail %d×%d, large %d×%d", thumb.cols, thumb.rows, large.cols, large.rows)
	}
	for _, p := range []*picture{thumb, large} {
		for _, want := range []string{
			fmt.Sprintf("\x1b_Ga=t,f=100,t=d,i=%d,q=2,m=0;", p.id),
			fmt.Sprintf("\x1b_Ga=p,U=1,i=%d,p=1,c=%d,r=%d,q=2\x1b\\", p.id, p.cols, p.rows),
		} {
			if !strings.Contains(sent, want) {
				t.Fatalf("the terminal did not get %q:\n%.300q", want, sent)
			}
		}
	}
	// The screen holds placeholder cells in the images' colours: row and
	// column diacritics after each.
	view := m.View()
	cell := string(kitty.Placeholder) + string(kitty.Diacritic(2)) + string(kitty.Diacritic(11))
	if !strings.Contains(view, cell) {
		t.Fatal("no placeholder for the thumbnail's last cell")
	}
	for _, want := range []string{"\x1b[38;2;18;52;86m" + string(kitty.Placeholder), "\x1b[38;2;18;52;87m" + string(kitty.Placeholder)} {
		if !strings.Contains(view, want) {
			t.Fatalf("no placeholders %q in the view", want)
		}
	}
	if !strings.Contains(shown, "[Image 1]") {
		t.Fatal("no label")
	}
	// Nothing is sent again while nothing changes.
	out.Reset()
	m.View()
	if out.Len() != 0 {
		t.Fatalf("sent again: %q", out.String())
	}
	// Clearing the composer deletes the pictures from the terminal.
	m.Update(key("ctrl+c"))
	if !strings.Contains(out.String(), "\x1b_Ga=d,d=I,i=1193046,q=2\x1b\\") || !strings.Contains(out.String(), "\x1b_Ga=d,d=I,i=1193047,q=2\x1b\\") {
		t.Fatalf("pictures were not deleted: %q", out.String())
	}
}

func TestHalfBlocksDrawThePictures(t *testing.T) {
	m := testModel(t)
	m.graphics = newGraphics(graphicsBlocks, false, &bytes.Buffer{})
	pasteImage(t, m, testPNG(t, 40, 40))
	view := m.View()
	if !strings.Contains(view, "▀") || m.stripRows() != 4 {
		t.Fatalf("no half blocks; the strip takes %d rows", m.stripRows())
	}
	for i, line := range strings.Split(view, "\n") {
		if w := ansi.StringWidth(line); w > m.width {
			t.Fatalf("line %d is %d wide", i, w)
		}
	}
	if len(strings.Split(view, "\n")) != m.height {
		t.Fatal("the frame does not fit")
	}
}

func TestClickingAThumbnailShowsItLarge(t *testing.T) {
	m := testModel(t)
	pasteImage(t, m, testPNG(t, 12, 12))
	typeText(m, " and more")
	m.View()
	if len(m.strip) != 1 || m.previewing() != nil {
		t.Fatalf("strip %v", m.strip)
	}
	x, y := m.strip[0].from, m.stripTop()
	m.px, m.py = x, y
	if h := m.hover(); h.kind != hoverAttachment || h.id != "[Image 1]" {
		t.Fatalf("hover = %+v", h)
	}
	click(m, x, y)
	if m.previewing() != m.images[0] {
		t.Fatalf("the click left the cursor at %q", m.beforeCursor())
	}
}

func TestAnEditedPromptBringsItsImages(t *testing.T) {
	m := testModel(t)
	data := testPNG(t, 16, 12)
	typeText(m, "fix ")
	pasteImage(t, m, data)
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	// A draft with an image of its own waits for the edit to end.
	typeText(m, "later ")
	pasteImage(t, m, testPNG(t, 5, 5))
	draft := m.images
	cmd = m.editLast()
	if m.input.Value() != "fix [Image 1]" || len(m.images) != 1 || !bytes.Equal(m.images[0].image.Data, data) {
		t.Fatalf("the edit holds %q and %d images", m.input.Value(), len(m.images))
	}
	drive(t, m, cmd, func() bool { return m.images[0].thumb != nil })
	m.cancelEdit()
	if m.input.Value() != "later [Image 1]" || !reflect.DeepEqual(m.images, draft) {
		t.Fatalf("after the edit: %q", m.input.Value())
	}
	// Run from the edit, the prompt keeps its image.
	m.editLast()
	typeText(m, " again")
	_, cmd = m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
	if last := m.lastPrompt(); last.Text != "fix [Image 1] again" || len(last.Images) != 1 {
		t.Fatalf("edited prompt = %+v", last)
	}
	if m.input.Value() != "later [Image 1]" || len(m.images) != 1 {
		t.Fatalf("the draft came back as %q with %d images", m.input.Value(), len(m.images))
	}
}

func TestQueuedAndForcedPromptsBringTheirImages(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("steer")
	_, run := m.Update(key("enter"))
	// Forced in while the agent works.
	typeText(m, "and ")
	pasteImage(t, m, testPNG(t, 6, 6))
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlX})
	if len(m.images) != 0 {
		t.Fatal("the forced prompt's image stayed in the composer")
	}
	// Queued for after the run.
	typeText(m, "then ")
	pasteImage(t, m, testPNG(t, 7, 7))
	m.Update(key("enter"))
	q := m.queue()
	if len(q.Items) != 2 || len(q.Items[1].Images) != 1 || q.Items[1].Images[0].Label != "[Image 1]" {
		t.Fatalf("queue = %+v", q.Items)
	}
	drive(t, m, run, func() bool {
		last := m.lastPrompt()
		return m.state == idle && last != nil && last.Text == "then [Image 1]" && last.State == ""
	})
	var forced *cockpit.Entry
	for _, e := range m.tr.Entries {
		if e.Kind == cockpit.KindUser && e.Forced {
			forced = e
		}
	}
	if forced == nil || forced.Text != "and [Image 1]" || len(forced.Images) != 1 || forced.Images[0].Width != 6 {
		t.Fatalf("forced prompt = %+v", forced)
	}
	if last := m.lastPrompt(); len(last.Images) != 1 || last.Images[0].Width != 7 {
		t.Fatalf("queued prompt = %+v", last)
	}
}

func TestInlineShowsThePreviewAboveTheComposer(t *testing.T) {
	m, _ := compactModel(t)
	pasteImage(t, m, testPNG(t, 20, 10))
	view := ansi.Strip(m.View())
	if !strings.Contains(view, "┌─ [Image 1] 20×10 PNG") || !strings.Contains(view, "IMAGES 1") {
		t.Fatalf("inline:\n%s", view)
	}
	if n := len(strings.Split(m.View(), "\n")); n > m.height {
		t.Fatalf("%d lines for %d rows", n, m.height)
	}
}

func TestPastedPathsSplitLikeAShell(t *testing.T) {
	for text, want := range map[string][]string{
		`/a/b.png`:               {"/a/b.png"},
		`/a/Screen\ Shot.png /c`: {"/a/Screen Shot.png", "/c"},
		`'/a b.png' "/c d.png"`:  {"/a b.png", "/c d.png"},
		"  /a.png\n":             {"/a.png"},
		`it's text`:              {"its text"},
	} {
		if got := shellWords(text); !reflect.DeepEqual(got, want) {
			t.Errorf("%q → %q, want %q", text, got, want)
		}
	}
}

func TestSwitchingScreensSendsThePicturesAgain(t *testing.T) {
	m := testModel(t)
	var out bytes.Buffer
	m.graphics = newGraphics(graphicsKitty, false, &out)
	pasteImage(t, m, testPNG(t, 60, 30))
	m.View()
	thumb, large := m.images[0].thumb, m.images[0].large
	before := []uint32{thumb.id, large.id}
	if before[0] == 0 || before[1] == 0 {
		t.Fatalf("not sent: %v", before)
	}
	// Each screen keeps its own images: the one left loses them, the other
	// gets them anew.
	out.Reset()
	m.setLayout("compact")
	for _, id := range before {
		if want := fmt.Sprintf("\x1b_Ga=d,d=I,i=%d,q=2\x1b\\", id); !strings.Contains(out.String(), want) {
			t.Fatalf("%x was not deleted: %q", id, out.String())
		}
	}
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	out.Reset()
	m.View()
	if thumb.id == 0 || large.id == 0 || thumb.id == before[0] || large.id == before[1] ||
		!strings.Contains(out.String(), fmt.Sprintf("a=t,f=100,t=d,i=%d,", thumb.id)) || !strings.Contains(out.String(), fmt.Sprintf("a=t,f=100,t=d,i=%d,", large.id)) {
		t.Fatalf("after the switch: thumb %x, large %x, sent %.200q", thumb.id, large.id, out.String())
	}
}

func TestImagesFitNarrowTerminals(t *testing.T) {
	for _, mode := range []graphicsMode{graphicsText, graphicsBlocks, graphicsKitty} {
		m := testModel(t)
		m.graphics = newGraphics(mode, false, &bytes.Buffer{})
		for range 6 {
			pasteImage(t, m, testPNG(t, 300, 20))
		}
		for _, size := range [][2]int{{40, 30}, {60, 16}, {100, 40}} {
			m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
			screen(t, m) // fails on a line wider than the terminal
		}
	}
}
