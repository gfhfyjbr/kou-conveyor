package contextbuilder

import (
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

var pasted = []llm.Image{
	{Label: "[Image 1]", URL: "data:image/png;base64,iVBORw0KGgo="},
	{Label: "[Image 2]", URL: "https://example.com/b.png"},
}

func TestExternalPayloadKeepsTextAsAString(t *testing.T) {
	payload, err := ExternalPayload("Hello", nil)
	if err != nil {
		t.Fatal(err)
	}
	if string(payload) != `"Hello"` {
		t.Fatalf("payload = %s, want a JSON string", payload)
	}
	text, images, err := ExternalMessage(payload)
	if err != nil || text != "Hello" || images != nil {
		t.Fatalf("decoded %q, %v, %v", text, images, err)
	}
}

func TestExternalPayloadCarriesImages(t *testing.T) {
	payload, err := ExternalPayload("Compare [Image 1] with [Image 2]", pasted)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(string(payload), `{"Text":"Compare [Image 1] with [Image 2]","Images":[{"Label":"[Image 1]","URL":"data:image/png;base64,`) {
		t.Fatalf("payload = %s", payload)
	}
	text, images, err := ExternalMessage(payload)
	if err != nil || text != "Compare [Image 1] with [Image 2]" || !reflect.DeepEqual(images, pasted) {
		t.Fatalf("decoded %q, %v, %v", text, images, err)
	}
	if _, err := ExternalPayload("x", []llm.Image{{Label: "[Image 1]"}}); err == nil {
		t.Fatal("an image without a URL was encoded")
	}
	for _, bad := range []string{`{"Text":"x","Other":1}`, `{"Text":"x","Images":[{"Label":"a"}]}`, `{}`, `{"Text":"x"}`, `12`, `null`} {
		if _, _, err := ExternalMessage([]byte(bad)); err == nil {
			t.Errorf("payload %s decoded", bad)
		}
	}
}

func TestBuilderAddsImagesToTheUserMessage(t *testing.T) {
	payload, err := ExternalPayload("What is on [Image 1]?", pasted[:1])
	if err != nil {
		t.Fatal(err)
	}
	current := NewBuilder()
	if err := current.AddExternalInput(inbox.Input{ID: "input-1", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	want := withPreamble(llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: "What is on [Image 1]?", Images: pasted[:1]}})
	if !reflect.DeepEqual(result.Request.Input, want) {
		t.Fatalf("input = %#v, want %#v", result.Request.Input, want)
	}
	// An image counts as much as one in a tool result.
	plain := NewBuilder()
	addInput(t, plain, "What is on [Image 1]?")
	without, _ := plain.Build()
	if result.EstimatedTokens-without.EstimatedTokens != imageTokens {
		t.Fatalf("an image adds %d tokens to the estimate, want %d", result.EstimatedTokens-without.EstimatedTokens, imageTokens)
	}
}

func TestCompactionKeepsTheLabelsOfAnsweredImages(t *testing.T) {
	current := NewBuilder()
	payload, err := ExternalPayload("Fix the layout on [Image 1] and [Image 2]", pasted)
	if err != nil {
		t.Fatal(err)
	}
	if err := current.AddExternalInput(inbox.Input{ID: "input-1", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	current.Commit()
	current.AddModelResponse(answer("Fixed."))
	// A prompt the model has not answered keeps its images after the summary.
	payload, _ = ExternalPayload("And [Image 1]?", pasted[1:])
	if err := current.AddExternalInput(inbox.Input{ID: "input-2", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	current.Commit()
	if !current.Compact(answer("<summary>Summary.</summary>")) {
		t.Fatal("did not compact")
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	summary := messageText(built.Request.Input[1])
	if !strings.Contains(summary, "Fix the layout on [Image 1] and [Image 2]\n[The user attached [Image 1], [Image 2] here; images are left out after a compaction.]") {
		t.Fatalf("kept prompts:\n%s", summary)
	}
	last := built.Request.Input[len(built.Request.Input)-1].Data.(llm.Message)
	if last.Text != "And [Image 1]?" || !reflect.DeepEqual(last.Images, pasted[1:]) {
		t.Fatalf("unanswered prompt = %#v", last)
	}
}
