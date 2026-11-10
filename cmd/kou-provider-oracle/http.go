package main

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/fireworks"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/ollama"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openai"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openaicodex"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openrouter"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/responsesapi"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

type httpReply struct {
	Status     int
	Body       string
	RetryAfter string
}
type httpInput struct {
	Request    llm.Request
	CacheKey   string
	Extensions map[string]jsontext.Value
	HTTP       struct {
		Replies     []httpReply
		MaxAttempts int
		Repeat      int
		Family      string
		CacheTTL    string
	}
}
type httpAPI struct {
	Status                     int
	Code, Message, Param, Type string
	ResetUnixNanos             *int64
}
type httpRetry struct {
	Attempt, MaxAttempts int
	Delay                int64
	Error                string
}
type httpCall struct {
	Response  *llm.Response
	Error     string
	API       *httpAPI
	RateLimit *struct{ RetryAfter int64 }
	Overflow  *struct{ Tokens, Limit int64 }
	Stream    []llm.StreamEvent
	Warnings  []string
	Retries   []httpRetry
}
type httpTrace struct {
	Request, Response jsontext.Value
	Status            int
}

func runHTTP(data []byte) error {
	var input httpInput
	if err := json.Unmarshal(data, &input); err != nil {
		return err
	}
	attempts := input.HTTP.MaxAttempts
	if attempts == 0 {
		attempts = 1
	}
	repeat := input.HTTP.Repeat
	if repeat == 0 {
		repeat = 1
	}
	if attempts < 1 || attempts > 3 || repeat < 1 || repeat > 3 || len(input.HTTP.Replies) == 0 || len(input.HTTP.Replies) > 9 {
		return fmt.Errorf("invalid local HTTP oracle limits")
	}
	var mu sync.Mutex
	requests := make([]jsontext.Value, 0)
	cacheHeaders := make([]string, 0)
	requestHeaders := make([]map[string]string, 0)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, err := io.ReadAll(io.LimitReader(request.Body, 8<<20))
		if err != nil {
			http.Error(writer, err.Error(), 500)
			return
		}
		mu.Lock()
		reply := input.HTTP.Replies[min(len(requests), len(input.HTTP.Replies)-1)]
		requests = append(requests, body)
		cacheHeaders = append(cacheHeaders, request.Header.Get("X-Cache-Key"))
		selected := make(map[string]string)
		for _, name := range []string{"authorization", "content-type", "accept", "x-cache-key", "x-session-id", "x-session-affinity", "chatgpt-account-id", "session-id", "originator"} {
			selected[name] = request.Header.Get(name)
		}
		if input.HTTP.Family == "codex" {
			selected["user-agent"] = request.Header.Get("User-Agent")
		}
		requestHeaders = append(requestHeaders, selected)
		mu.Unlock()
		// Input cannot supply Location or any target: every attempt stays here.
		if reply.RetryAfter != "" {
			writer.Header().Set("Retry-After", reply.RetryAfter)
		}
		status := reply.Status
		if status == 0 {
			status = 200
		}
		if status >= 200 && status < 300 {
			writer.Header().Set("Content-Type", "text/event-stream")
		} else {
			writer.Header().Set("Content-Type", "application/json")
		}
		writer.WriteHeader(status)
		_, _ = io.WriteString(writer, reply.Body)
	}))
	defer server.Close()
	remote := primitives.NewRemoteClientWithHTTPClient(server.Client())
	defer remote.Close()
	traces := make([]httpTrace, 0)
	trace := func(exchange responsesapi.Exchange) {
		// Traces borrow buffers in the adapter; the oracle owns its copies.
		request := append(jsontext.Value(nil), exchange.RequestBody...)
		response := append(jsontext.Value(nil), exchange.ResponseBody...)
		// HTTP errors may contain non-JSON text. Their trace is an exact string.
		if json.Unmarshal(response, new(any)) != nil {
			response, _ = json.Marshal(string(response))
		}
		traces = append(traces, httpTrace{Request: request, Response: response, Status: exchange.StatusCode})
	}
	var adapter llm.Adapter
	var err error
	switch input.HTTP.Family {
	case "":
		adapter, err = responsesapi.NewAdapter(remote, responsesapi.Config{
			Endpoint: server.URL + "/responses", Extensions: input.Extensions, MaxAttempts: &attempts,
			CacheKeyPlacement: responsesapi.CacheKeyPlacement{Header: "X-Cache-Key", UsePromptCacheKeyField: true}, Trace: trace,
		})
	case "openai":
		adapter, err = openai.NewClient(openai.Config{APIKey: "fixture-key", BaseURL: server.URL, MaxAttempts: &attempts, Trace: trace})
	case "fireworks":
		adapter, err = fireworks.NewClient(fireworks.Config{APIKey: "fixture-key", BaseURL: server.URL, MaxAttempts: &attempts, Trace: trace})
	case "openrouter":
		adapter, err = openrouter.NewClient(openrouter.Config{APIKey: "fixture-key", BaseURL: server.URL, MaxAttempts: &attempts, CacheTTL: input.HTTP.CacheTTL, Trace: trace})
	case "ollama":
		adapter, err = ollama.NewClient(ollama.Config{BaseURL: server.URL, MaxAttempts: &attempts})
	case "codex":
		adapter, err = openaicodex.NewClient(openaicodex.Config{AccessToken: "fixture-subscription-token", AccountID: "fixture-account", BaseURL: server.URL, MaxAttempts: &attempts, Trace: trace})
	default:
		return fmt.Errorf("unknown local HTTP oracle family")
	}
	if err != nil {
		return err
	}
	if closer, ok := adapter.(io.Closer); ok {
		defer closer.Close()
	}
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	calls := make([]httpCall, 0, repeat)
	for range repeat {
		call := httpCall{Stream: make([]llm.StreamEvent, 0), Warnings: make([]string, 0), Retries: make([]httpRetry, 0)}
		ctx := llm.WithStreamReporter(ctx, func(event llm.StreamEvent) { call.Stream = append(call.Stream, event) })
		ctx = llm.WithWarningReporter(ctx, func(message string) { call.Warnings = append(call.Warnings, message) })
		ctx = llm.WithRetryReporter(ctx, func(retry llm.Retry) {
			call.Retries = append(call.Retries, httpRetry{retry.Attempt, retry.MaxAttempts, int64(retry.Delay), retry.Err.Error()})
		})
		response, err := adapter.Respond(ctx, input.Request, llm.RequestOptions{CacheKey: input.CacheKey})
		if err == nil {
			call.Response = &response
		} else {
			call.Error = err.Error()
			if api, ok := errors.AsType[*responsesapi.APIError](err); ok {
				call.API = &httpAPI{Status: api.StatusCode, Code: api.Code, Message: api.Message, Param: api.Param, Type: api.Type}
				if !api.ResetsAt.IsZero() {
					value := api.ResetsAt.UnixNano()
					call.API.ResetUnixNanos = &value
				}
			}
			if limit, ok := errors.AsType[*llm.RateLimitError](err); ok {
				call.RateLimit = &struct{ RetryAfter int64 }{int64(limit.RetryAfter)}
			}
			if overflow, ok := errors.AsType[*llm.ContextOverflowError](err); ok {
				call.Overflow = &struct{ Tokens, Limit int64 }{overflow.Tokens, overflow.Limit}
			}
		}
		calls = append(calls, call)
	}
	mu.Lock()
	defer mu.Unlock()
	result := struct {
		Calls          []httpCall
		Requests       []jsontext.Value
		CacheHeaders   []string
		RequestHeaders []map[string]string
		Traces         []httpTrace
	}{calls, requests, cacheHeaders, requestHeaders, traces}
	encoded, err := json.Marshal(struct {
		Result any
		Error  string
	}{result, ""})
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
