package cockpit

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Retry is a model request that failed and that the runner sends again, as
// its diagnostic output reports it:
//
//	retry> attempt 2 of 5 failed, trying again in 4s: <why>
//
// The runner retries a request on its own, for minutes when the provider is
// unreachable; the report is all that says meanwhile that the model is not
// answering.
type Retry struct {
	Attempt, MaxAttempts int
	Delay                time.Duration
	Reason               string
}

var retryLine = regexp.MustCompile(`^retry> attempt (\d+) of (\d+) failed, trying again in (\S+): (.*)$`)

// ParseRetry reads a line of the runner's diagnostic output that reports a
// retry.
func ParseRetry(line string) (Retry, bool) {
	match := retryLine.FindStringSubmatch(strings.TrimSpace(line))
	if match == nil {
		return Retry{}, false
	}
	attempt, err1 := strconv.Atoi(match[1])
	attempts, err2 := strconv.Atoi(match[2])
	delay, err3 := time.ParseDuration(match[3])
	if err1 != nil || err2 != nil || err3 != nil {
		return Retry{}, false
	}
	return Retry{Attempt: attempt, MaxAttempts: attempts, Delay: delay, Reason: match[4]}, true
}

// Activity says what the run is doing while the request is tried again: the
// next attempt, and the gist of why the last one failed.
func (r Retry) Activity() string {
	return fmt.Sprintf("Model request failed, retrying (%d of %d): %s", r.Attempt+1, r.MaxAttempts, r.gist())
}

// gist is the end of the reason, which says what went wrong without the
// layers that wrap it: "connection refused", not the request that met it.
func (r Retry) gist() string {
	reason := Clean(r.Reason)
	if i := strings.LastIndex(reason, ": "); i >= 0 && strings.TrimSpace(reason[i+2:]) != "" {
		reason = reason[i+2:]
	}
	return Headline(reason, 80)
}

// Retrying shows, as the run's activity until the model answers, that a
// request to the model failed and is tried again.
func (t *Transcript) Retrying(r Retry) {
	t.retrying = r.Activity()
	t.Activity = t.retrying
}
