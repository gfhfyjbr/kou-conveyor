package contextbuilder

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"reflect"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestContextCheckpointAccountsForReasoningPrunedBeforeCompaction(t *testing.T) {
	full, live := NewBuilder().(*builder), NewBuilder().(*builder)
	live.SetPruning(PruneOptions{Budget: 1, ThinkingTurns: 1})
	for index := range 6 {
		payload, _ := json.Marshal(fmt.Sprintf("question %d", index))
		for _, current := range []*builder{full, live} {
			if err := current.AddExternalInput(inbox.Input{ID: inbox.ID(fmt.Sprint(index)), Kind: inbox.InputExternal, Payload: payload}); err != nil {
				t.Fatal(err)
			}
			current.Commit()
			current.AddModelResponse(llm.Response{Output: []llm.Item{
				{Type: llm.ItemReasoning, Data: llm.Reasoning{Raw: jsontext.Value(`"thinking"`)}},
				{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "answer"}},
			}})
		}
		if _, err := live.Build(); err != nil {
			t.Fatal(err)
		}
	}
	if live.thinkingLeftOut == 0 {
		t.Fatal("fixture did not prune reasoning")
	}
	for range 2 {
		response := llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "summary"}}}}
		full.Compact(response, nil)
		live.Compact(response, nil)
		encoded, err := live.Checkpoint(nil)
		if err != nil {
			t.Fatal(err)
		}
		resumed := NewBuilder().(*builder)
		if err := resumed.RestoreCheckpoint(encoded); err != nil {
			t.Fatal(err)
		}
		want, err := full.Build()
		if err != nil {
			t.Fatal(err)
		}
		got, err := resumed.Build()
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("pruned reasoning changed checkpoint replay report: got %#v, want %#v", got.Report, want.Report)
		}
		live = resumed
	}
}
