package cockpit_test

import (
	"bytes"
	"encoding/json/v2"
	"errors"
	"image"
	"image/color"
	"image/png"
	"io/fs"
	"math/rand/v2"
	"reflect"
	"strings"
	"testing"
	"time"
	"uuid"

	"golang.org/x/image/bmp"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

// pngImage encodes a width×height PNG; noisy ones do not compress.
func pngImage(t testing.TB, width, height int, noisy bool) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, width, height))
	random := rand.New(rand.NewPCG(1, 2))
	for y := range height {
		for x := range width {
			c := color.NRGBA{uint8(x), uint8(y), 200, 255}
			if noisy {
				c = color.NRGBA{uint8(random.Uint32()), uint8(random.Uint32()), uint8(random.Uint32()), 255}
			}
			img.SetNRGBA(x, y, c)
		}
	}
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		t.Fatal(err)
	}
	return out.Bytes()
}

func TestPrepareImageKeepsWhatAModelTakes(t *testing.T) {
	small := pngImage(t, 40, 30, false)
	img, err := cockpit.PrepareImage(small)
	if err != nil || img.MediaType != "image/png" || !bytes.Equal(img.Data, small) {
		t.Fatalf("a small PNG became %s (%d bytes), %v", img.MediaType, len(img.Data), err)
	}
	info := img.Info()
	if info.Width != 40 || info.Height != 30 || info.Size != len(small) {
		t.Fatalf("info = %+v", info)
	}
}

func TestPrepareImageConvertsAndScalesDown(t *testing.T) {
	// BMP is read, and goes to the model as PNG.
	var encoded bytes.Buffer
	if err := bmp.Encode(&encoded, image.NewRGBA(image.Rect(0, 0, 12, 8))); err != nil {
		t.Fatal(err)
	}
	img, err := cockpit.PrepareImage(encoded.Bytes())
	if err != nil || img.MediaType != "image/png" || img.Info().Width != 12 {
		t.Fatalf("a BMP became %+v, %v", img.Info(), err)
	}
	// A screenshot larger than a model takes is scaled down to fit.
	img, err = cockpit.PrepareImage(pngImage(t, 3000, 1200, false))
	if info := img.Info(); err != nil || info.Width != cockpit.MaxImageSide || info.Height != 800 {
		t.Fatalf("a large image became %+v, %v", info, err)
	}
	// One that does not compress goes as JPEG, small enough to send.
	img, err = cockpit.PrepareImage(pngImage(t, 1900, 1900, true))
	if info := img.Info(); err != nil || info.MediaType != "image/jpeg" || info.Size > 3_700_000 {
		t.Fatalf("a noisy image became %+v, %v", info, err)
	}
	for _, bad := range [][]byte{nil, []byte("not an image"), []byte("<svg></svg>")} {
		if _, err := cockpit.PrepareImage(bad); err == nil {
			t.Errorf("%q was taken for an image", bad)
		}
	}
}

func TestImageLabels(t *testing.T) {
	images := []cockpit.Image{{Label: "[Image 1]"}, {Label: "[Image 2]"}, {Label: "[Image 4]"}}
	if n := cockpit.NextImageNumber("", images); n != 5 {
		t.Fatalf("next = %d", n)
	}
	if n := cockpit.NextImageNumber("typed [Image 9] by hand", nil); n != 10 {
		t.Fatalf("next after a typed label = %d", n)
	}
	if n := cockpit.NextImageNumber("", nil); n != 1 || cockpit.ImageLabel(n) != "[Image 1]" {
		t.Fatalf("first = %d", n)
	}
	kept := cockpit.Referenced("compare [Image 4] with [Image 1], then [Image 4] again", images)
	if len(kept) != 2 || kept[0].Label != "[Image 1]" || kept[1].Label != "[Image 4]" {
		t.Fatalf("referenced = %+v", kept)
	}
	if cockpit.Referenced("[Image 10]", []cockpit.Image{{Label: "[Image 1]"}}) != nil {
		t.Fatal("[Image 10] referred to [Image 1]")
	}
}

func TestPromptWithImagesRoundTrips(t *testing.T) {
	sessions := t.TempDir()
	id, message := uuid.New().String(), uuid.New().String()
	first, second := pngImage(t, 4, 3, false), pngImage(t, 3, 2, false)
	// The first prompt's line is far longer than a Scanner reads.
	large := pngImage(t, 1200, 1200, true)
	images := []cockpit.Image{
		{Label: "[Image 1]", MediaType: "image/png", Data: first},
		{Label: "[Image 2]", MediaType: "image/png", Data: second},
		{Label: "[Image 3]", MediaType: "image/png", Data: large},
		// No longer in the text, so it stays behind.
		{Label: "[Image 4]", MediaType: "image/png", Data: first},
	}
	job, err := cockpit.Start(t.Context(), cockpit.Options{
		Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions, Heartbeat: time.Minute,
	}, cockpit.Request{SessionID: id, MessageID: message, Prompt: "what is on [Image 1], [Image 2] and [Image 3]?", Images: images})
	if err != nil {
		t.Fatal(err)
	}
	live := cockpit.NewTranscript()
	pending := live.WithImages(live.Submit(message, "what is on [Image 1], [Image 2] and [Image 3]?", time.Now()), images[:3])
	if len(pending.Images) != 3 || pending.Images[0].Width != 4 {
		t.Fatalf("pending images = %+v", pending.Images)
	}
	drain(t, job, live)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	prompt := live.Entry("input:" + message)
	want := []cockpit.ImageInfo{
		{Label: "[Image 1]", MediaType: "image/png", Width: 4, Height: 3, Size: len(first)},
		{Label: "[Image 2]", MediaType: "image/png", Width: 3, Height: 2, Size: len(second)},
		{Label: "[Image 3]", MediaType: "image/png", Width: 1200, Height: 1200, Size: len(large)},
	}
	if prompt == nil || prompt.State != "" || !reflect.DeepEqual(prompt.Images, want) {
		t.Fatalf("live prompt = %+v", prompt)
	}
	if answer := live.Entries[len(live.Entries)-1].Text; answer != "echo: what is on [Image 1], [Image 2] and [Image 3]? · 3 images" {
		t.Fatalf("answer = %q", answer)
	}
	loaded, err := cockpit.LoadSession(sessions, id)
	if err != nil {
		t.Fatal(err)
	}
	if e := loaded.Entry("input:" + message); e == nil || e.Text != prompt.Text || !reflect.DeepEqual(e.Images, want) {
		t.Fatalf("loaded prompt = %+v", e)
	}
	list, err := cockpit.ListSessions(sessions)
	if err != nil || len(list) != 1 || list[0].Title != "what is on [Image 1], [Image 2] and [Image 3]?" {
		t.Fatalf("sessions = %+v, %v", list, err)
	}
	got, err := cockpit.PromptImages(sessions, id, message)
	if err != nil || !reflect.DeepEqual(got, images[:3]) {
		t.Fatalf("prompt images = %d, %v", len(got), err)
	}
	if _, err := cockpit.PromptImages(sessions, id, uuid.New().String()); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("images of a prompt the session lacks: %v", err)
	}
}

func TestSteeredMessagesBringImages(t *testing.T) {
	job := start(t, t.Context(), t.TempDir(), uuid.New().String(), "steer")
	tr := cockpit.NewTranscript()
	message := uuid.New().String()
	sent := false
	for line := range job.Lines() {
		if line.Stderr {
			continue
		}
		tr.Apply([]byte(line.Text))
		if running := tr.Running(); !sent && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
			if err := job.Steer(message, "like [Image 1]", cockpit.Image{Label: "[Image 1]", MediaType: "image/png", Data: pngImage(t, 2, 2, false)}); err != nil {
				t.Fatal(err)
			}
			sent = true
		}
	}
	if e := tr.Entry("input:" + message); e == nil || !e.Forced || len(e.Images) != 1 || e.Images[0].Width != 2 {
		t.Fatalf("forced prompt = %+v", e)
	}
}

func TestQueuedImagesStayOutOfTheJSON(t *testing.T) {
	var q cockpit.Queue
	data := pngImage(t, 5, 4, false)
	item := q.AddImages("fix [Image 1]", "", []cockpit.Image{
		{Label: "[Image 1]", MediaType: "image/png", Data: data},
		{Label: "[Image 2]", MediaType: "image/png", Data: data},
	})
	if len(item.Images) != 1 {
		t.Fatalf("queued images = %d, want the one the text refers to", len(item.Images))
	}
	encoded, err := json.Marshal(q)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(encoded), `"data"`) || !strings.Contains(string(encoded), `"images":[{"label":"[Image 1]","media_type":"image/png","width":5,"height":4,"size":`) ||
		!strings.Contains(string(encoded), `"text":"fix [Image 1]"`) {
		t.Fatalf("queue JSON = %s", encoded)
	}
}
