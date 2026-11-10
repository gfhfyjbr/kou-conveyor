package main

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/anthropic"
)

type messagesReply struct {
	Status  int
	Body    string
	Headers map[string]string
}
type messagesInput struct {
	Action    string
	Request   llm.Request
	Body      jsontext.Value
	Stream    string
	Anthropic struct {
		Official        bool
		BaseURL         string
		APIKey          *string
		MaxAttempts     *int
		MaxTokens       int64
		CacheTTL        string
		ThinkingTurns   *int
		ServerFallbacks *bool
		Repeat          int
		Replies         []messagesReply
	}
}
type messagesCapture struct {
	Body    jsontext.Value
	Headers map[string]string
	Path    string
}
type messagesTransport struct {
	mu       sync.Mutex
	replies  []messagesReply
	requests []messagesCapture
}

func (transport *messagesTransport) snapshot() []messagesCapture {
	transport.mu.Lock()
	defer transport.mu.Unlock()
	return append(make([]messagesCapture, 0, len(transport.requests)), transport.requests...)
}

// RoundTrip never delegates to a network transport, including for the official URL.
func (transport *messagesTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	transport.mu.Lock()
	defer transport.mu.Unlock()
	var body []byte
	if request.Body != nil {
		var err error
		body, err = io.ReadAll(io.LimitReader(request.Body, 8<<20))
		if err != nil {
			return nil, err
		}
	}
	if len(body) == 0 {
		body = nil
	}
	headers := map[string]string{}
	for _, name := range []string{"Content-Type", "Accept", "X-Api-Key", "Authorization", "Anthropic-Version", "Anthropic-Beta"} {
		if value := strings.Join(request.Header.Values(name), ","); value != "" {
			headers[name] = value
		}
	}
	index := len(transport.requests)
	transport.requests = append(transport.requests, messagesCapture{Body: body, Headers: headers, Path: request.URL.RequestURI()})
	if len(transport.replies) == 0 {
		return nil, fmt.Errorf("oracle has no synthetic reply")
	}
	reply := transport.replies[min(index, len(transport.replies)-1)]
	if reply.Status == 0 {
		reply.Status = 200
	}
	contentType := "text/event-stream"
	if request.Method == http.MethodGet {
		contentType = "application/json"
	}
	header := http.Header{"Content-Type": []string{contentType}}
	for name, value := range reply.Headers {
		header.Set(name, value)
	}
	return &http.Response{StatusCode: reply.Status, Status: fmt.Sprintf("%d %s", reply.Status, http.StatusText(reply.Status)), Header: header, Body: io.NopCloser(strings.NewReader(reply.Body)), Request: request}, nil
}
func runAnthropic(data []byte) error {
	var input messagesInput
	if err := json.Unmarshal(data, &input); err != nil {
		return err
	}
	settings := input.Anthropic
	transport := &messagesTransport{requests: make([]messagesCapture, 0), replies: settings.Replies}
	if len(transport.replies) == 0 {
		stream := input.Stream
		if input.Action != "anthropic-stream" {
			body := input.Body
			if len(body) == 0 {
				body = jsontext.Value(`{"id":"msg","type":"message","role":"assistant","model":"fixture","content":[],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}`)
			}
			event := append([]byte(`{"type":"message_start","message":`), body...)
			event = append(event, '}')
			event = bytes.ReplaceAll(bytes.ReplaceAll(event, []byte("\n"), []byte(" ")), []byte("\r"), []byte(" "))
			stream = "event: message_start\ndata: " + string(event) + "\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
		}
		transport.replies = []messagesReply{{Status: 200, Body: stream}}
	}
	base := "http://messages.fixture.invalid/prefix/v1/"
	if settings.Official {
		base = anthropic.BaseURL
	}
	if settings.BaseURL != "" {
		base = settings.BaseURL
	}
	httpClient := &http.Client{Transport: transport}
	if input.Action == "anthropic-http" || input.Action == "anthropic-models" {
		if settings.Official || settings.BaseURL != "" {
			return fmt.Errorf("oracle HTTP action uses only its owned loopback server")
		}
		server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
			response, err := transport.RoundTrip(request)
			if err != nil {
				http.Error(writer, err.Error(), http.StatusInternalServerError)
				return
			}
			defer response.Body.Close()
			for name, values := range response.Header {
				for _, value := range values {
					writer.Header().Add(name, value)
				}
			}
			writer.WriteHeader(response.StatusCode)
			_, _ = io.Copy(writer, response.Body)
		}))
		defer server.Close()
		base = server.URL + "/prefix/v1/"
		httpClient = server.Client()
		httpClient.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	}
	key := "synthetic-key"
	if settings.APIKey != nil {
		key = *settings.APIKey
	}
	attempts := 1
	if settings.MaxAttempts != nil {
		attempts = *settings.MaxAttempts
	}
	config := anthropic.Config{APIKey: key, BaseURL: base, MaxAttempts: &attempts, MaxTokens: settings.MaxTokens, CacheTTL: settings.CacheTTL, ThinkingTurns: settings.ThinkingTurns, ServerFallbacks: settings.ServerFallbacks, HTTPClient: httpClient}
	client, err := anthropic.NewClient(config)
	var result any
	message := ""
	if err != nil {
		message = err.Error()
	} else {
		defer client.Close()
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if input.Action == "anthropic-models" {
			models, listErr := anthropic.ListModels(ctx, config)
			listMessage := ""
			if listErr != nil {
				listMessage = listErr.Error()
			}
			result = struct {
				Requests    []messagesCapture
				Models      []string
				Error       string
				Unavailable bool
			}{transport.snapshot(), models, listMessage, errors.Is(listErr, anthropic.ErrModelsUnavailable)}
		} else {
			stream := make([]llm.StreamEvent, 0)
			prefixes := make([]llm.PrefixReport, 0)
			type retry struct {
				Attempt, MaxAttempts int
				Delay                int64
				Error                string
			}
			retries := make([]retry, 0)
			ctx = llm.WithStreamReporter(ctx, func(event llm.StreamEvent) { stream = append(stream, event) })
			ctx = llm.WithPrefixReporter(ctx, func(report llm.PrefixReport) { prefixes = append(prefixes, report) })
			ctx = llm.WithRetryReporter(ctx, func(event llm.Retry) {
				retries = append(retries, retry{event.Attempt, event.MaxAttempts, int64(event.Delay), event.Err.Error()})
			})
			responses := make([]llm.Response, 0)
			errorsOut := make([]string, 0)
			type overflow struct{ Tokens, Limit int64 }
			overflows := make([]*overflow, 0)
			waits := make([]*int64, 0)
			for range max(1, settings.Repeat) {
				response, respondErr := client.Respond(ctx, input.Request, llm.RequestOptions{})
				responses = append(responses, response)
				errorText := ""
				if respondErr != nil {
					errorText = respondErr.Error()
				}
				errorsOut = append(errorsOut, errorText)
				var contextErr *llm.ContextOverflowError
				var exceeded *overflow
				if errors.As(respondErr, &contextErr) {
					exceeded = &overflow{contextErr.Tokens, contextErr.Limit}
				}
				overflows = append(overflows, exceeded)
				var rateErr *llm.RateLimitError
				var wait *int64
				if errors.As(respondErr, &rateErr) {
					n := int64(rateErr.RetryAfter)
					wait = &n
				}
				waits = append(waits, wait)
			}
			result = struct {
				Requests  []messagesCapture
				Responses []llm.Response
				Errors    []string
				Stream    []llm.StreamEvent
				Prefixes  []llm.PrefixReport
				Retries   []retry
				Overflows []*overflow
				Waits     []*int64
			}{transport.snapshot(), responses, errorsOut, stream, prefixes, retries, overflows, waits}
		}
	}
	encoded, err := json.Marshal(struct {
		Result any
		Error  string
	}{result, message})
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
