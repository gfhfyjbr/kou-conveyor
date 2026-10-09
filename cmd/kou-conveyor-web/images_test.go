package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json/v2"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"golang.org/x/image/bmp"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func testPNG(t *testing.T, width, height int) []byte {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, width, height))
	for y := range height {
		for x := range width {
			img.SetNRGBA(x, y, color.NRGBA{uint8(x), uint8(y), 99, 255})
		}
	}
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		t.Fatal(err)
	}
	return out.Bytes()
}

// imagesJSON is the images of a request, as a browser sends them.
func imagesJSON(images ...cockpit.Image) string {
	encoded, _ := json.Marshal(images)
	return string(encoded)
}

// fetch gets a path and returns its status, content type and body.
func (h *harness) fetch(path string) (int, http.Header, []byte) {
	h.Helper()
	res, err := http.Get(h.http.URL + path)
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(res.Body)
	return res.StatusCode, res.Header, body
}

func TestRunsBringImages(t *testing.T) {
	h := newHarness(t)
	shot := testPNG(t, 40, 30)
	var photo bytes.Buffer
	bmp.Encode(&photo, image.NewRGBA(image.Rect(0, 0, 12, 10)))
	message := "4a7c1d52-0b3e-4f5a-9c8d-1e2f3a4b5c6d"
	body := fmt.Sprintf(`{"prompt":"compare [Image 1] with [Image 2]","message_id":%q,"images":%s}`, message, imagesJSON(
		cockpit.Image{Label: "[Image 1]", MediaType: "image/png", Data: shot},
		// A BMP goes to the model as PNG.
		cockpit.Image{Label: "[Image 2]", MediaType: "image/bmp", Data: photo.Bytes()},
		// The text no longer names it: it stays behind.
		cockpit.Image{Label: "[Image 3]", MediaType: "image/png", Data: shot},
	))
	started := h.start(body)
	events, _ := h.stream(started["run_id"].(string), "")
	prompt := events.entries()["input:"+message]
	if prompt == nil || prompt.State != "" || len(prompt.Images) != 2 {
		t.Fatalf("prompt = %+v", prompt)
	}
	if first, second := prompt.Images[0], prompt.Images[1]; first.Label != "[Image 1]" || first.Width != 40 || first.MediaType != "image/png" ||
		second.Label != "[Image 2]" || second.MediaType != "image/png" || second.Width != 12 {
		t.Fatalf("images = %+v", prompt.Images)
	}
	if answer := entryTexts(events); !strings.HasSuffix(answer, "echo: compare [Image 1] with [Image 2] · 2 images") {
		t.Fatalf("the run streamed %q", answer)
	}
	// The browser gets each image's bytes from the session.
	session := started["session_id"].(string)
	status, header, data := h.fetch("/api/sessions/" + session + "/images/" + message + "/0")
	if status != http.StatusOK || header.Get("Content-Type") != "image/png" || !bytes.Equal(data, shot) ||
		!strings.Contains(header.Get("Cache-Control"), "immutable") || header.Get("X-Content-Type-Options") != "nosniff" {
		t.Fatalf("image 0: %d %v (%d bytes)", status, header, len(data))
	}
	if status, _, data := h.fetch("/api/sessions/" + session + "/images/" + message + "/1"); status != http.StatusOK || !bytes.HasPrefix(data, []byte("\x89PNG")) {
		t.Fatalf("image 1: %d", status)
	}
	for _, path := range []string{"/images/" + message + "/2", "/images/not-a-uuid/0", "/images/5b1e2d3c-4f5a-4b6c-8d7e-9f0a1b2c3d4e/0"} {
		if status, _, _ := h.fetch("/api/sessions/" + session + path); status != http.StatusNotFound {
			t.Errorf("%s: %d", path, status)
		}
	}
	// The session, as a browser loads it, describes the images.
	_, view := h.do("GET", "/api/sessions/"+session, "")
	loaded, _ := json.Marshal(view["entries"])
	var entries []cockpit.Entry
	if err := json.Unmarshal(loaded, &entries); err != nil || len(entries) == 0 || len(entries[0].Images) != 2 ||
		entries[0].Images[0] != (cockpit.ImageInfo{Label: "[Image 1]", MediaType: "image/png", Width: 40, Height: 30, Size: len(shot)}) {
		t.Fatalf("entries = %s", loaded)
	}
}

func TestRunsRejectImagesNoModelTakes(t *testing.T) {
	h := newHarness(t)
	for _, test := range []struct {
		image cockpit.Image
		want  string
	}{
		{cockpit.Image{Label: "[Image 1]", MediaType: "text/html", Data: []byte("<b>no</b>")}, "images: [Image 1]: not an image"},
		// Labels reach terminals.
		{cockpit.Image{Label: "\x1b[31mred", MediaType: "image/png", Data: testPNG(t, 2, 2)}, "is not of the form [Image 1]"},
	} {
		request, _ := json.Marshal(map[string]any{"prompt": "look at [Image 1] \x1b[31mred", "images": []cockpit.Image{test.image}})
		res, body := h.do("POST", "/api/runs", string(request))
		if res.StatusCode != http.StatusBadRequest || !strings.Contains(fmt.Sprint(body["error"]), test.want) {
			t.Errorf("%d %v, want %q", res.StatusCode, body, test.want)
		}
	}
}

func TestQueuedPromptsKeepTheirImages(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"hold at the gate"}`)
	session := started["session_id"].(string)
	shot := testPNG(t, 20, 10)
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", fmt.Sprintf(`{"text":"then [Image 1] and [Image 2]","images":%s}`, imagesJSON(
		cockpit.Image{Label: "[Image 1]", Data: shot}, cockpit.Image{Label: "[Image 2]", Data: shot},
	)))
	if res.StatusCode != http.StatusCreated {
		t.Fatalf("enqueue: %d %v", res.StatusCode, body)
	}
	item := body["item"].(map[string]any)
	queue, _ := json.Marshal(body["queue"])
	var described struct {
		Items []struct {
			Images []cockpit.ImageInfo `json:"images"`
		} `json:"items"`
	}
	json.Unmarshal(queue, &described)
	if strings.Contains(string(queue), `"data"`) || len(described.Items) != 1 || len(described.Items[0].Images) != 2 ||
		described.Items[0].Images[1] != (cockpit.ImageInfo{Label: "[Image 2]", MediaType: "image/png", Width: 20, Height: 10, Size: len(shot)}) {
		t.Fatalf("queue = %s", queue)
	}
	path := "/api/sessions/" + session + "/queue/" + item["id"].(string) + "/images/"
	if status, header, data := h.fetch(path + "1"); status != http.StatusOK || header.Get("Content-Type") != "image/png" || !bytes.Equal(data, shot) {
		t.Fatalf("queued image: %d %v", status, header)
	}
	// Edited without a label, the prompt leaves its image behind.
	res, body = h.do("PATCH", "/api/sessions/"+session+"/queue/"+item["id"].(string), `{"text":"then [Image 1] alone"}`)
	queue, _ = json.Marshal(body["queue"])
	if res.StatusCode != http.StatusOK || strings.Contains(string(queue), "[Image 2]") {
		t.Fatalf("edit: %d %s", res.StatusCode, queue)
	}
	if status, _, _ := h.fetch(path + "1"); status != http.StatusNotFound {
		t.Fatalf("the image left behind is served: %d", status)
	}
	os.WriteFile(filepath.Join(h.server.opt.Workspace, "gate"), nil, 0o600)
	first, _ := h.stream(started["run_id"].(string), "")
	second, _ := h.stream(first.last().Next.ID, "")
	prompt := second.entries()["input:"+item["id"].(string)]
	if prompt == nil || len(prompt.Images) != 1 || prompt.Images[0].Width != 20 || entryTexts(second) != "then [Image 1] alone|echo: then [Image 1] alone · 1 images" {
		t.Fatalf("the queued run streamed %q, prompt %+v", entryTexts(second), prompt)
	}
}

func TestForcedPromptsBringTheirImages(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"steer the tests"}`)
	session := started["session_id"].(string)
	h.waitFor(session, func(body map[string]any) bool {
		for _, raw := range body["entries"].([]any) {
			if tool, _ := raw.(map[string]any)["tool"].(map[string]any); tool != nil && tool["state"] == cockpit.ToolRunning {
				return true
			}
		}
		return false
	})
	data := base64.StdEncoding.EncodeToString(testPNG(t, 8, 8))
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"like [Image 1]","force":true,"images":[{"label":"[Image 1]","data":"`+data+`"}]}`)
	if res.StatusCode != http.StatusCreated || body["note"] != "" {
		t.Fatalf("force: %d %v", res.StatusCode, body)
	}
	events, _ := h.stream(started["run_id"].(string), "")
	for _, e := range events.entries() {
		if e.Forced {
			if e.Text != "like [Image 1]" || len(e.Images) != 1 || e.Images[0].Width != 8 {
				t.Fatalf("forced prompt = %+v", e)
			}
			return
		}
	}
	t.Fatalf("no forced prompt: %q", entryTexts(events))
}

func TestToolCallsShowThePictureTheyRead(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"look at the screen"}`)
	session := started["session_id"].(string)
	events, _ := h.stream(started["run_id"].(string), "")
	var call *cockpit.Entry
	for _, e := range events.entries() {
		if e.Kind == cockpit.KindTool && e.Tool.Name == "ViewImage" {
			call = e
		}
	}
	picture := cockpittest.Picture()
	want := cockpit.ImageInfo{Label: "screen.png", MediaType: "image/png", Width: 480, Height: 320, Size: len(picture)}
	if call == nil || call.Tool.Image == nil || *call.Tool.Image != want {
		t.Fatalf("the run streamed the call %+v", call)
	}
	// The browser gets its bytes from the session.
	path := "/api/sessions/" + session + "/tools/" + call.Tool.CallID + "/image"
	status, header, data := h.fetch(path)
	if status != http.StatusOK || header.Get("Content-Type") != "image/png" || !bytes.Equal(data, picture) ||
		!strings.Contains(header.Get("Cache-Control"), "immutable") || header.Get("X-Content-Type-Options") != "nosniff" {
		t.Fatalf("picture: %d %v (%d bytes)", status, header, len(data))
	}
	ws := h.server.workspaces.startup().ID
	if status, _, data := h.fetch("/api/w/" + ws + "/sessions/" + session + "/tools/" + call.Tool.CallID + "/image"); status != http.StatusOK || !bytes.Equal(data, picture) {
		t.Fatalf("picture by workspace: %d", status)
	}
	// The session, as a browser loads it, describes the picture.
	_, _, loaded := h.fetch("/api/sessions/" + session)
	var view struct {
		Entries []cockpit.Entry `json:"entries"`
	}
	json.Unmarshal(loaded, &view)
	found := false
	for _, e := range view.Entries {
		found = found || e.Tool != nil && e.Tool.Image != nil && *e.Tool.Image == want
	}
	if !found || !strings.Contains(string(loaded), fmt.Sprintf(`"image":{"label":"screen.png","media_type":"image/png","width":480,"height":320,"size":%d}`, len(picture))) {
		t.Fatalf("session = %.900s", loaded)
	}
	// Calls that read no picture, and names that are no calls, have none.
	h.stream(h.start(fmt.Sprintf(`{"prompt":"use a tool","session_id":%q}`, session))["run_id"].(string), "")
	for _, call := range []string{"call-1", "call-nobody-made", "%1b%5b31m"} {
		if status, _, _ := h.fetch("/api/sessions/" + session + "/tools/" + call + "/image"); status != http.StatusNotFound {
			t.Errorf("%s: %d", call, status)
		}
	}
	if status, _, _ := h.fetch("/api/sessions/not..valid/tools/" + call.Tool.CallID + "/image"); status != http.StatusNotFound {
		t.Errorf("invalid session: %d", status)
	}
}
