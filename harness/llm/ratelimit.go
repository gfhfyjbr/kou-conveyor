package llm

import "time"

// RateLimitError reports a model request refused for a usage limit that an
// adapter does not wait out on its own: a subscription's five-hour or weekly
// window, a gateway that cools an account down, a rate limit that persisted
// through every attempt. RetryAfter is how long until the limit lifts, as
// the provider said; 0 when it did not say.
type RateLimitError struct {
	RetryAfter time.Duration
	Err        error
}

func (err *RateLimitError) Error() string { return err.Err.Error() }

func (err *RateLimitError) Unwrap() error { return err.Err }
