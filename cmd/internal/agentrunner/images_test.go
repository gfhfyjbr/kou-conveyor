package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestRunMainSendsAMessagesImagesToTheModel(t *testing.T) {
	requests := make(chan llm.Request, 1)
	client := &fakeClient{respond: func(_ context.Context, request llm.Request) (llm.Response, error) {
		requests <- request
		return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "a cat"},
		}}}, nil
	}}
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(), []string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()},
		func(name string) string {
			return map[string]string{"OPENAI_API_KEY": "secret", "SHELL": "/bin/sh"}[name]
		},
		func() []string { return nil },
		strings.NewReader(`{"messages":[{"content":"What is on [Image 1] and [Image 2]?","images":[
			{"label":"[Image 1]","media_type":"image/png","data":"iVBORw0KGgo="},
			{"media_type":"image/jpg","data":"/9j/4AAQ"},
			{"label":"logo","url":"https://example.com/logo.png"}]}]}`),
		&stdout, &stderr, testConfig(client))
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q, stdout = %q", code, stderr.String(), stdout.String())
	}
	request := <-requests
	want := llm.Message{Role: llm.RoleUser, Text: "What is on [Image 1] and [Image 2]?", Images: []llm.Image{
		{Label: "[Image 1]", URL: "data:image/png;base64,iVBORw0KGgo="},
		{Label: "[Image 2]", URL: "data:image/jpeg;base64,/9j/4AAQ"},
		{Label: "logo", URL: "https://example.com/logo.png"},
	}}
	last := request.Input[len(request.Input)-1].Data
	if !reflect.DeepEqual(last, want) {
		t.Fatalf("prompt = %#v, want %#v", last, want)
	}
	// The session records the images with the prompt, for resuming.
	for _, item := range decodeLogItems(t, stdout.Bytes()) {
		if input, ok := item.Data.(inbox.Input); ok && input.Kind == inbox.InputExternal {
			text, images, err := contextbuilder.ExternalMessage(input.Payload)
			if err != nil || text != want.Text || !reflect.DeepEqual(images, want.Images) {
				t.Fatalf("recorded %q %#v, %v", text, images, err)
			}
			return
		}
	}
	t.Fatal("the prompt was not recorded")
}

func TestValidateRequestChecksImages(t *testing.T) {
	for _, test := range []struct {
		images []RequestImage
		want   string
	}{
		{[]RequestImage{{MediaType: "image/png"}}, "image 1 has neither data nor a url"},
		{[]RequestImage{{MediaType: "image/tiff", Data: []byte("x")}}, "image 1: media_type must be one of"},
		{[]RequestImage{{MediaType: "image/png", Data: []byte("x"), URL: "https://x"}}, "image 1 has both data and a url"},
		{[]RequestImage{{URL: "file:///etc/passwd"}}, "image 1: url must be an http(s) URL"},
		{make([]RequestImage, maxImages+1), "a message takes at most 20 images"},
		{[]RequestImage{{MediaType: "image/png", Data: make([]byte, maxImageBytes+1)}}, "an image takes at most"},
	} {
		_, err := validateRequest(Request{Messages: []RequestMessage{{Content: "hi", Images: test.images}}})
		if err == nil || !strings.Contains(err.Error(), test.want) || !strings.HasPrefix(err.Error(), "messages[0].images: ") {
			t.Errorf("error = %v, want %q", err, test.want)
		}
	}
}

func TestSteeringMessagesTakeImages(t *testing.T) {
	input, err := steeringInput(jsontext.Value(`{"content":"","images":[{"media_type":"image/webp","data":"UklGRg=="}]}`))
	if err != nil {
		t.Fatal(err)
	}
	text, images, err := contextbuilder.ExternalMessage(input.Payload)
	if err != nil || text != "" || !reflect.DeepEqual(images, []llm.Image{{Label: "[Image 1]", URL: "data:image/webp;base64,UklGRg=="}}) {
		t.Fatalf("steered %q %#v, %v", text, images, err)
	}
	if _, err := steeringInput(jsontext.Value(`{"content":"x","images":[{"media_type":"text/html","data":"PGI+"}]}`)); err == nil {
		t.Fatal("an image of an unknown type was accepted")
	}
}
