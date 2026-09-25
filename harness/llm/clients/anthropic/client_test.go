package anthropic

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// server answers each request with the next handler and records what it got.
type server struct {
	t        *testing.T
	mu       sync.Mutex
	handlers []http.HandlerFunc
	requests []recorded
	url      string
}

type recorded struct {
	header http.Header
	body   map[string]any
}

func newServer(t *testing.T, handlers ...http.HandlerFunc) *server {
	s := &server{t: t, handlers: handlers}
	httpServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Error(err)
			return
		}
		var decoded map[string]any
		if err := json.Unmarshal(body, &decoded); err != nil {
			t.Errorf("request body: %v\n%s", err, body)
		}
		s.mu.Lock()
		s.requests = append(s.requests, recorded{header: r.Header.Clone(), body: decoded})
		index := len(s.requests) - 1
		s.mu.Unlock()
		if r.URL.Path != "/v1/messages" {
			t.Errorf("path = %s", r.URL.Path)
		}
		if index >= len(s.handlers) {
			t.Errorf("unexpected request %d", index+1)
			http.Error(w, "unexpected", http.StatusInternalServerError)
			return
		}
		s.handlers[index](w, r)
	}))
	t.Cleanup(httpServer.Close)
	s.url = httpServer.URL
	return s
}

func (s *server) request(index int) recorded {
	s.mu.Lock()
	defer s.mu.Unlock()
	if index >= len(s.requests) {
		s.t.Fatalf("only %d requests were made", len(s.requests))
	}
	return s.requests[index]
}

func (s *server) count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.requests)
}

func newTestClient(t *testing.T, s *server, config Config) *Client {
	t.Helper()
	config.APIKey = "test-key"
	config.BaseURL = s.url
	client, err := NewClient(config)
	if err != nil {
		t.Fatal(err)
	}
	client.backoff = func(int, bool) time.Duration { return time.Millisecond }
	return client
}

// sse writes a Messages API stream from event payloads.
func sse(events ...string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		writeEvents(w, events...)
	}
}

func writeEvents(w http.ResponseWriter, events ...string) {
	for _, event := range events {
		var head struct {
			Type string `json:"type"`
		}
		if err := json.Unmarshal([]byte(event), &head); err != nil {
			panic(fmt.Sprintf("bad event %s: %v", event, err))
		}
		fmt.Fprintf(w, "event: %s\ndata: %s\n\n", head.Type, event)
	}
	w.(http.Flusher).Flush()
}

func apiError(status int, kind, message string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("request-id", "req_test")
		w.WriteHeader(status)
		fmt.Fprintf(w, `{"type":"error","error":{"type":%q,"message":%q}}`, kind, message)
	}
}

func start(model string) string {
	return `{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"` + model +
		`","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":100,"output_tokens":1}}}`
}

func block(index int, content string) string {
	return fmt.Sprintf(`{"type":"content_block_start","index":%d,"content_block":%s}`, index, content)
}

func delta(index int, content string) string {
	return fmt.Sprintf(`{"type":"content_block_delta","index":%d,"delta":%s}`, index, content)
}

func stop(index int) string { return fmt.Sprintf(`{"type":"content_block_stop","index":%d}`, index) }

func finish(reason string) []string {
	return []string{
		`{"type":"message_delta","delta":{"stop_reason":"` + reason + `","stop_sequence":null},"usage":{"output_tokens":42,"output_tokens_details":{"thinking_tokens":7}}}`,
		`{"type":"message_stop"}`,
	}
}

func textResponse(text string) http.HandlerFunc {
	return sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"text","text":""}`),
		delta(0, fmt.Sprintf(`{"type":"text_delta","text":%q}`, text)),
		stop(0),
	}, finish("end_turn"))...)
}

func conversationRequest(model string) llm.Request {
	return llm.Request{
		Model: llm.Model{ID: model, ReasoningEffort: llm.ReasoningEffortHigh},
		Input: []llm.Item{
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleSystem, Text: "You are careful."}},
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleUser, Text: "List the files."}},
		},
		Tools: []llm.Tool{{
			Type: llm.ToolFunction, Name: "Bash", Description: "Run a command.",
			Parameters: map[string]any{
				"type":                 "object",
				"properties":           map[string]any{"command": map[string]any{"type": "string"}},
				"required":             []any{"command"},
				"additionalProperties": false,
			},
		}},
	}
}

func encode(t *testing.T, value any) string {
	t.Helper()
	encoded, err := json.Marshal(value, json.Deterministic(true))
	if err != nil {
		t.Fatal(err)
	}
	return string(encoded)
}

func TestRespondStreamsAToolTurn(t *testing.T) {
	s := newServer(t, sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"thinking","thinking":"","signature":""}`),
		delta(0, `{"type":"thinking_delta","thinking":"Listing is enough."}`),
		delta(0, `{"type":"signature_delta","signature":"sig-1"}`),
		stop(0),
		block(1, `{"type":"text","text":""}`),
		delta(1, `{"type":"text_delta","text":"Looking."}`),
		stop(1),
		block(2, `{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}`),
		delta(2, `{"type":"input_json_delta","partial_json":"{\"command\":"}`),
		delta(2, `{"type":"input_json_delta","partial_json":"\"ls\"}"}`),
		stop(2),
	}, finish("tool_use"))...))
	client := newTestClient(t, s, Config{})

	response, err := client.Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{CacheKey: "session"})
	if err != nil {
		t.Fatal(err)
	}
	if response.ID != "msg_1" || response.Stop != llm.StopComplete || response.Failure != nil || len(response.Output) != 3 {
		t.Fatalf("response = %#v", response)
	}
	thinking := response.Output[0].Data.(llm.Reasoning)
	if !slices.Equal(thinking.Summary, []string{"Listing is enough."}) ||
		string(thinking.Raw) != `{"type":"thinking","thinking":"Listing is enough.","signature":"sig-1"}` {
		t.Fatalf("thinking = %#v (%s)", thinking, thinking.Raw)
	}
	if message := response.Output[1].Data.(llm.Message); message.Role != llm.RoleAssistant || message.Text != "Looking." {
		t.Fatalf("message = %#v", message)
	}
	if call := response.Output[2].Data.(llm.ToolCall); call != (llm.ToolCall{CallID: "toolu_1", Name: "Bash", Arguments: `{"command":"ls"}`}) {
		t.Fatalf("call = %#v", call)
	}
	want := llm.Usage{InputTokens: 115, CachedInputTokens: 100, CacheWriteInputTokens: 5, OutputTokens: 42, ReasoningTokens: 7}
	got := response.Usage
	got.Raw = nil
	if encode(t, got) != encode(t, want) || !response.Usage.Raw.IsValid() {
		t.Fatalf("usage = %#v (%s)", response.Usage, response.Usage.Raw)
	}

	sent := s.request(0)
	if sent.header.Get("X-Api-Key") != "test-key" || sent.header.Get("Authorization") != "Bearer test-key" ||
		sent.header.Get("Anthropic-Version") == "" {
		t.Fatalf("headers = %v", sent.header)
	}
	if beta := sent.header.Get("Anthropic-Beta"); beta != "" {
		t.Fatalf("fallbacks were requested from a gateway: %q", beta)
	}
	body := sent.body
	for field, want := range map[string]string{
		"model":         `"claude-opus-5"`,
		"max_tokens":    `64000`,
		"stream":        `true`,
		"thinking":      `{"display":"summarized","type":"adaptive"}`,
		"output_config": `{"effort":"high"}`,
		"system":        `[{"cache_control":{"type":"ephemeral"},"text":"You are careful.","type":"text"}]`,
		"messages":      `[{"content":[{"cache_control":{"type":"ephemeral"},"text":"List the files.","type":"text"}],"role":"user"}]`,
		"tools":         `[{"description":"Run a command.","input_schema":{"additionalProperties":false,"properties":{"command":{"type":"string"}},"required":["command"],"type":"object"},"name":"Bash"}]`,
	} {
		if got := encode(t, body[field]); got != want {
			t.Errorf("%s = %s, want %s", field, got, want)
		}
	}
	for _, field := range []string{"fallbacks", "temperature", "tool_choice"} {
		if _, ok := body[field]; ok {
			t.Errorf("unexpected %s = %v", field, body[field])
		}
	}
}

func item(kind llm.ItemType, data any) llm.Item { return llm.Item{Type: kind, Data: data} }

func text(value string) []llm.ToolResultOutput {
	return []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: value}}
}

func TestConversationAnswersEveryCallOnce(t *testing.T) {
	thinking := jsontext.Value(`{"type":"thinking","thinking":"Two commands.","signature":"sig-1"}`)
	openAIReasoning := jsontext.Value(`{"type":"reasoning","id":"rs_1","encrypted_content":"x","summary":[]}`)
	items := []llm.Item{
		item(llm.ItemMessage, llm.Message{Role: llm.RoleSystem, Text: "System."}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "Build it."}),
		item(llm.ItemReasoning, llm.Reasoning{Summary: []string{"Two commands."}, Raw: thinking}),
		item(llm.ItemReasoning, llm.Reasoning{Raw: openAIReasoning}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleAssistant, Text: "Starting.", Phase: "commentary"}),
		item(llm.ItemToolCall, llm.ToolCall{CallID: "a", Name: "Bash", Arguments: `{"z":1,"a":2}`}),
		item(llm.ItemToolCall, llm.ToolCall{CallID: "b", Name: "Bash", Arguments: `not json`}),
		// A prompt that arrived while the tools ran, recorded before their results.
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "Also run tests."}),
		item(llm.ItemToolResult, llm.ToolResult{CallID: "b", Output: text("b done")}),
		item(llm.ItemToolResult, llm.ToolResult{CallID: "a", Output: text("still running")}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "   "}),
		// An answer from a turn whose response was refused and left no output.
		item(llm.ItemToolCall, llm.ToolCall{CallID: "c", Name: "Bash", Arguments: `{}`}),
		item(llm.ItemToolResult, llm.ToolResult{CallID: "a", Output: []llm.ToolResultOutput{
			{Kind: llm.ToolResultText, Value: "a done"},
			{Kind: llm.ToolResultImage, Value: "data:image/png;base64,iVBORw0KGgo="},
		}}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "Status?"}),
	}
	system, messages, err := conversation(items, true)
	if err != nil {
		t.Fatal(err)
	}
	if got := encode(t, system); got != `[{"text":"System.","type":"text"}]` {
		t.Fatalf("system = %s", got)
	}
	want := `[` +
		`{"content":[{"text":"Build it.","type":"text"}],"role":"user"},` +
		`{"content":[{"signature":"sig-1","thinking":"Two commands.","type":"thinking"},{"text":"Starting.","type":"text"},` +
		`{"id":"a","input":{"a":2,"z":1},"name":"Bash","type":"tool_use"},` +
		`{"id":"b","input":{"invalid_arguments":"not json"},"name":"Bash","type":"tool_use"}],"role":"assistant"},` +
		`{"content":[{"content":[{"text":"still running","type":"text"}],"tool_use_id":"a","type":"tool_result"},` +
		`{"content":[{"text":"b done","type":"text"}],"tool_use_id":"b","type":"tool_result"},` +
		`{"text":"Also run tests.","type":"text"}],"role":"user"},` +
		`{"content":[{"id":"c","input":{},"name":"Bash","type":"tool_use"}],"role":"assistant"},` +
		`{"content":[{"content":[{"text":"No result was recorded for this tool call.","type":"text"}],"is_error":true,"tool_use_id":"c","type":"tool_result"},` +
		`{"text":"Result of tool call a:","type":"text"},{"text":"a done","type":"text"},` +
		`{"source":{"data":"iVBORw0KGgo=","media_type":"image/png","type":"base64"},"type":"image"},` +
		`{"text":"Status?","type":"text"}],"role":"user"}]`
	if got := encodeSDK(t, messages); got != want {
		t.Fatalf("messages =\n%s\nwant\n%s", got, want)
	}
	// Replayed calls keep the key order the model wrote them in.
	if sent, err := json.Marshal(messages); err != nil || !strings.Contains(string(sent), `"input":{"z":1,"a":2}`) {
		t.Fatalf("sent = %s, %v", sent, err)
	}

	// Without thinking, the same turns come back minus the thinking blocks.
	_, stripped, err := conversation(items, false)
	if err != nil {
		t.Fatal(err)
	}
	if got := encodeSDK(t, stripped); got != strings.Replace(want, `{"signature":"sig-1","thinking":"Two commands.","type":"thinking"},`, "", 1) {
		t.Fatalf("stripped = %s", got)
	}
}

// encodeSDK marshals SDK params through their own MarshalJSON, as the SDK
// sends them, then sorts keys for comparison.
func encodeSDK(t *testing.T, value any) string {
	t.Helper()
	sent, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	var decoded any
	if err := json.Unmarshal(sent, &decoded); err != nil {
		t.Fatal(err)
	}
	return encode(t, decoded)
}

func TestConversationEndsWithAnsweredCalls(t *testing.T) {
	_, messages, err := conversation([]llm.Item{
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "Go."}),
		item(llm.ItemToolCall, llm.ToolCall{CallID: "a", Name: "Bash", Arguments: `{}`}),
	}, true)
	if err != nil {
		t.Fatal(err)
	}
	if len(messages) != 3 || messages[2].Role != "user" || messages[2].Content[0].OfToolResult.ToolUseID != "a" {
		t.Fatalf("messages = %s", encodeSDK(t, messages))
	}
}

func TestRetriesTransientFailuresThenRecoversThinking(t *testing.T) {
	s := newServer(t,
		apiError(529, "overloaded_error", "Overloaded"),
		sse(start("claude-opus-5"), `{"type":"error","error":{"type":"api_error","message":"Internal"}}`),
		apiError(400, "invalid_request_error", "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation."),
		textResponse("Done."),
	)
	client := newTestClient(t, s, Config{})
	request := conversationRequest("claude-opus-5")
	request.Input = append(request.Input,
		item(llm.ItemReasoning, llm.Reasoning{Raw: jsontext.Value(`{"type":"thinking","thinking":"","signature":"sig-old"}`)}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleAssistant, Text: "Earlier answer."}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "And now?"}),
	)
	response, err := client.Respond(t.Context(), request, llm.RequestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if len(response.Output) != 2 || response.Output[1].Data.(llm.Message).Text != "Done." {
		t.Fatalf("response = %#v", response)
	}
	if s.count() != 4 {
		t.Fatalf("requests = %d", s.count())
	}
	if messages := encode(t, s.request(2).body["messages"]); !strings.Contains(messages, "sig-old") {
		t.Fatalf("thinking was dropped before the API rejected it: %s", messages)
	}
	if messages := encode(t, s.request(3).body["messages"]); strings.Contains(messages, "sig-old") || !strings.Contains(messages, "Earlier answer.") {
		t.Fatalf("retry messages = %s", messages)
	}

	// The response records the reset, so the next request, in this runner or
	// a later one, leaves the old thinking out without another rejection.
	if !isThinkingReset(response.Output[0]) {
		t.Fatalf("no reset recorded: %#v", response.Output)
	}
	s = newServer(t, textResponse("Next."))
	request.Input = append(request.Input, response.Output...)
	request.Input = append(request.Input,
		item(llm.ItemReasoning, llm.Reasoning{Raw: jsontext.Value(`{"type":"thinking","thinking":"","signature":"sig-new"}`)}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleAssistant, Text: "Later answer."}),
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "Go on."}),
	)
	if _, err := newTestClient(t, s, Config{}).Respond(t.Context(), request, llm.RequestOptions{}); err != nil {
		t.Fatal(err)
	}
	if messages := encode(t, s.request(0).body["messages"]); strings.Contains(messages, "sig-old") || !strings.Contains(messages, "sig-new") ||
		strings.Contains(messages, "thinking_reset") {
		t.Fatalf("messages after a reset = %s", messages)
	}
}

func TestGivesUpOnPermanentErrorsAndAfterMaxAttempts(t *testing.T) {
	s := newServer(t, apiError(401, "authentication_error", "invalid x-api-key"))
	_, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err == nil || err.Error() != "Messages API error 401 authentication_error: invalid x-api-key (request req_test)" {
		t.Fatalf("err = %v", err)
	}

	attempts := 2
	s = newServer(t, apiError(500, "api_error", "boom"), apiError(500, "api_error", "boom"))
	_, err = newTestClient(t, s, Config{MaxAttempts: &attempts}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err == nil || !strings.Contains(err.Error(), "500 api_error: boom") || s.count() != 2 {
		t.Fatalf("err = %v after %d requests", err, s.count())
	}
}

func TestIdleStreamIsRetried(t *testing.T) {
	release := make(chan struct{})
	s := newServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(w, "event: message_start\ndata: %s\n\n", start("claude-opus-5"))
		w.(http.Flusher).Flush()
		select {
		case <-release:
		case <-r.Context().Done():
		}
	}, textResponse("Back."))
	t.Cleanup(func() { close(release) })
	client := newTestClient(t, s, Config{})
	client.idleTimeout = 200 * time.Millisecond
	response, err := client.Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err != nil || response.Output[0].Data.(llm.Message).Text != "Back." {
		t.Fatalf("response = %#v, %v", response, err)
	}
}

func TestPingsKeepAStreamAlive(t *testing.T) {
	s := newServer(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(w, "event: message_start\ndata: %s\n\n", start("claude-opus-5"))
		for range 6 {
			fmt.Fprint(w, "event: ping\ndata: {\"type\":\"ping\"}\n\n")
			w.(http.Flusher).Flush()
			time.Sleep(50 * time.Millisecond)
		}
		writeEvents(w, slices.Concat([]string{block(0, `{"type":"text","text":"Slow but fine."}`), stop(0)}, finish("end_turn"))...)
	})
	client := newTestClient(t, s, Config{})
	client.idleTimeout = 150 * time.Millisecond
	if _, err := client.Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{}); err != nil {
		t.Fatal(err)
	}
}

func TestCancelStopsRetrying(t *testing.T) {
	s := newServer(t, apiError(529, "overloaded_error", "Overloaded"))
	client := newTestClient(t, s, Config{})
	client.backoff = func(int, bool) time.Duration { return time.Hour }
	ctx, cancel := context.WithTimeout(t.Context(), 100*time.Millisecond)
	defer cancel()
	if _, err := client.Respond(ctx, conversationRequest("claude-opus-5"), llm.RequestOptions{}); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v", err)
	}
}

func TestRefusalDiscardsPartialOutput(t *testing.T) {
	s := newServer(t, sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"text","text":""}`),
		delta(0, `{"type":"text_delta","text":"Here is how to"}`),
		stop(0),
	}, finish("refusal"))...))
	response, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err != nil || response.Stop != llm.StopRefused || len(response.Output) != 0 {
		t.Fatalf("response = %#v, %v", response, err)
	}
}

func TestMaxTokensDropsATruncatedCall(t *testing.T) {
	s := newServer(t, sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"tool_use","id":"toolu_1","name":"Read","input":{}}`),
		delta(0, `{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}`),
		stop(0),
		block(1, `{"type":"tool_use","id":"toolu_2","name":"Write","input":{}}`),
		delta(1, `{"type":"input_json_delta","partial_json":"{\"content\":\"half"}`),
		stop(1),
	}, finish("max_tokens"))...))
	response, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err != nil || response.Stop != llm.StopMaxOutputTokens || len(response.Output) != 1 ||
		response.Output[0].Data.(llm.ToolCall).Arguments != `{"path":"a"}` {
		t.Fatalf("response = %#v, %v", response, err)
	}
}

func TestMalformedToolInputReachesTheHarness(t *testing.T) {
	s := newServer(t, sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"tool_use","id":"toolu_1","name":"Write","input":{}}`),
		delta(0, `{"type":"input_json_delta","partial_json":"{\"content\": nope}"}`),
		stop(0),
	}, finish("tool_use"))...))
	response, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err != nil || len(response.Output) != 1 || response.Output[0].Data.(llm.ToolCall).Arguments != `{"content": nope}` {
		t.Fatalf("response = %#v, %v", response, err)
	}
}

func TestMidOutputFallbackKeepsOnlyText(t *testing.T) {
	s := newServer(t, sse(slices.Concat([]string{
		start("claude-opus-5"),
		block(0, `{"type":"thinking","thinking":"","signature":"sig-declined"}`), stop(0),
		block(1, `{"type":"text","text":"Part one. "}`), stop(1),
		block(2, `{"type":"tool_use","id":"toolu_declined","name":"Bash","input":{}}`), stop(2),
		block(3, `{"type":"fallback","from":{"model":"claude-opus-5"},"to":{"model":"claude-opus-4-8"}}`), stop(3),
		block(4, `{"type":"text","text":"Part two."}`), stop(4),
		block(5, `{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls"}}`), stop(5),
	}, finish("tool_use"))...))
	config := Config{ServerFallbacks: new(true)}
	response, err := newTestClient(t, s, config).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	var kinds []string
	for _, output := range response.Output {
		switch value := output.Data.(type) {
		case llm.Message:
			kinds = append(kinds, value.Text)
		case llm.ToolCall:
			kinds = append(kinds, value.CallID)
		default:
			kinds = append(kinds, fmt.Sprintf("%T", value))
		}
	}
	if !slices.Equal(kinds, []string{"Part one. ", "Part two.", "toolu_2"}) {
		t.Fatalf("output = %q", kinds)
	}
	sent := s.request(0)
	if sent.header.Get("Anthropic-Beta") != "server-side-fallback-2026-07-01" || encode(t, sent.body["fallbacks"]) != `"default"` {
		t.Fatalf("fallbacks not requested: %v %v", sent.header, sent.body["fallbacks"])
	}
}

func TestRejectedFallbacksAreNotRequestedAgain(t *testing.T) {
	s := newServer(t,
		apiError(400, "invalid_request_error", "fallbacks: Extra inputs are not permitted"),
		textResponse("One."),
		textResponse("Two."),
	)
	client := newTestClient(t, s, Config{ServerFallbacks: new(true)})
	for range 2 {
		if _, err := client.Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{}); err != nil {
			t.Fatal(err)
		}
	}
	for index := 1; index < 3; index++ {
		if sent := s.request(index); sent.header.Get("Anthropic-Beta") != "" || sent.body["fallbacks"] != nil {
			t.Fatalf("request %d still asks for fallbacks", index)
		}
	}
}

func TestRequestShapeFollowsTheModel(t *testing.T) {
	for _, test := range []struct {
		model, effort string
		check         func(t *testing.T, body map[string]any)
	}{
		{"claude-opus-5", "max", func(t *testing.T, body map[string]any) {
			if body["max_tokens"] != 128000.0 || encode(t, body["output_config"]) != `{"effort":"max"}` {
				t.Errorf("body = %v", body)
			}
		}},
		{"anthropic/claude-sonnet-4.6", "xhigh", func(t *testing.T, body map[string]any) {
			if encode(t, body["output_config"]) != `{"effort":"high"}` || body["thinking"] == nil {
				t.Errorf("body = %v", body)
			}
		}},
		{"claude-haiku-4-5", "high", func(t *testing.T, body map[string]any) {
			if body["max_tokens"] != 64000.0 || encode(t, body["thinking"]) != `{"budget_tokens":20000,"type":"enabled"}` || body["output_config"] != nil {
				t.Errorf("body = %v", body)
			}
		}},
		{"claude-opus-4-1", "max", func(t *testing.T, body map[string]any) {
			if body["max_tokens"] != 32000.0 || encode(t, body["thinking"]) != `{"budget_tokens":24000,"type":"enabled"}` {
				t.Errorf("body = %v", body)
			}
		}},
		{"glm-4.6", "high", func(t *testing.T, body map[string]any) {
			if body["max_tokens"] != 16384.0 || body["thinking"] != nil || body["output_config"] != nil ||
				strings.Contains(encode(t, body), "cache_control") {
				t.Errorf("body = %v", body)
			}
		}},
	} {
		t.Run(test.model, func(t *testing.T) {
			s := newServer(t, textResponse("ok"))
			request := conversationRequest(test.model)
			request.Model.ReasoningEffort = llm.ReasoningEffort(test.effort)
			if _, err := newTestClient(t, s, Config{}).Respond(t.Context(), request, llm.RequestOptions{}); err != nil {
				t.Fatal(err)
			}
			test.check(t, s.request(0).body)
		})
	}
}

func TestConfiguredMaxTokensWin(t *testing.T) {
	s := newServer(t, textResponse("ok"))
	if _, err := newTestClient(t, s, Config{MaxTokens: 2048}).Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{}); err != nil {
		t.Fatal(err)
	}
	if got := s.request(0).body["max_tokens"]; got != 2048.0 {
		t.Fatalf("max_tokens = %v", got)
	}
}

func TestOfficialAPIStreamsToolInputEagerly(t *testing.T) {
	tools, err := requestTools(conversationRequest("claude-opus-5").Tools, true)
	if err != nil {
		t.Fatal(err)
	}
	if got := encodeSDK(t, tools); !strings.Contains(got, `"eager_input_streaming":true`) {
		t.Fatalf("tools = %s", got)
	}
	if _, err := requestTools([]llm.Tool{{Type: llm.ToolHosted, Name: "web_search"}}, true); err == nil {
		t.Fatal("hosted tools were accepted")
	}
}

func TestNewClientValidatesConfig(t *testing.T) {
	zero := 0
	for _, config := range []Config{
		{},
		{APIKey: "k", BaseURL: "ftp://example.com"},
		{APIKey: "k", MaxAttempts: &zero},
		{APIKey: "k", MaxTokens: -1},
	} {
		if _, err := NewClient(config); err == nil {
			t.Errorf("NewClient(%+v) succeeded", config)
		}
	}
	for raw, want := range map[string]string{
		"":                                BaseURL + "/",
		"https://api.anthropic.com/v1/":   "https://api.anthropic.com/",
		"https://gateway.example.com/api": "https://gateway.example.com/api/",
	} {
		if got, _, err := normalizeBaseURL(raw); err != nil || got != want {
			t.Errorf("normalizeBaseURL(%q) = %q, %v", raw, got, err)
		}
	}
	client, err := NewClient(Config{APIKey: "k"})
	if err != nil || !client.official || !client.fallbacks {
		t.Fatalf("official client = %+v, %v", client, err)
	}
}

func TestToolSchemasEncodeTheSameEveryTime(t *testing.T) {
	tools := []llm.Tool{{Type: llm.ToolFunction, Name: "Edit", Parameters: map[string]any{
		"type": "object", "additionalProperties": false, "description": "an edit", "$defs": map[string]any{"b": 1, "a": 2},
		"properties": map[string]any{"path": map[string]any{"type": "string"}, "content": map[string]any{"type": "string"}},
		"required":   []any{"path"},
	}}}
	first := ""
	for range 100 {
		converted, err := requestTools(tools, false)
		if err != nil {
			t.Fatal(err)
		}
		sent, err := json.Marshal(converted)
		if err != nil {
			t.Fatal(err)
		}
		if first == "" {
			first = string(sent)
		} else if string(sent) != first {
			t.Fatalf("encodings differ:\n%s\n%s", first, sent)
		}
	}
	if !strings.Contains(first, `"input_schema":{"$defs":{"a":2,"b":1},"additionalProperties":false,"description":"an edit"`) {
		t.Fatalf("schema = %s", first)
	}
}

func TestContextWindowFollowsTheModel(t *testing.T) {
	for id, want := range map[string]int64{
		"claude-opus-5":              1_000_000,
		"claude-opus-5-5":            1_000_000,
		"anthropic/claude-fable-5":   1_000_000,
		"claude-opus-4-7":            1_000_000,
		"claude-sonnet-4-6":          200_000,
		"claude-opus-4.1":            200_000,
		"claude-3-5-haiku-latest":    200_000,
		"claude-sonnet-4-5-20250929": 200_000,
		"gpt-4o":                     0,
		"meta-llama/llama-3.3-70b":   0,
		"":                           0,
	} {
		if got := ContextWindow(id); got != want {
			t.Errorf("ContextWindow(%q) = %d, want %d", id, got, want)
		}
	}
}

func TestLongConversationAsksForAShorterResponse(t *testing.T) {
	tooLong := apiError(400, "invalid_request_error", "input length and `max_tokens` exceed context limit: 188240 + 64000 > 200000, decrease input length or `max_tokens` and try again")
	s := newServer(t, tooLong, textResponse("ok"))
	request := conversationRequest("claude-sonnet-4-5")
	response, err := newTestClient(t, s, Config{}).Respond(t.Context(), request, llm.RequestOptions{})
	if err != nil || len(response.Output) != 1 {
		t.Fatalf("response %#v, err %v", response, err)
	}
	if first, second := s.request(0).body["max_tokens"], s.request(1).body["max_tokens"]; first != 64000.0 || second != 11760.0 {
		t.Fatalf("max_tokens = %v, then %v", first, second)
	}
	// The thinking budget fits the smaller response.
	if thinking, _ := s.request(1).body["thinking"].(map[string]any); thinking["budget_tokens"] != 8820.0 {
		t.Fatalf("thinking = %#v", s.request(1).body["thinking"])
	}

	// Too little room is an error to report, and so is a second rejection.
	attempts := 1
	for _, test := range []struct {
		handlers []http.HandlerFunc
		overflow bool
	}{
		{[]http.HandlerFunc{apiError(400, "invalid_request_error", "input length and `max_tokens` exceed context limit: 198000 + 64000 > 200000, decrease input length or `max_tokens` and try again")}, true},
		{[]http.HandlerFunc{tooLong, tooLong}, false},
	} {
		s := newServer(t, test.handlers...)
		_, err := newTestClient(t, s, Config{MaxAttempts: &attempts}).Respond(t.Context(), request, llm.RequestOptions{})
		if err == nil || !strings.Contains(err.Error(), "exceed context limit") || s.count() != len(test.handlers) {
			t.Fatalf("err = %v after %d requests", err, s.count())
		}
		// Too little room for a response is a context the window cannot hold.
		var overflow *llm.ContextOverflowError
		if errors.As(err, &overflow) != test.overflow || test.overflow && (overflow.Tokens != 198000+minimumRoom || overflow.Limit != 200000) {
			t.Fatalf("overflow = %#v", overflow)
		}
	}
}

func TestInputTooLongIsAContextOverflow(t *testing.T) {
	for _, test := range []struct {
		handler       http.HandlerFunc
		tokens, limit int64
	}{
		{apiError(400, "invalid_request_error", "prompt is too long: 215000 tokens > 200000 maximum"), 215000, 200000},
		{apiError(400, "invalid_request_error", "Input is too long for requested model."), 0, 0},
		{apiError(413, "request_too_large", "Request exceeds the maximum allowed number of bytes."), 0, 0},
	} {
		s := newServer(t, test.handler)
		_, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-sonnet-4-5"), llm.RequestOptions{})
		var overflow *llm.ContextOverflowError
		if !errors.As(err, &overflow) || overflow.Tokens != test.tokens || overflow.Limit != test.limit || s.count() != 1 {
			t.Fatalf("err = %v (%#v) after %d requests", err, overflow, s.count())
		}
		if !strings.Contains(err.Error(), "Messages API error") {
			t.Fatalf("err = %v", err)
		}
	}
	// Other rejections are not.
	s := newServer(t, apiError(400, "invalid_request_error", "messages: text content blocks must be non-empty"))
	_, err := newTestClient(t, s, Config{}).Respond(t.Context(), conversationRequest("claude-sonnet-4-5"), llm.RequestOptions{})
	var overflow *llm.ContextOverflowError
	if err == nil || errors.As(err, &overflow) {
		t.Fatalf("err = %v", err)
	}
}

func TestConversationPutsAPromptsImagesBeforeItsText(t *testing.T) {
	items := []llm.Item{
		item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: "What changed between [Image 1] and [Image 2]?", Images: []llm.Image{
			{Label: "[Image 1]", URL: "data:image/png;base64,iVBORw0KGgo="},
			{Label: "[Image 2]", URL: "data:image/bmp;base64,Qk0="},
			{URL: "https://example.com/c.png"},
		}}),
	}
	_, messages, err := conversation(items, true)
	if err != nil {
		t.Fatal(err)
	}
	want := `[{"content":[` +
		`{"text":"[Image 1]","type":"text"},` +
		`{"source":{"data":"iVBORw0KGgo=","media_type":"image/png","type":"base64"},"type":"image"},` +
		`{"text":"[Image 2]","type":"text"},` +
		`{"text":"[image omitted: the Messages API accepts PNG, JPEG, GIF and WebP images]","type":"text"},` +
		`{"source":{"type":"url","url":"https://example.com/c.png"},"type":"image"},` +
		`{"text":"What changed between [Image 1] and [Image 2]?","type":"text"}` +
		`],"role":"user"}]`
	if got := encodeSDK(t, messages); got != want {
		t.Fatalf("messages =\n%s\nwant\n%s", got, want)
	}
}
