package contextbuilder

import (
	"encoding/json/v2"
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestContextCheckpointBoundsEarlierPromptsAndPreservesFutureCompactions(t *testing.T) {
	full := NewBuilder().(*builder)
	for index := range 80 {
		text := fmt.Sprintf("prompt %d: ", index) + strings.Repeat("я", 1000)
		if index == 79 {
			text += strings.Repeat("long input ", 5000)
		}
		payload, err := json.Marshal(text)
		if err != nil {
			t.Fatal(err)
		}
		if err := full.AddExternalInput(inbox.Input{ID: inbox.ID(fmt.Sprint(index)), Kind: inbox.InputExternal, Payload: payload}); err != nil {
			t.Fatal(err)
		}
		full.Commit()
		full.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "answer"}}}})
	}
	response := llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "summary"}}}}
	if _, ok := full.Compact(response, nil); !ok {
		t.Fatal("compaction failed")
	}
	encoded, err := full.Checkpoint(nil)
	if err != nil {
		t.Fatal(err)
	}
	var value builderCheckpoint
	if err := json.Unmarshal(encoded, &value); err != nil {
		t.Fatal(err)
	}
	var bytes int
	for _, message := range value.Answered {
		bytes += len(message)
	}
	if bytes > keptInputsBudget || len(value.Answered) >= len(full.answered) || value.AnsweredOmitted == 0 {
		t.Fatalf("checkpoint retained unbounded earlier prompts: %d bytes, %d messages, %d omitted", bytes, len(value.Answered), value.AnsweredOmitted)
	}
	resumed := NewBuilder().(*builder)
	if err := resumed.RestoreCheckpoint(encoded); err != nil {
		t.Fatal(err)
	}
	for generation := range 3 {
		// The saved long prompt must not be clipped a second time. Once it
		// ages out, its omission still counts exactly once in later summaries.
		payload, _ := json.Marshal(fmt.Sprintf("after checkpoint %d %s", generation, strings.Repeat("new ", generation*3000)))
		for _, current := range []*builder{full, resumed} {
			if err := current.AddExternalInput(inbox.Input{ID: "new", Kind: inbox.InputExternal, Payload: payload}); err != nil {
				t.Fatal(err)
			}
			current.Commit()
			current.AddModelResponse(response)
		}
		want, _ := full.Compact(response, nil)
		got, _ := resumed.Compact(response, nil)
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("future compaction %d differs", generation)
		}
		encoded, err = resumed.Checkpoint(nil)
		if err != nil {
			t.Fatal(err)
		}
		if err := resumed.RestoreCheckpoint(encoded); err != nil {
			t.Fatal(err)
		}
	}
}
