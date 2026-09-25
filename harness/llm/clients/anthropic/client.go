// Package anthropic adapts the Anthropic Messages API, and endpoints that
// implement it, to llm.Adapter through the official Go SDK.
package anthropic

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"math/rand/v2"
	"net"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	sdk "github.com/anthropics/anthropic-sdk-go"
	"github.com/anthropics/anthropic-sdk-go/option"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

const (
	// BaseURL is Anthropic's API. Other endpoints that speak the Messages API
	// work as well; features only Anthropic serves are then left out.
	BaseURL = "https://api.anthropic.com"
	// DefaultModel is used when neither the request nor the environment names one.
	DefaultModel = "claude-opus-5"
	// DefaultMaxAttempts matches the other clients' retry budget.
	DefaultMaxAttempts = primitives.DefaultRemoteMaxAttempts

	// Streams carry pings; this long without any event means the connection is gone.
	streamIdleTimeout = 10 * time.Minute

	// minimumRoom is the least a response is asked to fit in when the
	// context window has no room for the usual maximum.
	minimumRoom = 4096
)

type Config struct {
	APIKey  string
	BaseURL string
	// Nil uses DefaultMaxAttempts.
	MaxAttempts *int
	// MaxTokens caps each response, thinking included; 0 picks a default for
	// the model.
	MaxTokens int64
	// HTTPClient replaces the default transport, e.g. in tests.
	HTTPClient *http.Client
	// ServerFallbacks lets the API re-run a refused request on Anthropic's
	// recommended fallback model. Nil enables it on Anthropic's API only.
	ServerFallbacks *bool
}

type Client struct {
	api         sdk.Client
	official    bool
	maxAttempts int
	maxTokens   int64
	fallbacks   bool
	// noFallbacks remembers that the endpoint rejected refusal fallbacks.
	noFallbacks atomic.Bool
	idleTimeout time.Duration
	backoff     func(attempt int, overloaded bool) time.Duration
}

var _ llm.Adapter = (*Client)(nil)

func NewClient(config Config) (*Client, error) {
	key := strings.TrimSpace(config.APIKey)
	if key == "" {
		return nil, errors.New("Anthropic API key must be set")
	}
	base, official, err := normalizeBaseURL(config.BaseURL)
	if err != nil {
		return nil, err
	}
	maxAttempts := DefaultMaxAttempts
	if config.MaxAttempts != nil {
		maxAttempts = *config.MaxAttempts
	}
	if maxAttempts <= 0 {
		return nil, errors.New("max attempts must be positive")
	}
	if config.MaxTokens < 0 {
		return nil, errors.New("max tokens must not be negative")
	}
	// The runner resolves credentials itself, and retries are ours: the SDK
	// cannot retry a stream that fails midway.
	options := []option.RequestOption{
		option.WithoutEnvironmentDefaults(), option.WithAPIKey(key), option.WithBaseURL(base),
		option.WithMaxRetries(0), option.WithMiddleware(watchActivity),
	}
	if !official {
		// Many Messages-compatible gateways take the key as a bearer token.
		options = append(options, option.WithAuthToken(key))
	}
	if config.HTTPClient != nil {
		options = append(options, option.WithHTTPClient(config.HTTPClient))
	}
	fallbacks := official
	if config.ServerFallbacks != nil {
		fallbacks = *config.ServerFallbacks
	}
	return &Client{
		api:         sdk.NewClient(options...),
		official:    official,
		maxAttempts: maxAttempts,
		maxTokens:   config.MaxTokens,
		fallbacks:   fallbacks,
		idleTimeout: streamIdleTimeout,
		backoff:     defaultBackoff,
	}, nil
}

// normalizeBaseURL accepts the API root with or without the version path the
// SDK adds itself, and reports whether it is Anthropic's own API.
func normalizeBaseURL(raw string) (string, bool, error) {
	base := strings.TrimRight(strings.TrimSpace(raw), "/")
	if base == "" {
		base = BaseURL
	}
	base = strings.TrimSuffix(base, "/v1")
	parsed, err := url.Parse(base)
	if err != nil || (parsed.Scheme != "https" && parsed.Scheme != "http") || parsed.Host == "" {
		return "", false, fmt.Errorf("invalid Messages API base URL %q", raw)
	}
	return base + "/", strings.EqualFold(parsed.Hostname(), "api.anthropic.com"), nil
}

func (client *Client) Close() error { return nil }

// ErrModelsUnavailable reports an endpoint without a model listing, as some
// Messages API gateways are.
var ErrModelsUnavailable = errors.New("the endpoint does not list models")

// ListModels returns the models an endpoint offers. It proves a base URL and
// key without spending tokens.
func ListModels(ctx context.Context, config Config) ([]string, error) {
	client, err := NewClient(config)
	if err != nil {
		return nil, err
	}
	var models []string
	pages := client.api.Models.ListAutoPaging(ctx, sdk.ModelListParams{Limit: sdk.Int(1000)})
	for pages.Next() {
		models = append(models, pages.Current().ID)
	}
	if err := pages.Err(); err != nil {
		var apiErr *sdk.Error
		if errors.As(err, &apiErr) && (apiErr.StatusCode == http.StatusNotFound || apiErr.StatusCode == http.StatusMethodNotAllowed) {
			return nil, ErrModelsUnavailable
		}
		return nil, describe(err)
	}
	return models, nil
}

func (client *Client) Respond(ctx context.Context, request llm.Request, _ llm.RequestOptions) (llm.Response, error) {
	keepThinking := true
	// room caps the response once the API says the context window cannot
	// hold the input and the usual maximum.
	var room int64
	for attempt := 1; ; attempt++ {
		params, err := client.params(request, keepThinking, room)
		if err != nil {
			return llm.Response{}, err
		}
		message, inputs, err := client.stream(ctx, params)
		if err == nil {
			response := responseFromMessage(message, inputs)
			if !keepThinking {
				// Later requests must render the history as this one did, or the
				// thinking this response returns is rejected in turn.
				response.Output = append([]llm.Item{thinkingReset()}, response.Output...)
			}
			return response, nil
		}
		if ctx.Err() != nil {
			return llm.Response{}, ctx.Err()
		}
		switch {
		case keepThinking && thinkingRejected(err):
			// The API bound replayed thinking to a history that has changed
			// since, e.g. a new system prompt on resume. The documented
			// recovery: answer without the old thinking, once; the response
			// records the reset so no later request replays it again.
			keepThinking = false
			attempt--
			continue
		case len(params.Betas) != 0 && fallbacksRejected(err):
			client.noFallbacks.Store(true)
			attempt--
			continue
		}
		if left, ok := roomLeft(err); ok && left < params.MaxTokens && left >= minimumRoom {
			// Claude models count max_tokens against the context window,
			// so a long conversation asks for a shorter response instead.
			room = left
			attempt--
			continue
		}
		if overflow, ok := contextOverflow(err); ok {
			return llm.Response{}, overflow
		}
		delay, retry := client.retryDelay(err, attempt)
		if !retry || attempt >= client.maxAttempts {
			return llm.Response{}, describe(err)
		}
		timer := time.NewTimer(delay)
		select {
		case <-ctx.Done():
			timer.Stop()
			return llm.Response{}, ctx.Err()
		case <-timer.C:
		}
	}
}

var errIdle = errors.New("Messages API stream went idle")

// stream runs one streaming request and accumulates the final message, along
// with the tool inputs exactly as they streamed: the SDK replaces input that is
// not valid JSON, such as a call cut off by the token limit, with {}.
func (client *Client) stream(ctx context.Context, params sdk.BetaMessageNewParams) (sdk.BetaMessage, map[int][]byte, error) {
	streamContext, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	idle := time.AfterFunc(client.idleTimeout, func() { cancel(errIdle) })
	defer idle.Stop()
	// Any bytes count as activity, including the pings the SDK swallows.
	streamContext = context.WithValue(streamContext, activityKey{}, func() { idle.Reset(client.idleTimeout) })

	stream := client.api.Beta.Messages.NewStreaming(streamContext, params)
	defer stream.Close()
	var message sdk.BetaMessage
	inputs := map[int][]byte{}
	complete := false
	for stream.Next() {
		event := stream.Current()
		if err := message.Accumulate(event); err != nil {
			return message, nil, fmt.Errorf("read Messages API stream: %w", err)
		}
		switch event.Type {
		case "content_block_delta":
			if event.Delta.Type == "input_json_delta" {
				inputs[int(event.Index)] = append(inputs[int(event.Index)], event.Delta.PartialJSON...)
			}
		case "message_stop":
			complete = true
		}
	}
	if err := stream.Err(); err != nil {
		if errors.Is(context.Cause(streamContext), errIdle) {
			return message, nil, errIdle
		}
		return message, nil, err
	}
	if !complete {
		return message, nil, fmt.Errorf("Messages API stream ended before the message did: %w", io.ErrUnexpectedEOF)
	}
	return message, inputs, nil
}

type activityKey struct{}

func watchActivity(request *http.Request, next option.MiddlewareNext) (*http.Response, error) {
	response, err := next(request)
	if touch, ok := request.Context().Value(activityKey{}).(func()); ok && err == nil && response.Body != nil {
		response.Body = activityBody{ReadCloser: response.Body, touch: touch}
	}
	return response, err
}

type activityBody struct {
	io.ReadCloser
	touch func()
}

func (body activityBody) Read(buffer []byte) (int, error) {
	n, err := body.ReadCloser.Read(buffer)
	if n > 0 {
		body.touch()
	}
	return n, err
}

func (client *Client) retryDelay(err error, attempt int) (time.Duration, bool) {
	var apiErr *sdk.Error
	if errors.As(err, &apiErr) {
		kind := string(apiErr.Type())
		switch {
		case kind == "overloaded_error" || apiErr.StatusCode == 529:
			return client.backoff(attempt, true), true
		case kind == "rate_limit_error" || kind == "api_error" || kind == "timeout_error":
		case apiErr.StatusCode == http.StatusRequestTimeout, apiErr.StatusCode == http.StatusConflict,
			apiErr.StatusCode == http.StatusTooManyRequests, apiErr.StatusCode >= 500:
		default:
			return 0, false
		}
		if apiErr.Response != nil {
			if hint := retryAfter(apiErr.Response.Header); hint > 0 {
				return min(hint, time.Minute), true
			}
		}
		return client.backoff(attempt, false), true
	}
	var netErr net.Error
	if errors.Is(err, io.ErrUnexpectedEOF) || errors.Is(err, errIdle) || errors.Is(err, syscall.ECONNRESET) ||
		errors.Is(err, syscall.ECONNREFUSED) || errors.As(err, &netErr) {
		return client.backoff(attempt, false), true
	}
	return 0, false
}

func defaultBackoff(attempt int, overloaded bool) time.Duration {
	delay := min(2*time.Second<<min(attempt-1, 4), 30*time.Second)
	if overloaded {
		// Capacity recovers slowly; unattended runs wait longer between tries.
		delay = max(delay, 10*time.Second*time.Duration(attempt))
	}
	return delay - time.Duration(float64(delay/5)*rand.Float64())
}

func retryAfter(header http.Header) time.Duration {
	if value := header.Get("retry-after-ms"); value != "" {
		if ms, err := strconv.ParseFloat(value, 64); err == nil && ms > 0 {
			return time.Duration(ms * float64(time.Millisecond))
		}
	}
	value := strings.TrimSpace(header.Get("retry-after"))
	if seconds, err := strconv.ParseFloat(value, 64); err == nil && seconds > 0 {
		return time.Duration(seconds * float64(time.Second))
	}
	if deadline, err := http.ParseTime(value); err == nil {
		return max(0, time.Until(deadline))
	}
	return 0
}

func thinkingRejected(err error) bool {
	var apiErr *sdk.Error
	if !errors.As(err, &apiErr) || apiErr.StatusCode != http.StatusBadRequest {
		return false
	}
	message := strings.ToLower(errorMessage(apiErr))
	return strings.Contains(message, "thinking") && strings.Contains(message, "signature")
}

// contextLimit matches how the API says input and max_tokens do not fit:
// "input length and `max_tokens` exceed context limit: 188240 + 64000 > 200000".
var contextLimit = regexp.MustCompile(`(\d+)\s*\+\s*(\d+)\s*>\s*(\d+)`)

// roomLeft reports how many tokens the context window leaves for the
// response, when the API rejected a request for asking for more.
func roomLeft(err error) (int64, bool) {
	var apiErr *sdk.Error
	if !errors.As(err, &apiErr) || apiErr.StatusCode != http.StatusBadRequest {
		return 0, false
	}
	message := errorMessage(apiErr)
	match := contextLimit.FindStringSubmatch(message)
	if match == nil || !strings.Contains(message, "max_tokens") {
		return 0, false
	}
	input, inputErr := strconv.ParseInt(match[1], 10, 64)
	window, windowErr := strconv.ParseInt(match[3], 10, 64)
	if inputErr != nil || windowErr != nil || window <= input {
		return 0, false
	}
	return window - input, true
}

// promptTooLong matches how the API rejects input the context window cannot
// hold: "prompt is too long: 215000 tokens > 200000 maximum".
var promptTooLong = regexp.MustCompile(`(?i)(?:prompt|input) is too long(?:\D*?(\d+) tokens? > (\d+))?`)

// contextOverflow reports a request the context window cannot hold: input
// too long for it, input that leaves less than minimumRoom for the
// response, or a request over the API's size limit.
func contextOverflow(err error) (*llm.ContextOverflowError, bool) {
	var apiErr *sdk.Error
	if !errors.As(err, &apiErr) {
		return nil, false
	}
	overflow := &llm.ContextOverflowError{Err: describe(err)}
	if apiErr.StatusCode == http.StatusRequestEntityTooLarge {
		return overflow, true
	}
	if apiErr.StatusCode != http.StatusBadRequest {
		return nil, false
	}
	message := errorMessage(apiErr)
	if match := promptTooLong.FindStringSubmatch(message); match != nil {
		overflow.Tokens, _ = strconv.ParseInt(match[1], 10, 64)
		overflow.Limit, _ = strconv.ParseInt(match[2], 10, 64)
		return overflow, true
	}
	if match := contextLimit.FindStringSubmatch(message); match != nil && strings.Contains(message, "max_tokens") {
		input, inputErr := strconv.ParseInt(match[1], 10, 64)
		window, windowErr := strconv.ParseInt(match[3], 10, 64)
		if inputErr == nil && windowErr == nil && window-input < minimumRoom {
			overflow.Tokens, overflow.Limit = input+minimumRoom, window
			return overflow, true
		}
	}
	return nil, false
}

func fallbacksRejected(err error) bool {
	var apiErr *sdk.Error
	return errors.As(err, &apiErr) && apiErr.StatusCode == http.StatusBadRequest &&
		strings.Contains(strings.ToLower(errorMessage(apiErr)), "fallback")
}

// describe turns SDK errors into one line that names the API's own message.
func describe(err error) error {
	var apiErr *sdk.Error
	if !errors.As(err, &apiErr) {
		return fmt.Errorf("Messages API request failed: %w", err)
	}
	kind := string(apiErr.Type())
	if kind == "" {
		kind = http.StatusText(apiErr.StatusCode)
	}
	if apiErr.RequestID != "" {
		return fmt.Errorf("Messages API error %d %s: %s (request %s)", apiErr.StatusCode, kind, errorMessage(apiErr), apiErr.RequestID)
	}
	return fmt.Errorf("Messages API error %d %s: %s", apiErr.StatusCode, kind, errorMessage(apiErr))
}

func errorMessage(apiErr *sdk.Error) string {
	var envelope struct {
		Error struct {
			Message string `json:"message"`
		} `json:"error"`
		Message string `json:"message"`
	}
	raw := apiErr.RawJSON()
	if json.Unmarshal([]byte(raw), &envelope) == nil {
		if envelope.Error.Message != "" {
			return envelope.Error.Message
		}
		if envelope.Message != "" {
			return envelope.Message
		}
	}
	if raw = strings.TrimSpace(raw); raw != "" {
		return raw
	}
	return http.StatusText(apiErr.StatusCode)
}
