package responsesapi

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestUsageLimitsAreLeftToTheHarness(t *testing.T) {
	resets := time.Now().Add(3 * time.Hour).Unix()
	for _, test := range []struct {
		name, contentType, body, retryAfter string
		status, requests                    int
		min, max                            time.Duration
		limit                               bool
	}{
		// A ChatGPT subscription's window says when it resets, and is not retried.
		{name: "subscription window", status: 429, contentType: "application/json", requests: 1, limit: true,
			body: fmt.Sprintf(`{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","resets_at":%d}}`, resets),
			min:  3*time.Hour - time.Minute, max: 3 * time.Hour},
		{name: "reset in seconds", status: 429, contentType: "application/json", requests: 1, limit: true,
			body: `{"error":{"code":"usage_limit_reached","message":"limit","resets_in_seconds":5400}}`,
			min:  89 * time.Minute, max: 90 * time.Minute},
		{name: "stream error", contentType: "text/event-stream", requests: 1, limit: true,
			body: "data: {\"type\":\"error\",\"error\":{\"type\":\"usage_limit_reached\",\"message\":\"limit\",\"resets_in_seconds\":60}}\n\n",
			min:  59 * time.Second, max: time.Minute},
		// A gateway that cools the account down is retried, then left to the harness.
		{name: "cooldown", status: 429, contentType: "application/json", requests: 2, limit: true, retryAfter: "1",
			body: `{"error":{"code":"model_cooldown","message":"cooling down"}}`, min: time.Second, max: time.Second},
		// Only billing lifts these.
		{name: "billing", status: 429, contentType: "application/json", requests: 1,
			body: `{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}`},
	} {
		t.Run(test.name, func(t *testing.T) {
			var requests atomic.Int32
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				requests.Add(1)
				w.Header().Set("Content-Type", test.contentType)
				if test.retryAfter != "" {
					w.Header().Set("Retry-After", test.retryAfter)
				}
				if test.status != 0 {
					w.WriteHeader(test.status)
				}
				_, _ = io.WriteString(w, test.body)
			}))
			defer server.Close()
			adapter := newTestAdapterWithConfig(t, Config{Endpoint: server.URL, MaxAttempts: new(2)})
			_, err := adapter.Respond(t.Context(), validRequest(), llm.RequestOptions{})
			limit, ok := errors.AsType[*llm.RateLimitError](err)
			if ok != test.limit || ok && (limit.RetryAfter < test.min || limit.RetryAfter > test.max) {
				t.Fatalf("error = %v (%#v)", err, limit)
			}
			if _, ok := errors.AsType[*APIError](err); !ok {
				t.Fatalf("error = %v, want the API error inside", err)
			}
			if int(requests.Load()) != test.requests {
				t.Fatalf("requests = %d, want %d", requests.Load(), test.requests)
			}
		})
	}
}
