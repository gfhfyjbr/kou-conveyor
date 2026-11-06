package contextbuilder

import (
	"encoding/json/v2"
	"reflect"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestContextCheckpointRejectsNonSummaryWithoutMutation(t *testing.T) {
	current := NewBuilder().(*builder)
	current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "answer"}}}})
	current.Compact(llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "summary"}}}}, nil)
	encoded, err := current.Checkpoint(nil)
	if err != nil {
		t.Fatal(err)
	}
	var value builderCheckpoint
	if err := json.Unmarshal(encoded, &value); err != nil {
		t.Fatal(err)
	}
	value.Committed[0] = llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "not a compacted user summary"}}
	encoded, err = json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	fresh := NewBuilder().(*builder)
	before, err := fresh.Build()
	if err != nil {
		t.Fatal(err)
	}
	if err := fresh.RestoreCheckpoint(encoded); err == nil {
		t.Fatal("accepted non-summary checkpoint")
	}
	after, err := fresh.Build()
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(after, before) {
		t.Fatal("invalid checkpoint mutated builder")
	}
}
