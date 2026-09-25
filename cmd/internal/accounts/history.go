package accounts

import (
	"context"
	"encoding/json/v2"
	"os"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/usage"
)

const (
	// BucketSize is the span of time a bucket of history counts.
	BucketSize = 15 * time.Minute
	// Retention is how long history is kept.
	Retention = 7 * 24 * time.Hour
	// maxErrors is how many of an account's latest errors are kept.
	maxErrors = 20
	// maxMessage bounds the text kept of an error.
	maxMessage = 400
)

// History records what each account's requests came to: in each bucket of
// time, how many succeeded and failed and the tokens they used, and the
// account's latest errors. It is the gateway's usage plugin: the gateway
// reports every request it sends upstream, the ones it retries on another
// account included, so an account's history is its uptime. The gateway
// forgets its own counts when it restarts; History is kept in a file.
type History struct {
	path string
	now  func() time.Time

	mu       sync.Mutex
	accounts map[string]*accountHistory // by the gateway's auth ID
	dirty    bool
}

type accountHistory struct {
	Provider string        `json:"provider,omitzero"`
	Index    string        `json:"index,omitzero"`
	Buckets  []Bucket      `json:"buckets,omitzero"` // oldest first, only those with requests
	Errors   []ErrorSample `json:"errors,omitzero"`  // oldest first
	LastOK   time.Time     `json:"last_ok,omitzero"`
	LastFail time.Time     `json:"last_failure,omitzero"`
}

// Bucket counts the requests that started in one span of BucketSize.
type Bucket struct {
	Start  time.Time `json:"t"`
	OK     int64     `json:"ok,omitzero"`
	Failed int64     `json:"failed,omitzero"`
	Input  int64     `json:"in,omitzero"`
	Output int64     `json:"out,omitzero"`
	// Latency is the total, in milliseconds, of the successful requests.
	Latency int64 `json:"ms,omitzero"`
}

// ErrorSample is one failed request.
type ErrorSample struct {
	At      time.Time `json:"at"`
	Model   string    `json:"model,omitzero"`
	Status  int       `json:"status,omitzero"`
	Message string    `json:"message,omitzero"`
}

type historyFile struct {
	Version  int                        `json:"version"`
	Accounts map[string]*accountHistory `json:"accounts"`
}

// NewHistory reads the history kept at path; "" keeps it in memory only.
func NewHistory(path string) *History {
	h := &History{path: path, now: time.Now, accounts: map[string]*accountHistory{}}
	if path == "" {
		return h
	}
	var saved historyFile
	if data, err := os.ReadFile(path); err == nil && json.Unmarshal(data, &saved) == nil && saved.Version == 1 {
		for id, account := range saved.Accounts {
			if account != nil {
				h.accounts[id] = account
			}
		}
	}
	h.prune()
	return h
}

// HandleUsage records one request the gateway sent upstream.
func (h *History) HandleUsage(_ context.Context, record usage.Record) {
	id := strings.TrimSpace(record.AuthID)
	if id == "" {
		return
	}
	at := record.RequestedAt
	if at.IsZero() {
		at = h.now()
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	account := h.accounts[id]
	if account == nil {
		account = &accountHistory{}
		h.accounts[id] = account
	}
	if record.Provider != "" {
		account.Provider = record.Provider
	}
	if record.AuthIndex != "" {
		account.Index = record.AuthIndex
	}
	bucket := account.bucket(at.Truncate(BucketSize))
	if record.Failed {
		bucket.Failed++
		account.LastFail = later(account.LastFail, at)
		sample := ErrorSample{
			At: at.UTC(), Model: firstNonEmpty(record.Alias, record.Model),
			Status: record.Fail.StatusCode, Message: errorMessage(record.Fail.Body),
		}
		account.Errors = append(account.Errors, sample)
		slices.SortStableFunc(account.Errors, func(a, b ErrorSample) int { return a.At.Compare(b.At) })
		if len(account.Errors) > maxErrors {
			account.Errors = account.Errors[len(account.Errors)-maxErrors:]
		}
	} else {
		bucket.OK++
		bucket.Latency += record.Latency.Milliseconds()
		account.LastOK = later(account.LastOK, at)
	}
	bucket.Input += record.Detail.InputTokens
	bucket.Output += record.Detail.OutputTokens
	h.dirty = true
}

// bucket returns the bucket that starts at start, adding it in order.
func (a *accountHistory) bucket(start time.Time) *Bucket {
	start = start.UTC()
	i, found := slices.BinarySearchFunc(a.Buckets, start, func(b Bucket, t time.Time) int { return b.Start.Compare(t) })
	if !found {
		a.Buckets = slices.Insert(a.Buckets, i, Bucket{Start: start})
	}
	return &a.Buckets[i]
}

// Slot is what one stretch of an Uptime counts.
type Slot struct {
	OK     int64 `json:"ok"`
	Failed int64 `json:"failed"`
}

// Uptime is an account's requests over a span of time, in equal slots.
type Uptime struct {
	From  time.Time `json:"from"`
	Slot  int64     `json:"slot_seconds"`
	Slots []Slot    `json:"slots"` // oldest first
	// Totals over the span.
	OK      int64 `json:"ok"`
	Failed  int64 `json:"failed"`
	Input   int64 `json:"input_tokens"`
	Output  int64 `json:"output_tokens"`
	Latency int64 `json:"avg_latency_ms,omitzero"`
	// LastOK and LastFailure are the latest ever recorded.
	LastOK      time.Time `json:"last_ok,omitzero"`
	LastFailure time.Time `json:"last_failure,omitzero"`
}

// Uptime sums an account's history over the span that ends now, in slots
// slots; the account is found by its auth ID or, failing that, its index.
func (h *History) Uptime(id, index string, span time.Duration, slots int) Uptime {
	h.mu.Lock()
	defer h.mu.Unlock()
	var found []*accountHistory
	if account := h.find(id, index); account != nil {
		found = append(found, account)
	}
	return h.uptime(found, span, slots)
}

// UptimeOf sums the history of the accounts with the given auth indexes, as
// the keys of one endpoint are.
func (h *History) UptimeOf(indexes []string, span time.Duration, slots int) Uptime {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.uptime(h.byIndex(indexes), span, slots)
}

// uptime sums accounts' history; the caller holds h.mu.
func (h *History) uptime(accounts []*accountHistory, span time.Duration, slots int) Uptime {
	now := h.now()
	slot := max(BucketSize, span/time.Duration(slots)).Truncate(BucketSize)
	// Slots line up with their size and end with the one in progress.
	end := now.Truncate(slot).Add(slot)
	from := end.Add(-slot * time.Duration(slots))
	u := Uptime{From: from.UTC(), Slot: int64(slot / time.Second), Slots: make([]Slot, slots)}
	var latency int64
	for _, account := range accounts {
		u.LastOK, u.LastFailure = later(u.LastOK, account.LastOK), later(u.LastFailure, account.LastFail)
		for _, b := range account.Buckets {
			if b.Start.Before(from) || !b.Start.Before(end) {
				continue
			}
			n := min(int(b.Start.Sub(from)/slot), slots-1)
			u.Slots[n].OK += b.OK
			u.Slots[n].Failed += b.Failed
			u.OK += b.OK
			u.Failed += b.Failed
			u.Input += b.Input
			u.Output += b.Output
			latency += b.Latency
		}
	}
	if u.OK > 0 {
		u.Latency = latency / u.OK
	}
	return u
}

// Errors returns an account's latest errors, newest first.
func (h *History) Errors(id, index string, n int) []ErrorSample {
	h.mu.Lock()
	defer h.mu.Unlock()
	var found []*accountHistory
	if account := h.find(id, index); account != nil {
		found = append(found, account)
	}
	return latestErrors(found, n)
}

// ErrorsOf returns the latest errors of the accounts with the given auth
// indexes, newest first.
func (h *History) ErrorsOf(indexes []string, n int) []ErrorSample {
	h.mu.Lock()
	defer h.mu.Unlock()
	return latestErrors(h.byIndex(indexes), n)
}

func latestErrors(accounts []*accountHistory, n int) []ErrorSample {
	var out []ErrorSample
	for _, account := range accounts {
		out = append(out, account.Errors...)
	}
	slices.SortStableFunc(out, func(a, b ErrorSample) int { return b.At.Compare(a.At) })
	if len(out) > n {
		out = out[:n]
	}
	return out
}

// byIndex finds accounts by their auth indexes; the caller holds h.mu.
func (h *History) byIndex(indexes []string) []*accountHistory {
	var out []*accountHistory
	for _, account := range h.accounts {
		if account.Index != "" && slices.Contains(indexes, account.Index) {
			out = append(out, account)
		}
	}
	return out
}

// find is an account's history; the caller holds h.mu.
func (h *History) find(id, index string) *accountHistory {
	if account := h.accounts[id]; account != nil {
		return account
	}
	if index == "" {
		return nil
	}
	for _, account := range h.accounts {
		if account.Index == index {
			return account
		}
	}
	return nil
}

// known is an account the history has seen.
type known struct{ id, provider, index string }

// Known lists the accounts the history has seen.
func (h *History) Known() []known {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]known, 0, len(h.accounts))
	for id, account := range h.accounts {
		out = append(out, known{id: id, provider: account.Provider, index: account.Index})
	}
	return out
}

// ForgetIndex drops the history of the account with an auth index.
func (h *History) ForgetIndex(index string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for id, account := range h.accounts {
		if index != "" && account.Index == index {
			delete(h.accounts, id)
			h.dirty = true
		}
	}
}

// Forget drops an account's history, as its removal does.
func (h *History) Forget(id string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, ok := h.accounts[id]; ok {
		delete(h.accounts, id)
		h.dirty = true
	}
}

// prune drops what is older than Retention.
func (h *History) prune() {
	h.mu.Lock()
	defer h.mu.Unlock()
	cutoff := h.now().Add(-Retention)
	for id, account := range h.accounts {
		keep := account.Buckets[:0]
		for _, b := range account.Buckets {
			if !b.Start.Before(cutoff.Truncate(BucketSize)) {
				keep = append(keep, b)
			}
		}
		if len(keep) != len(account.Buckets) {
			h.dirty = true
		}
		account.Buckets = keep
		account.Errors = slices.DeleteFunc(account.Errors, func(e ErrorSample) bool { return e.At.Before(cutoff) })
		if len(account.Buckets) == 0 && len(account.Errors) == 0 {
			delete(h.accounts, id)
			h.dirty = true
		}
	}
}

// Save writes the history if it changed since it was last written.
func (h *History) Save() error {
	if h.path == "" {
		return nil
	}
	h.prune()
	h.mu.Lock()
	if !h.dirty {
		h.mu.Unlock()
		return nil
	}
	data, err := json.Marshal(historyFile{Version: 1, Accounts: h.accounts}, json.Deterministic(true))
	h.dirty = false
	h.mu.Unlock()
	if err != nil {
		return err
	}
	if err := writeFileAtomic(h.path, data); err != nil {
		h.mu.Lock()
		h.dirty = true
		h.mu.Unlock()
		return err
	}
	return nil
}

// keep saves the history every interval until ctx ends, and once more then.
func (h *History) keep(ctx context.Context, interval time.Duration) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			_ = h.Save()
			return
		case <-ticker.C:
			_ = h.Save()
		}
	}
}

// errorMessage is the readable part of an upstream error body: the message
// of a JSON error when there is one, otherwise the text, on one line.
func errorMessage(body string) string {
	body = strings.TrimSpace(body)
	if body == "" {
		return ""
	}
	var decoded any
	if json.Unmarshal([]byte(body), &decoded) == nil {
		if message := findMessage(decoded, 0); message != "" {
			body = message
		}
	}
	body = strings.Join(strings.FieldsFunc(body, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }), " ")
	if utf8.RuneCountInString(body) > maxMessage {
		body = string([]rune(body)[:maxMessage]) + "…"
	}
	return body
}

// findMessage looks for the message of a JSON error, as providers nest it:
// {"error":{"type":...,"message":...}}, {"message":...}, {"error":"..."},
// {"detail":...}. The error's kind leads the message when it has one.
func findMessage(v any, depth int) string {
	if depth > 3 {
		return ""
	}
	switch v := v.(type) {
	case string:
		return v
	case map[string]any:
		for _, key := range []string{"message", "error_description", "detail", "error"} {
			found := findMessage(v[key], depth+1)
			if found == "" {
				continue
			}
			if key == "message" || key == "error_description" {
				for _, kindKey := range []string{"type", "status", "code"} {
					if kind, ok := v[kindKey].(string); ok && kind != "" && kind != "error" && !strings.Contains(found, kind) {
						return kind + ": " + found
					}
				}
			}
			return found
		}
	case []any:
		for _, item := range v {
			if found := findMessage(item, depth+1); found != "" {
				return found
			}
		}
	}
	return ""
}

func later(a, b time.Time) time.Time {
	if b.After(a) {
		return b.UTC()
	}
	return a
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v = strings.TrimSpace(v); v != "" {
			return v
		}
	}
	return ""
}
