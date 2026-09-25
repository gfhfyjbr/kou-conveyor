package anthropic

import (
	"encoding/json/jsontext"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A session may change models from one prompt to the next, and a thinking
// block's signature holds for the model that wrote it. What another model
// thought stays out of the request, the rest of its answers stay in, and
// thinking that does not say whose it is is sent for the API to judge.
func TestParamsReplayOnlyTheModelsOwnThinking(t *testing.T) {
	thinking := func(text, model string) llm.Item {
		return llm.Item{Type: llm.ItemReasoning, Model: model, Data: llm.Reasoning{
			Raw: jsontext.Value(`{"type":"thinking","thinking":"` + text + `","signature":"sig-` + text + `"}`),
		}}
	}
	answer := func(text, model string) llm.Item {
		return llm.Item{Type: llm.ItemMessage, Model: model, Data: llm.Message{Role: llm.RoleAssistant, Text: text}}
	}
	user := func(text string) llm.Item {
		return llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: text}}
	}
	input := []llm.Item{
		user("one"), thinking("legacy", ""), answer("legacy answer", ""),
		user("two"), thinking("sonnet", "claude-sonnet-5"), answer("sonnet answer", "claude-sonnet-5"),
		user("three"), thinking("opus", "claude-opus-5-5"), answer("opus answer", "claude-opus-5-5"),
		user("four"),
	}
	client, err := NewClient(Config{APIKey: "k"})
	if err != nil {
		t.Fatal(err)
	}
	params, err := client.params(llm.Request{Model: llm.Model{ID: "claude-opus-5-5", ReasoningEffort: llm.ReasoningEffortHigh}, Input: input}, true, 0)
	if err != nil {
		t.Fatal(err)
	}
	var thought, answered []string
	for _, message := range params.Messages {
		for _, block := range message.Content {
			switch {
			case block.OfThinking != nil:
				thought = append(thought, block.OfThinking.Thinking)
			case block.OfText != nil && message.Role == "assistant":
				answered = append(answered, block.OfText.Text)
			}
		}
	}
	if len(thought) != 2 || thought[0] != "legacy" || thought[1] != "opus" {
		t.Fatalf("thinking replayed = %v, want the legacy block and the model's own", thought)
	}
	if len(answered) != 3 {
		t.Fatalf("answers replayed = %v, want all three", answered)
	}
	if len(input) != 10 || input[4].Model != "claude-sonnet-5" {
		t.Fatal("the request's input was changed")
	}
}
