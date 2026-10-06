package contextbuilder

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"image"
	"image/png"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func pngURL(t *testing.T, width, height int) string {
	t.Helper()
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, image.NewGray(image.Rect(0, 0, width, height))); err != nil {
		t.Fatal(err)
	}
	return "data:image/png;base64," + base64.StdEncoding.EncodeToString(encoded.Bytes())
}

func TestImagesAreEstimatedByTheirSize(t *testing.T) {
	for _, test := range []struct {
		name string
		url  string
		want int64
	}{
		{"small", pngURL(t, 300, 250), 100},
		{"screenshot", pngURL(t, 1500, 1000), 2000},
		// Scaled to fit 2576 pixels first.
		{"large", pngURL(t, 5152, 2576), 2576 * 1288 / pixelsPerToken},
		{"reference", "https://example.com/a.png", imageTokens},
		{"not an image", "data:image/png;base64," + base64.StdEncoding.EncodeToString([]byte("plain text")), imageTokens},
	} {
		if got := imageTokensOf(test.url); got != test.want {
			t.Errorf("%s: %d tokens, want %d", test.name, got, test.want)
		}
	}
}

func TestBuildLeavesTheOldestImagesOut(t *testing.T) {
	current := NewBuilder().(*builder)
	shot := pngURL(t, 10, 10)
	payload, err := ExternalPayload("look at [Image 1]", []llm.Image{{Label: "[Image 1]", URL: shot}})
	if err != nil {
		t.Fatal(err)
	}
	if err := current.AddExternalInput(inbox.Input{ID: "first", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	current.Commit()
	for index := range maxImages {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(answer("", llm.ToolCall{CallID: id, Name: "ViewImage", Arguments: "{}"}))
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultImage, Value: shot}, {Kind: llm.ToolResultText, Value: "10x10"}}, false)
		current.Commit()
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	// 41 images: the oldest go until 30 are left, the user's first.
	images := 0
	for _, item := range built.Request.Input {
		images += len(imagesOf(item))
	}
	if images != maxImages*3/4 {
		t.Fatalf("%d images left", images)
	}
	if text := messageText(built.Request.Input[1]); text != "look at [Image 1]\n\n[The images of this message were left out of the context to keep the requests within the provider's limits: [Image 1].]" {
		t.Fatalf("message = %q", text)
	}
	var result llm.ToolResult
	for _, item := range built.Request.Input {
		if found, ok := item.Data.(llm.ToolResult); ok && found.CallID == "call-0" {
			result = found
		}
	}
	if len(result.Output) != 2 || result.Output[0].Value != leftOutImage || result.Output[1].Value != "10x10" {
		t.Fatalf("result = %#v", result)
	}
	if last := built.Request.Input[len(built.Request.Input)-1].Data.(llm.ToolResult); last.Output[0].Kind != llm.ToolResultImage {
		t.Fatalf("the latest image went: %#v", last)
	}
	if changes := built.Report.Changes; len(changes) != 1 || !strings.Contains(changes[0].Reason, "11 older images were left out") {
		t.Fatalf("changes = %#v", changes)
	}
	// Under the bounds, the conversation stays as it is.
	if current.boundImages() != 0 {
		t.Fatal("left images out again")
	}
}

func TestBuildBoundsTheBytesOfImages(t *testing.T) {
	current := NewBuilder().(*builder)
	large := "data:image/png;base64," + strings.Repeat("A", 7<<20)
	for index := range 3 {
		id := fmt.Sprintf("call-%d", index)
		current.AddModelResponse(answer("", llm.ToolCall{CallID: id, Name: "ViewImage", Arguments: "{}"}))
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultImage, Value: large}}, false)
		current.Commit()
	}
	// 21 MB: the two oldest go, leaving 7.
	if left := current.boundImages(); left != 2 {
		t.Fatalf("left out %d images", left)
	}
}
