package llm

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"reflect"
	"strings"
	"testing"
)

func TestItemJSONRoundTrip(t *testing.T) {
	tests := []Item{
		{ProviderID: "message-1", Type: ItemMessage, Data: Message{
			Role: RoleAssistant, Text: "hello", Phase: "commentary",
		}},
		{ProviderID: "call-1", Type: ItemToolCall, Data: ToolCall{
			CallID: "call-1", Name: "test", Arguments: `{}`,
		}},
		{ProviderID: "result-1", Type: ItemToolResult, Data: ToolResult{
			CallID: "call-1", Output: []ToolResultOutput{{Kind: ToolResultText, Value: "done"}},
		}},
		{ProviderID: "result-image", Type: ItemToolResult, Data: ToolResult{
			CallID: "call-image", Output: []ToolResultOutput{{Kind: ToolResultImage, Value: "image-data"}},
		}},
		{ProviderID: "result-mixed", Type: ItemToolResult, Data: ToolResult{
			CallID: "call-mixed", Output: []ToolResultOutput{
				{Kind: ToolResultText, Value: "Dimensions: 2000x1500"},
				{Kind: ToolResultImage, Value: "data:image/png;base64,aGVsbG8="},
			},
		}},
		{ProviderID: "reasoning-1", Type: ItemReasoning, Data: Reasoning{
			Summary: []string{"inspect"}, Raw: jsontext.Value(`{"encrypted":"opaque"}`),
		}},
		{ProviderID: "reasoning-empty", Type: ItemReasoning, Data: Reasoning{}},
	}

	for _, want := range tests {
		t.Run(string(want.Type), func(t *testing.T) {
			encoded, err := json.Marshal(want)
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(string(encoded), `"ProviderID"`) ||
				!strings.Contains(string(encoded), `"Data":{`) {
				t.Fatalf("encoded item = %s", encoded)
			}
			var got Item
			if err := json.Unmarshal(encoded, &got); err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(got, want) {
				t.Fatalf("round trip = %#v, want %#v", got, want)
			}
		})
	}
}

func TestItemJSONRejectsInvalidTagAndData(t *testing.T) {
	invalid := []struct {
		item Item
		want string
	}{
		{item: Item{Type: ItemMessage, Data: ToolCall{}}, want: "llm.Message"},
		{item: Item{Type: ItemToolCall, Data: Message{}}, want: "llm.ToolCall"},
		{item: Item{Type: ItemToolResult, Data: Message{}}, want: "llm.ToolResult"},
		{item: Item{Type: ItemReasoning, Data: Message{}}, want: "llm.Reasoning"},
		{item: Item{Type: "unknown", Data: struct{}{}}, want: `unsupported item type "unknown"`},
	}
	for _, test := range invalid {
		if _, err := json.Marshal(test.item); err == nil || !strings.Contains(err.Error(), test.want) {
			t.Fatalf("marshal error = %v, want %q", err, test.want)
		}
	}

	for _, encoded := range []string{
		`{`,
		`{"Type":[],"Data":{}}`,
		`{"ProviderID":[],"Type":"message","Data":{"Role":"user"}}`,
		`{"Type":"unknown","Data":{}}`,
		`{"Type":"message","Data":[]}`,
		`{"Type":"tool_call","Data":[]}`,
		`{"Type":"tool_result","Data":[]}`,
		`{"Type":"tool_result","Data":{"Output":"done"}}`,
		`{"Type":"tool_result","Data":{"Output":42}}`,
		`{"Type":"tool_result","Data":{"Output":{}}}`,
		`{"Type":"tool_result","Data":{"Output":["done"]}}`,
		`{"Type":"tool_result","Data":{"Output":[{"Value":42}]}}`,
		`{"Type":"reasoning","Data":[]}`,
		`{"Type":"message","Data":null}`,
		`{"Type":"tool_call","Data":null}`,
		`{"Type":"tool_result","Data":null}`,
		`{"Type":"reasoning","Data":null}`,
	} {
		var item Item
		if err := json.Unmarshal([]byte(encoded), &item); err == nil {
			t.Fatalf("decoded invalid item %s", encoded)
		}
	}
}
