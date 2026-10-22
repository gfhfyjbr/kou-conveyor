package responsesapi

import (
	"slices"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestStreamedTextIsReported(t *testing.T) {
	var reported []llm.StreamEvent
	state := responseState{report: func(event llm.StreamEvent) { reported = append(reported, event) }}
	for _, event := range []string{
		`{"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}`,
		`{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"**Plan**"}`,
		`{"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":1,"part":{"type":"summary_text","text":""}}`,
		`{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":1,"delta":"Then act"}`,
		`{"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"Do"}`,
		`{"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"ne."}`,
		// A piece without its item, or not as expected, is dropped.
		`{"type":"response.output_text.delta","delta":"lost"}`,
		`{"type":"response.output_text.delta","output_index":1,"delta":7}`,
	} {
		if err := state.observe([]byte(event)); err != nil {
			t.Fatalf("observe %s: %v", event, err)
		}
	}
	want := []llm.StreamEvent{
		{Index: 0, Kind: llm.StreamReasoning, Text: "**Plan**"},
		{Index: 0, Kind: llm.StreamReasoning, Text: "\n\n"},
		{Index: 0, Kind: llm.StreamReasoning, Text: "Then act"},
		{Index: 1, Kind: llm.StreamText, Text: "Do"},
		{Index: 1, Kind: llm.StreamText, Text: "ne."},
	}
	if !slices.Equal(reported, want) {
		t.Fatalf("reported = %+v, want %+v", reported, want)
	}
	if state.terminal || state.err != nil {
		t.Fatalf("pieces ended the stream: terminal %v, err %v", state.terminal, state.err)
	}
}
