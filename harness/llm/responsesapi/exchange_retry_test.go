package responsesapi

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestExchangeRestartsFailedGeneration(t *testing.T) {
	for _, kind := range []string{"error", "response.failed"} {
		for _, code := range []string{"rate_limit_exceeded", "server_error", "server_is_overloaded", "slow_down"} {
			t.Run(kind+"/"+code, func(t *testing.T) {
				t.Parallel()
				var calls atomic.Int32
				type sentRequest struct{ body, cacheKey string }
				requests := make(chan sentRequest, 2)
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					n := calls.Add(1)
					body, err := io.ReadAll(r.Body)
					if err != nil {
						t.Error(err)
					}
					if r.Method != "POST" || r.URL.RequestURI() != "/responses?tenant=1" {
						t.Errorf("request = %s %s", r.Method, r.URL)
					}
					if r.Header.Get("Authorization") != "Bearer token" || r.Header.Get("Accept") != "text/event-stream" {
						t.Error("headers changed")
					}
					var wire struct {
						Store  *bool `json:"store"`
						Stream bool  `json:"stream"`
					}
					if err := json.Unmarshal(body, &wire); err != nil || wire.Store == nil || *wire.Store || !wire.Stream {
						t.Errorf("invalid request: %s, %v", body, err)
					}
					requests <- sentRequest{string(body), r.Header.Get("session-id")}
					w.Header().Set("Content-Type", "text/event-stream")
					if n == 1 {
						w.Header().Set("Retry-After", "1")
						_, _ = io.WriteString(w, "data: {\"type\":\"response.created\",\"response\":{\"id\":\"old\"},\"sequence_number\":0}\n\n")
						_, _ = fmt.Fprintf(w, "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":%s,\"sequence_number\":7}\n\n", fallbackCall)
						_, _ = io.WriteString(w, retryErrorEvent(kind, code, "temporary failure"))
						return
					}
					_, _ = io.WriteString(w, "data: {\"type\":\"response.created\",\"response\":{\"id\":\"new\"}}\n\n")
					_, _ = fmt.Fprintf(w, "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":%s}\n\n", fallbackMessage)
					_, _ = io.WriteString(w, "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"new\",\"status\":\"completed\",\"output\":[]}}\n\n")
				}))
				defer server.Close()
				var traces atomic.Int32
				adapter := newTestAdapterWithConfig(t, Config{
					Endpoint: server.URL + "/responses?tenant=1", MaxAttempts: new(2),
					Headers:           map[string][]string{"Authorization": {"Bearer token"}},
					CacheKeyPlacement: CacheKeyPlacement{Header: "session-id", UsePromptCacheKeyField: true},
					Trace:             func(Exchange) { traces.Add(1) },
				})
				response, err := adapter.Respond(t.Context(), validRequest(), llm.RequestOptions{CacheKey: "same-session"})
				if err != nil || response.ID != "new" || response.Failure != nil || calls.Load() != 2 || traces.Load() != 1 || len(response.Output) != 1 {
					t.Fatalf("response=%#v error=%v calls=%d traces=%d", response, err, calls.Load(), traces.Load())
				}
				if response.Output[0].Data.(llm.Message).Text != "fallback" {
					t.Fatal("retried generation did not discard the old tool call")
				}
				first, second := <-requests, <-requests
				if first != second || first.cacheKey == "" {
					t.Fatal("retry changed request body or cache affinity")
				}
			})
		}
	}
}

func TestExchangeDoesNotRetryPermanentErrors(t *testing.T) {
	for _, kind := range []string{"error", "response.failed", "http"} {
		for _, code := range []string{"context_length_exceeded", "insufficient_quota", "usage_not_included", "usage_limit_reached", "cyber_policy", "misalignment_policy_violation", "invalid_prompt", "bio_policy", "invalid_api_key", "invalid_token"} {
			t.Run(kind+"/"+code, func(t *testing.T) {
				t.Parallel()
				var calls atomic.Int32
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
					calls.Add(1)
					if kind == "http" {
						w.WriteHeader(http.StatusTooManyRequests)
						_, _ = fmt.Fprintf(w, `{"error":{"code":%q,"message":"permanent error"}}`, code)
						return
					}
					w.Header().Set("Content-Type", "text/event-stream")
					_, _ = io.WriteString(w, retryErrorEvent(kind, code, "permanent error"))
				}))
				defer server.Close()
				response, err := newTestAdapter(t, server.URL).Respond(t.Context(), validRequest(), llm.RequestOptions{})
				if calls.Load() != 1 {
					t.Fatalf("requests=%d", calls.Load())
				}
				assertResponseError(t, kind, code, response, err)
			})
		}
	}
}

func TestContextOverflowGivesTheSizes(t *testing.T) {
	for _, test := range []struct {
		body          string
		tokens, limit int64
	}{
		{`{"error":{"code":"context_length_exceeded","type":"invalid_request_error","message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130225 tokens. Please reduce the length of the messages."}}`, 130225, 128000},
		{`{"error":{"code":"context_length_exceeded","type":"invalid_request_error","param":"input","message":"Your input exceeds the context window of this model. Please adjust your input and try again."}}`, 0, 0},
		// OpenRouter gives no code, and a number where it does.
		{`{"error":{"message":"This endpoint's maximum context length is 200000 tokens. However, you requested about 230466 tokens (222466 of text input, 8000 in the output). Please reduce the length of either one.","code":400}}`, 230466, 200000},
	} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusBadRequest)
			_, _ = io.WriteString(w, test.body)
		}))
		_, err := newTestAdapter(t, server.URL).Respond(t.Context(), validRequest(), llm.RequestOptions{})
		server.Close()
		overflow, ok := errors.AsType[*llm.ContextOverflowError](err)
		if !ok || overflow.Tokens != test.tokens || overflow.Limit != test.limit {
			t.Fatalf("error = %v (%#v)", err, overflow)
		}
	}
	// Other rejections are not.
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"error":{"code":"invalid_value","message":"Invalid value for 'input'."}}`)
	}))
	defer server.Close()
	_, err := newTestAdapter(t, server.URL).Respond(t.Context(), validRequest(), llm.RequestOptions{})
	if _, ok := errors.AsType[*llm.ContextOverflowError](err); err == nil || ok {
		t.Fatalf("error = %v", err)
	}
}

func TestExchangePreservesSSEErrorAfterDisconnect(t *testing.T) {
	for _, kind := range []string{"error", "response.failed"} {
		for _, code := range []string{"insufficient_quota", "rate_limit_exceeded"} {
			t.Run(kind+"/"+code, func(t *testing.T) {
				t.Parallel()
				var calls atomic.Int32
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
					calls.Add(1)
					w.Header().Set("Content-Type", "text/event-stream")
					w.Header().Set("Content-Length", "100000")
					_, _ = io.WriteString(w, retryErrorEvent(kind, code, "Please try again in 10ms."))
					_, _ = io.WriteString(w, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"trailer\"}\n\n")
				}))
				defer server.Close()
				adapter := newTestAdapterWithConfig(t, Config{Endpoint: server.URL, MaxAttempts: new(2)})
				response, err := adapter.Respond(t.Context(), validRequest(), llm.RequestOptions{})
				wantCalls := int32(1)
				if code == "rate_limit_exceeded" {
					wantCalls = 2
				}
				if calls.Load() != wantCalls {
					t.Fatalf("requests=%d, want %d", calls.Load(), wantCalls)
				}
				assertResponseError(t, kind, code, response, err)
			})
		}
	}
}

func TestExchangeIncompleteResponsesAreNotRetried(t *testing.T) {
	for reason, stop := range map[string]llm.StopReason{"max_output_tokens": llm.StopMaxOutputTokens, "content_filter": llm.StopRefused} {
		t.Run(reason, func(t *testing.T) {
			t.Parallel()
			var calls atomic.Int32
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				calls.Add(1)
				w.Header().Set("Content-Type", "text/event-stream")
				_, _ = fmt.Fprintf(w, "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"output\":[],\"incomplete_details\":{\"reason\":%q}}}\n\n", reason)
			}))
			defer server.Close()
			response, err := newTestAdapter(t, server.URL).Respond(t.Context(), validRequest(), llm.RequestOptions{})
			if err != nil || response.Stop != stop || calls.Load() != 1 {
				t.Fatalf("response=%#v error=%v requests=%d", response, err, calls.Load())
			}
		})
	}
}

func retryErrorEvent(kind, code, message string) string {
	if kind == "response.failed" {
		return fmt.Sprintf("data: {\"type\":\"response.failed\",\"response\":{\"id\":\"old\",\"status\":\"failed\",\"error\":{\"code\":%q,\"message\":%q},\"output\":[]}}\n\n", code, message)
	}
	return fmt.Sprintf("data: {\"type\":\"error\",\"code\":%q,\"message\":%q}\n\n", code, message)
}

func assertResponseError(t *testing.T, kind, code string, response llm.Response, err error) {
	t.Helper()
	if code == contextLengthExceeded {
		// However it arrives, it is a request the context window cannot hold.
		_, overflow := errors.AsType[*llm.ContextOverflowError](err)
		apiErr, _ := errors.AsType[*APIError](err)
		if !overflow || apiErr == nil || apiErr.Code != code || response.Failure != nil {
			t.Fatalf("response=%#v error=%v", response, err)
		}
		return
	}
	if kind == "response.failed" {
		if err != nil || response.Failure == nil || response.Failure.Code != code || response.Failure.Message == "" {
			t.Fatalf("response=%#v error=%v", response, err)
		}
		return
	}
	apiErr, ok := errors.AsType[*APIError](err)
	if !ok || apiErr.Code != code || strings.TrimSpace(apiErr.Message) == "" {
		t.Fatalf("error=%v", err)
	}
}
