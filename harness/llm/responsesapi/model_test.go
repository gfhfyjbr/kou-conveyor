package responsesapi

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A session may change models from one prompt to the next. What another
// model wrote goes to this one without the reasoning encrypted for that
// model and without the IDs its provider gave the items, which this
// provider may not accept; this model's own output, and output that does not
// say whose it is, are replayed whole.
func TestRequestBodyReplaysOnlyItsModelsReasoningAndIDs(t *testing.T) {
	reasoning := func(id, model string) llm.Item {
		return llm.Item{ProviderID: id, Type: llm.ItemReasoning, Model: model, Data: llm.Reasoning{
			Raw: jsontext.Value(`{"id":"` + id + `","type":"reasoning","summary":[],"encrypted_content":"secret-` + id + `"}`),
		}}
	}
	message := func(id, model, text string) llm.Item {
		return llm.Item{ProviderID: id, Type: llm.ItemMessage, Model: model, Data: llm.Message{Role: llm.RoleAssistant, Text: text}}
	}
	call := func(id, model, callID string) llm.Item {
		return llm.Item{ProviderID: id, Type: llm.ItemToolCall, Model: model, Data: llm.ToolCall{CallID: callID, Name: "Bash", Arguments: `{}`}}
	}
	result := func(callID string) llm.Item {
		return llm.Item{Type: llm.ItemToolResult, Data: llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "ok"}}}}
	}
	input := []llm.Item{
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: "first"}},
		reasoning("rs_legacy", ""), message("msg_legacy", "", "from before models were recorded"),
		reasoning("rs_other", "grok-4.7"), message("msg_other", "grok-4.7", "grok answered"), call("fc_other", "grok-4.7", "call-1"), result("call-1"),
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: "second"}},
		reasoning("rs_own", "gpt-6-astra"), message("msg_own", "gpt-6-astra", "gpt answered"),
	}
	body, err := requestBody(llm.Request{Model: llm.Model{ID: "gpt-6-astra"}, Input: input}, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	var sent struct {
		Input []map[string]any `json:"input"`
	}
	if err := json.Unmarshal(body, &sent); err != nil {
		t.Fatal(err)
	}
	var ids, reasoned []string
	var texts []string
	for _, item := range sent.Input {
		if id, ok := item["id"].(string); ok {
			ids = append(ids, id)
		}
		if item["type"] == "reasoning" {
			reasoned = append(reasoned, item["id"].(string))
		}
		switch content := item["content"].(type) {
		case string:
			texts = append(texts, content)
		case []any:
			for _, part := range content {
				if text, ok := part.(map[string]any)["text"].(string); ok {
					texts = append(texts, text)
				}
			}
		}
	}
	want := []string{"rs_legacy", "msg_legacy", "rs_own", "msg_own"}
	if len(ids) != len(want) {
		t.Fatalf("provider IDs sent = %v, want %v", ids, want)
	}
	for i := range want {
		if ids[i] != want[i] {
			t.Fatalf("provider IDs sent = %v, want %v", ids, want)
		}
	}
	if len(reasoned) != 2 || reasoned[0] != "rs_legacy" || reasoned[1] != "rs_own" {
		t.Fatalf("reasoning sent = %v, want the legacy and own only", reasoned)
	}
	joined := ""
	for _, text := range texts {
		joined += text + "|"
	}
	if joined != "first|from before models were recorded|grok answered|second|gpt answered|" {
		t.Fatalf("messages sent = %q, want every message, the other model's included", joined)
	}
	// The other model's call and its result stay paired.
	var calls, results int
	for _, item := range sent.Input {
		switch item["type"] {
		case "function_call":
			calls++
			if item["call_id"] != "call-1" {
				t.Errorf("call = %v", item)
			}
		case "function_call_output":
			results++
		}
	}
	if calls != 1 || results != 1 {
		t.Fatalf("calls = %d, results = %d", calls, results)
	}
	// The input the request was built from is untouched.
	if input[4].ProviderID != "msg_other" {
		t.Fatal("the request's input was changed")
	}
}

// The accounts gateway answers for Grok, over the Responses API, with every
// field of the request echoed back, the instructions as an empty string
// among them, which OpenAI leaves null.
func TestDecodeResponseOfTheGatewaysGrok(t *testing.T) {
	body := []byte(`{"created_at":1790285736,"completed_at":1790285737,"id":"0d316f05","max_output_tokens":null,"model":"grok-4.3","object":"response",
		"output":[{"id":"rs_0d316f05","summary":[{"text":"The user requested a simple \"ok\" reply.","type":"summary_text"}],"type":"reasoning","status":"completed","encrypted_content":"JGwWldz7"},
		{"content":[{"type":"output_text","text":"ok","logprobs":[],"annotations":[]}],"id":"msg_0d316f05","role":"assistant","type":"message","status":"completed"}],
		"parallel_tool_calls":true,"previous_response_id":null,"reasoning":{"effort":"low","summary":"detailed"},"temperature":0.699999988079071,
		"text":{"format":{"type":"text"}},"tool_choice":"auto","tools":[],"top_p":0.949999988079071,
		"usage":{"input_tokens":197,"input_tokens_details":{"cached_tokens":192},"output_tokens":101,"output_tokens_details":{"reasoning_tokens":100},"total_tokens":298,
			"num_sources_used":0,"num_server_side_tools_used":0,"cost_in_usd_ticks":2971500,"context_details":{"input_tokens":197,"output_tokens":109}},
		"user":null,"incomplete_details":null,"status":"completed","store":false,"metadata":{"system_fingerprint":"fp_eb3c003fc66c14ed"},"background":false,
		"service_tier":"default","truncation":"disabled","top_logprobs":0,"presence_penalty":0.0,"frequency_penalty":0.0,"prompt_cache_key":"a479e37c",
		"max_tool_calls":null,"safety_identifier":null,"error":null,"instructions":""}`)
	response, err := decodeResponse(body)
	if err != nil {
		t.Fatal(err)
	}
	if response.Stop != llm.StopComplete || len(response.Output) != 2 || response.Usage.InputTokens != 197 || response.Usage.ReasoningTokens != 100 {
		t.Fatalf("response = %#v", response)
	}
	if message, ok := response.Output[1].Data.(llm.Message); !ok || message.Text != "ok" {
		t.Fatalf("answer = %#v", response.Output[1])
	}
	for _, instructions := range []string{`"be brief"`, `[{"role":"developer","content":"be brief"}]`, `null`} {
		body := []byte(`{"id":"r","status":"completed","output":[],"instructions":` + instructions + `}`)
		if _, err := decodeResponse(body); err != nil {
			t.Errorf("instructions %s: %v", instructions, err)
		}
	}
}
