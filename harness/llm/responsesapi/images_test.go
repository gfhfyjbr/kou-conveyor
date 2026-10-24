package responsesapi

import (
	"bytes"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// gatewayImageRejection is how the gateway's text-only models reject a
// request that carries an image.
const gatewayImageRejection = "messages.content.type is invalid, allowed values: ['text']"

const seenResponse = `{"id":"resp-1","object":"response","status":"completed","output":[` +
	`{"id":"message-1","type":"message","role":"assistant","status":"completed",` +
	`"content":[{"type":"output_text","text":"seen","annotations":[],"logprobs":[]}]}],` +
	`"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}`

// imageRequest has an image in a message and one in a ViewImage result.
func imageRequest() llm.Request {
	return llm.Request{
		Model: llm.Model{ID: "mimo-v2.6-pro"},
		Input: []llm.Item{
			{Type: llm.ItemMessage, Data: llm.Message{
				Role: llm.RoleUser, Text: "what is on [Image #1]?",
				Images: []llm.Image{{Label: "[Image #1]", URL: "data:image/png;base64,AAAA"}},
			}},
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "ViewImage", Arguments: `{"path":"a.png"}`}},
			{Type: llm.ItemToolResult, Data: llm.ToolResult{CallID: "call-1", Output: []llm.ToolResultOutput{
				{Kind: llm.ToolResultText, Value: "a.png"},
				{Kind: llm.ToolResultImage, Value: "data:image/png;base64,BBBB"},
				{Kind: llm.ToolResultText, Value: "800x600"},
			}}},
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: "and now?"}},
		},
	}
}

func TestImagesRejected(t *testing.T) {
	for message, want := range map[string]bool{
		gatewayImageRejection: true,
		"Invalid content type. image_url is only supported by certain models.":       true,
		"This model does not support image input.":                                   true,
		"Invalid value: 'input_image'. Supported values are: 'input_text'.":          true,
		"Image input is not supported for this model":                                true,
		"Invalid value for 'input'.":                                                 false,
		"This model's maximum context length is 128000 tokens.":                      false,
		"Rate limit reached for requests. Please try again in 20ms.":                 false,
		"The image you provided could not be processed: it is not a valid PNG file.": true,
	} {
		if got := imagesRejected(&APIError{Message: message}); got != want {
			t.Errorf("imagesRejected(%q) = %v, want %v", message, got, want)
		}
	}
	if imagesRejected(nil) {
		t.Error("imagesRejected(nil) = true")
	}
}

func TestWithoutImages(t *testing.T) {
	request := imageRequest()
	before := imageRequest()
	stripped := withoutImages(request)

	if !reflect.DeepEqual(request, before) {
		t.Fatalf("the caller's request changed:\n%#v", request)
	}
	if !hasImages(request) || hasImages(stripped) {
		t.Fatalf("hasImages: request %v, stripped %v", hasImages(request), hasImages(stripped))
	}
	message := stripped.Input[0].Data.(llm.Message)
	if message.Images != nil || !strings.HasPrefix(message.Text, "what is on [Image #1]?\n\n[") ||
		!strings.Contains(message.Text, "cannot read images") || !strings.Contains(message.Text, "[Image #1]") {
		t.Fatalf("message = %#v", message)
	}
	result := stripped.Input[2].Data.(llm.ToolResult)
	want := []llm.ToolResultOutput{
		{Kind: llm.ToolResultText, Value: "a.png"},
		{Kind: llm.ToolResultText, Value: imageLeftOut},
		{Kind: llm.ToolResultText, Value: "800x600"},
	}
	if result.CallID != "call-1" || !reflect.DeepEqual(result.Output, want) {
		t.Fatalf("tool result = %#v", result)
	}
	for _, index := range []int{1, 3} {
		if !reflect.DeepEqual(stripped.Input[index], request.Input[index]) {
			t.Fatalf("item %d changed: %#v", index, stripped.Input[index])
		}
	}

	// A request without images is the same request.
	plain := validRequest()
	if same := withoutImages(plain); &same.Input[0] != &plain.Input[0] {
		t.Fatal("a request without images was copied")
	}
}

// imageRejectingServer answers a request to mimo-v2.6-pro that carries an
// image as a text-only model does, in the way given, and any other with
// seenResponse. It keeps the bodies of the requests it answered.
func imageRejectingServer(t *testing.T, way string) (*httptest.Server, *atomic.Int32, func() [][]byte) {
	t.Helper()
	var calls atomic.Int32
	var mu sync.Mutex
	var answered [][]byte
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Errorf("read request: %v", err)
		}
		if !bytes.Contains(body, []byte(`"input_image"`)) || !bytes.Contains(body, []byte(`"mimo-v2.6-pro"`)) {
			mu.Lock()
			answered = append(answered, body)
			mu.Unlock()
			writeStreamResponse(t, w, seenResponse)
			return
		}
		switch way {
		case "http":
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			_, _ = io.WriteString(w, `{"error":{"code":"1210","message":"`+gatewayImageRejection+`"}}`)
		default: // "error" or "response.failed", in the stream
			w.Header().Set("Content-Type", "text/event-stream")
			_, _ = io.WriteString(w, retryErrorEvent(way, "1210", gatewayImageRejection))
		}
	}))
	t.Cleanup(server.Close)
	return server, &calls, func() [][]byte {
		mu.Lock()
		defer mu.Unlock()
		return answered
	}
}

func TestAdapterLeavesOutImagesTheModelRejects(t *testing.T) {
	for _, way := range []string{"http", "error", "response.failed"} {
		t.Run(way, func(t *testing.T) {
			t.Parallel()
			server, calls, answered := imageRejectingServer(t, way)
			var warnings []string
			ctx := llm.WithWarningReporter(t.Context(), func(message string) { warnings = append(warnings, message) })
			adapter := newTestAdapter(t, server.URL)

			response, err := adapter.Respond(ctx, imageRequest(), llm.RequestOptions{})
			if err != nil || response.Failure != nil || len(response.Output) == 0 {
				t.Fatalf("response = %#v, error = %v", response, err)
			}
			// Rejected once, at once, and answered without the images.
			if calls.Load() != 2 {
				t.Fatalf("requests = %d, want 2", calls.Load())
			}
			if len(warnings) != 1 || !strings.Contains(warnings[0], "mimo-v2.6-pro does not accept images") ||
				!strings.Contains(warnings[0], gatewayImageRejection) {
				t.Fatalf("warnings = %q", warnings)
			}
			if bodies := answered(); len(bodies) != 1 || !bytes.Contains(bodies[0], []byte("cannot read images")) ||
				bytes.Contains(bodies[0], []byte("base64")) {
				t.Fatalf("answered requests = %q", bodies)
			}

			// The run's later requests leave the images out from the start.
			if _, err := adapter.Respond(ctx, imageRequest(), llm.RequestOptions{}); err != nil {
				t.Fatalf("later request: %v", err)
			}
			if calls.Load() != 3 || len(warnings) != 1 {
				t.Fatalf("later request: requests = %d, warnings = %q", calls.Load(), warnings)
			}

			// Another model behind the same adapter still gets its images.
			vision := imageRequest()
			vision.Model.ID = "vision-model"
			if _, err := adapter.Respond(ctx, vision, llm.RequestOptions{}); err != nil {
				t.Fatalf("request to another model: %v", err)
			}
			bodies := answered()
			if calls.Load() != 4 || len(warnings) != 1 || !bytes.Contains(bodies[len(bodies)-1], []byte("base64,AAAA")) ||
				!bytes.Contains(bodies[len(bodies)-1], []byte("base64,BBBB")) {
				t.Fatalf("request to another model: requests = %d, warnings = %q, body = %s", calls.Load(), warnings, bodies[len(bodies)-1])
			}
		})
	}
}

func TestAdapterKeepsAnImageRejectionOfARequestWithoutImages(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"error":{"code":"1210","message":"`+gatewayImageRejection+`"}}`)
	}))
	defer server.Close()
	warned := false
	ctx := llm.WithWarningReporter(t.Context(), func(string) { warned = true })

	_, err := newTestAdapter(t, server.URL).Respond(ctx, validRequest(), llm.RequestOptions{})
	apiErr, ok := errors.AsType[*APIError](err)
	if !ok || apiErr.Code != "1210" || calls.Load() != 1 || warned {
		t.Fatalf("error = %v, requests = %d, warned = %v", err, calls.Load(), warned)
	}
}
