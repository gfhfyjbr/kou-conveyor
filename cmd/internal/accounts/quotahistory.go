package accounts

import (
	"context"
	"encoding/json/v2"
	"errors"
	"maps"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	// quotaHistoryInterval is how often the quotas of the accounts in use
	// are written down.
	quotaHistoryInterval = 15 * time.Minute
	// quotaHistoryLimit bounds quota-history.jsonl: a file that would grow
	// past it becomes quota-history.1.jsonl, replacing the one before.
	quotaHistoryLimit = 2 << 20
)

// QuotaSnapshot is an account's quota at one time: a line of
// quota-history.jsonl.
type QuotaSnapshot struct {
	At       time.Time `json:"at"`
	Account  string    `json:"account"`
	Provider string    `json:"provider"`
	Quota    Quota     `json:"quota"`
}

// quotaHistory writes down the quotas of the accounts that serve requests.
// How much of a subscription's windows a long run spends is known only if
// the windows are written down as it goes, whether a page of the cockpit is
// open or not.
type quotaHistory struct {
	path  string
	limit int64
	// since is when the history started: an account last used before is
	// written down once it serves a request again.
	since time.Time
	// seen is each account's latest request as of its latest snapshot.
	seen map[string]time.Time
}

func newQuotaHistory(path string, now time.Time) *quotaHistory {
	return &quotaHistory{path: path, limit: quotaHistoryLimit, since: now, seen: map[string]time.Time{}}
}

// record writes down the quota of each account of the gateway's list that
// served a request since it was last written down. lastUsed tells when an
// account's latest request was; live asks the provider for its quota, and
// the headers of the account's latest response stand in when the provider
// cannot be asked.
func (q *quotaHistory) record(ctx context.Context, now time.Time, records []record, lastUsed func(id, index string) time.Time, live func(Account) Quota) error {
	var lines []byte
	written := map[string]time.Time{}
	for _, r := range records {
		a := accountOf(r, now)
		if !a.QuotaSupported || a.State == StateDisabled {
			continue
		}
		used := lastUsed(a.ID, a.Index)
		seen, ok := q.seen[a.Name]
		if !ok {
			seen = q.since
		}
		if !used.After(seen) {
			continue
		}
		quota := live(a)
		if ctx.Err() != nil {
			break
		}
		if observed, ok := observedQuota(a.Provider, r.obj("quota")); ok && quota.Error != "" {
			quota = observed
		}
		line, err := json.Marshal(QuotaSnapshot{At: now.UTC(), Account: a.Name, Provider: a.Provider, Quota: quota})
		if err != nil {
			return err
		}
		lines = append(append(lines, line...), '\n')
		written[a.Name] = used
	}
	if err := appendBounded(q.path, lines, q.limit); err != nil {
		return err
	}
	maps.Copy(q.seen, written)
	return nil
}

// keepQuotas writes down, every interval until ctx ends, the quota of each
// account that served a request since it was last written down, in
// quota-history.jsonl beside the history.
func (h *Host) keepQuotas(ctx context.Context, interval time.Duration) {
	history := newQuotaHistory(filepath.Join(h.opt.Dir, "quota-history.jsonl"), time.Now())
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		client, err := h.running()
		if err != nil {
			continue
		}
		records, err := client.authFiles(ctx)
		if err != nil {
			continue
		}
		_ = history.record(ctx, time.Now(), records, h.history.LastUsed, func(a Account) Quota {
			return h.quotas.get(ctx, client, a.target, false)
		})
	}
}

// appendBounded appends data to the file at path. A file that would grow
// past limit first becomes the previous one, named with ".1" before its
// extension, so the two never hold more than twice limit.
func appendBounded(path string, data []byte, limit int64) error {
	if len(data) == 0 {
		return nil
	}
	if info, err := os.Stat(path); err == nil && info.Size() > 0 && info.Size()+int64(len(data)) > limit {
		extension := filepath.Ext(path)
		if err := os.Rename(path, strings.TrimSuffix(path, extension)+".1"+extension); err != nil {
			return err
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	_, err = file.Write(data)
	return errors.Join(err, file.Close())
}
