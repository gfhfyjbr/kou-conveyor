package responsesapi

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

var retryAfterMessagePattern = regexp.MustCompile(`(?i)\btry again in\s*(\d+(?:\.\d+)?)\s*(ms|milliseconds?|s|seconds?)\b`)

// Providers report failures inconsistently, so this classifier intentionally
// fails open, favoring retries.
func retryableResponseError(err *APIError, retryableStatuses []int) bool {
	if err == nil {
		return false
	}
	switch err.Code {
	case "context_length_exceeded", "insufficient_quota", "usage_not_included", "usage_limit_reached",
		"credit_balance_exhausted", "billing_hard_limit_reached",
		"cyber_policy", "misalignment_policy_violation", "invalid_prompt", "bio_policy",
		"invalid_api_key", "invalid_token":
		return false
	}
	switch err.Type {
	case "authentication_error", "permission_error", "insufficient_quota", "usage_limit_reached":
		return false
	}
	if err.StatusCode < http.StatusOK || err.StatusCode >= http.StatusMultipleChoices {
		return slices.Contains(retryableStatuses, err.StatusCode)
	}
	return true
}

func responseRetryDelay(policy primitives.RemoteRetryPolicy, attempt int, err *APIError, headers http.Header, now time.Time, jitter float64) time.Duration {
	hint := retryAfterHeader(headers.Get("Retry-After"), now)
	if err != nil && err.Code == "rate_limit_exceeded" {
		hint = max(hint, retryAfterMessage(err.Message))
	}
	if hint > 0 {
		return min(hint, policy.MaxBackoff)
	}
	if err != nil && (err.Code == "server_is_overloaded" || err.Code == "slow_down") {
		// Unlike the Codex CLI, unattended runs retry overloads with a longer backoff.
		policy.InitialBackoff = 10 * time.Second
		policy.MaxBackoff = time.Minute
	}
	delay := policy.Backoff(attempt)
	return delay - time.Duration(float64(delay/5)*jitter)
}

// usageLimit reports an attempt refused for a rate or usage limit that the
// retries did not outlast, or that no retry would, as an error the harness
// can wait out: an HTTP 429, or a ChatGPT subscription's exhausted usage
// window. Quotas that only billing lifts, and everything else, give nil.
func usageLimit(result responseAttempt, now time.Time) *llm.RateLimitError {
	apiErr := result.apiError
	if apiErr == nil || result.err == nil && apiErr.StatusCode == http.StatusOK {
		// A response that failed is the model's answer, which the harness
		// asks for again as it does for any failed response.
		return nil
	}
	switch apiErr.Code {
	case "insufficient_quota", "usage_not_included", "credit_balance_exhausted", "billing_hard_limit_reached":
		return nil
	}
	usage := apiErr.Code == "usage_limit_reached" || apiErr.Type == "usage_limit_reached"
	if apiErr.Type == "insufficient_quota" || apiErr.StatusCode != http.StatusTooManyRequests && !usage {
		return nil
	}
	err := result.err
	if err == nil {
		err = fmt.Errorf("create response: %w", apiErr)
	}
	limit := &llm.RateLimitError{Err: err, RetryAfter: max(retryAfterHeader(result.headers.Get("Retry-After"), now), retryAfterMessage(apiErr.Message))}
	if !apiErr.ResetsAt.IsZero() {
		limit.RetryAfter = max(limit.RetryAfter, apiErr.ResetsAt.Sub(now))
	}
	return limit
}

// resetTime reads when a usage limit lifts: at, a Unix time in seconds or
// an RFC 3339 time, or else in, seconds from now. It is zero when neither
// says.
func resetTime(at, in jsontext.Value, now time.Time) time.Time {
	number := func(value jsontext.Value) (float64, bool) {
		var parsed float64
		if len(value) == 0 || json.Unmarshal(value, &parsed) != nil || parsed <= 0 {
			return 0, false
		}
		return parsed, true
	}
	if seconds, ok := number(at); ok {
		return time.Unix(int64(seconds), 0)
	}
	var text string
	if len(at) != 0 && json.Unmarshal(at, &text) == nil {
		if parsed, err := time.Parse(time.RFC3339, strings.TrimSpace(text)); err == nil {
			return parsed
		}
	}
	if seconds, ok := number(in); ok {
		return now.Add(time.Duration(seconds * float64(time.Second)))
	}
	return time.Time{}
}

func retryAfterHeader(value string, now time.Time) time.Duration {
	value = strings.TrimSpace(value)
	if seconds, err := strconv.ParseUint(value, 10, 64); err == nil {
		if seconds > uint64((1<<63-1)/time.Second) {
			return 0
		}
		return time.Duration(seconds) * time.Second
	}
	if deadline, err := http.ParseTime(value); err == nil {
		return max(0, deadline.Sub(now))
	}
	return 0
}

func retryAfterMessage(message string) time.Duration {
	match := retryAfterMessagePattern.FindStringSubmatch(message)
	if match == nil {
		return 0
	}
	unit := "s"
	if strings.HasPrefix(strings.ToLower(match[2]), "m") {
		unit = "ms"
	}
	delay, err := time.ParseDuration(match[1] + unit)
	if err != nil {
		return 0
	}
	return delay
}
