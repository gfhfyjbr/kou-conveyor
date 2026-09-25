package coordinator

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"reflect"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// A session may change models from one prompt to the next. Each turn records
// the model its request went to, and later requests see that turn's output
// marked as the model's, so an adapter replays another model's reasoning and
// provider IDs to no one else. The session keeps the output as the provider
// returned it.
func TestCoordinatorMarksOutputWithItsTurnsModel(t *testing.T) {
	store := emptyFakeStore()
	responseStored := make(chan sessionstore.ModelResponse, 2)
	store.onAppendModelResponse = func(response sessionstore.ModelResponse) { responseStored <- response }
	returned := llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{
		{ProviderID: "rs_1", Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"thought"}, Raw: jsontext.Value(`{"type":"reasoning","id":"rs_1"}`)}},
		{ProviderID: "msg_1", Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}},
	}}
	adapter := &fakeAdapter{respond: func(context.Context, llm.Request) (llm.Response, error) {
		// Each call gets its own copy, as a provider's decoding would.
		response := returned
		response.Output = append([]llm.Item(nil), returned.Output...)
		return response, nil
	}}
	builder := contextbuilder.NewBuilder()
	builder.SetModel(llm.Model{ID: "model-a"})
	inputs := newTestInbox(t)
	ctx, cancel := context.WithCancel(t.Context())
	current := newTestCoordinatorWithAdapter(store, inputs, newFakeOperationManager(), builder, tool.NewRegistry(tool.StaticTranslators{}), adapter)
	done := make(chan error, 1)
	go func() { done <- current.Run(ctx) }()

	submitTestInput(t, inputs, externalEvent(t, 1, "input-1", "first"))
	first := receiveTestValue(t, responseStored)
	submitTestInput(t, inputs, externalEvent(t, 2, "input-2", "second"))
	receiveTestValue(t, responseStored)
	cancel()
	if err := receiveTestValue(t, done); !errors.Is(err, context.Canceled) {
		t.Fatalf("Run error = %v, want context cancellation", err)
	}

	if !reflect.DeepEqual(first.Response, returned) {
		t.Fatalf("stored response = %#v, want it as returned", first.Response)
	}
	if len(store.appendedTurns) != 2 || store.appendedTurns[0].Model != "model-a" || store.appendedTurns[1].Model != "model-a" {
		t.Fatalf("turns = %#v, want each to record model-a", store.appendedTurns)
	}
	requests := adapter.requestSnapshot()
	if len(requests) != 2 {
		t.Fatalf("requests = %d", len(requests))
	}
	var marked int
	for _, item := range requests[1].Input {
		switch {
		case item.ProviderID == "rs_1" || item.ProviderID == "msg_1":
			if item.Model != "model-a" {
				t.Errorf("replayed output %q is marked %q, want model-a", item.ProviderID, item.Model)
			}
			marked++
		case item.Model != "":
			t.Errorf("input %#v is marked as a model's output", item)
		}
	}
	if marked != 2 {
		t.Fatalf("second request replays %d of the first response's items: %#v", marked, requests[1].Input)
	}
}

func TestWrittenByLeavesTheResponseAlone(t *testing.T) {
	response := llm.Response{Output: []llm.Item{
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "a"}},
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "b"}, Model: "earlier"},
	}}
	marked := writtenBy(response, "model-b")
	if marked.Output[0].Model != "model-b" || marked.Output[1].Model != "earlier" {
		t.Fatalf("marked = %#v", marked.Output)
	}
	if response.Output[0].Model != "" {
		t.Fatal("marking changed the response it was given")
	}
	if got := writtenBy(response, ""); !reflect.DeepEqual(got, response) {
		t.Fatalf("a turn without a model marked its output: %#v", got)
	}
}
