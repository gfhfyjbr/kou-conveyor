package accounts

import (
	"cmp"
	"context"
	"encoding/json/v2"
	"fmt"
	"maps"
	"os"
	"regexp"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/usage"
)

// The Ledger is the gateway's other usage plugin beside History: History
// keeps each account's uptime, the Ledger what its requests used — hour by
// hour, for each account and model, the requests and their tokens by the
// kinds API prices tell apart: fresh input, cache reads, cache writes and
// output. Priced (prices.go), they are what the same requests would have
// cost as API credits. It is kept in usage.json beside the history, for
// longer than the history, so a month and a quarter can be looked back on.

const (
	// LedgerSlot is the span of time a row of the ledger counts.
	LedgerSlot = time.Hour
	// LedgerRetention is how long the ledger keeps what requests used.
	LedgerRetention = 92 * 24 * time.Hour
)

// Tokens counts tokens by the kinds API prices tell apart. The kinds do not
// overlap: Input is the input neither read from a cache nor written to one.
type Tokens struct {
	Input      int64 `json:"input"`
	CacheRead  int64 `json:"cache_read"`
	CacheWrite int64 `json:"cache_write"`
	Output     int64 `json:"output"`
	// Reasoning is the part of Output the model thought in.
	Reasoning int64 `json:"reasoning,omitzero"`
	// Other are tokens the provider did not say the kind of.
	Other int64 `json:"other,omitzero"`
}

// Total counts every token once.
func (t Tokens) Total() int64 { return t.Input + t.CacheRead + t.CacheWrite + t.Output + t.Other }

func (t *Tokens) add(o Tokens) {
	t.Input += o.Input
	t.CacheRead += o.CacheRead
	t.CacheWrite += o.CacheWrite
	t.Output += o.Output
	t.Reasoning += o.Reasoning
	t.Other += o.Other
}

// tokensOf sorts what a request used into the kinds, from the gateway's
// breakdown when it has one, and otherwise from what the provider reported.
func tokensOf(detail usage.Detail, provider, executor string) Tokens {
	detail = usage.EnsureTokenBreakdownForProvider(detail, provider, executor)
	b := detail.TokenBreakdown
	classified := b.Input.TotalTokens+b.Output.TotalTokens > 0
	if b.Valid() && b.Quality != usage.TokenAccountingQualityInconsistent && (classified || b.TotalTokens == 0) {
		return Tokens{
			Input: b.Input.UncachedTokens, CacheRead: b.Input.CacheReadTokens, CacheWrite: b.Input.CacheWriteTokens,
			Output: b.Output.TotalTokens, Reasoning: b.Output.ReasoningTokens, Other: b.UnclassifiedTokens,
		}
	}
	// A provider whose kinds the gateway does not know: its caches are
	// taken to be part of its input when they fit in it, as most APIs count
	// them, and apart from it otherwise.
	read, written := max(detail.CacheReadTokens, 0), max(detail.CacheCreationTokens, 0)
	if read == 0 && detail.CachedTokens > 0 && detail.CachedTokens != written {
		read = detail.CachedTokens
	}
	input := max(detail.InputTokens, 0)
	if read+written <= input {
		input -= read + written
	}
	output := max(detail.OutputTokens, detail.ReasoningTokens, 0)
	t := Tokens{Input: input, CacheRead: read, CacheWrite: written, Output: output, Reasoning: min(max(detail.ReasoningTokens, 0), output)}
	if rest := detail.TotalTokens - t.Total(); rest > 0 {
		t.Other = rest
	}
	return t
}

// tally is what one account's requests for one model used in one slot.
type tally struct {
	Requests int64
	Failed   int64
	// Latency is the total, in milliseconds, of the successful requests.
	Latency int64
	Tokens  Tokens
}

func (t *tally) add(o tally) {
	t.Requests += o.Requests
	t.Failed += o.Failed
	t.Latency += o.Latency
	t.Tokens.add(o.Tokens)
}

type ledgerKey struct {
	hour  int64 // Unix seconds
	auth  string
	model string
}

// ledgerAuth is what the ledger knows of an account or a key: the
// provider and index its requests came with, and the name people know it
// by, remembered for when it is gone.
type ledgerAuth struct {
	ID       string `json:"id"` // in usage.json
	Provider string `json:"p,omitzero"`
	Index    string `json:"i,omitzero"`
	Label    string `json:"l,omitzero"`
	Name     string `json:"n,omitzero"` // the provider's name, or the endpoint's kind
	Kind     string `json:"k,omitzero"` // "account", "endpoint" or "key"
}

// Ledger records what the gateway's requests used.
type Ledger struct {
	path string
	now  func() time.Time

	mu    sync.Mutex
	rows  map[ledgerKey]*tally
	auths map[string]*ledgerAuth // by the gateway's auth ID
	since time.Time              // the first request recorded
	dirty bool
}

// ledgerRow is a row of usage.json, short: a quarter of hours of several
// models adds up. Auth is the number of its account in the file's list,
// from 1; 0 for requests that reached none.
type ledgerRow struct {
	Hour       int64  `json:"h"`
	Auth       int    `json:"a,omitzero"`
	Model      string `json:"m,omitzero"`
	Requests   int64  `json:"n,omitzero"`
	Failed     int64  `json:"f,omitzero"`
	Latency    int64  `json:"ms,omitzero"`
	Input      int64  `json:"in,omitzero"`
	CacheRead  int64  `json:"cr,omitzero"`
	CacheWrite int64  `json:"cw,omitzero"`
	Output     int64  `json:"out,omitzero"`
	Reasoning  int64  `json:"rs,omitzero"`
	Other      int64  `json:"x,omitzero"`
}

type ledgerFile struct {
	Version int          `json:"version"`
	Since   time.Time    `json:"since,omitzero"`
	Auths   []ledgerAuth `json:"auths"`
	Rows    []ledgerRow  `json:"rows"`
}

// NewLedger reads the ledger kept at path; "" keeps it in memory only.
func NewLedger(path string) *Ledger {
	l := &Ledger{path: path, now: time.Now, rows: map[ledgerKey]*tally{}, auths: map[string]*ledgerAuth{}}
	if path == "" {
		return l
	}
	var saved ledgerFile
	if data, err := os.ReadFile(path); err == nil && json.Unmarshal(data, &saved) == nil && saved.Version == 1 {
		l.since = saved.Since
		for _, a := range saved.Auths {
			l.auths[a.ID] = &a
		}
		for _, r := range saved.Rows {
			if r.Auth < 0 || r.Auth > len(saved.Auths) {
				continue
			}
			auth := ""
			if r.Auth > 0 {
				auth = saved.Auths[r.Auth-1].ID
			}
			t := l.row(ledgerKey{hour: r.Hour, auth: auth, model: r.Model})
			t.add(tally{Requests: r.Requests, Failed: r.Failed, Latency: r.Latency, Tokens: Tokens{
				Input: r.Input, CacheRead: r.CacheRead, CacheWrite: r.CacheWrite, Output: r.Output, Reasoning: r.Reasoning, Other: r.Other,
			}})
		}
	}
	l.prune()
	return l
}

// row is the tally of a key, added if new; the caller holds l.mu, or owns l.
func (l *Ledger) row(key ledgerKey) *tally {
	t := l.rows[key]
	if t == nil {
		t = &tally{}
		l.rows[key] = t
	}
	return t
}

// HandleUsage records one request the gateway sent upstream.
func (l *Ledger) HandleUsage(_ context.Context, record usage.Record) {
	id := strings.TrimSpace(record.AuthID)
	tokens := tokensOf(record.Detail, record.Provider, record.ExecutorType)
	if id == "" && tokens.Total() == 0 {
		return // a request that reached no account, and used nothing
	}
	at := record.RequestedAt
	if at.IsZero() {
		at = l.now()
	}
	model := firstNonEmpty(record.Model, record.Alias, record.ResponseModel)
	one := tally{Requests: 1, Tokens: tokens}
	if record.Failed {
		one.Failed = 1
	} else {
		one.Latency = record.Latency.Milliseconds()
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	l.row(ledgerKey{hour: at.Truncate(LedgerSlot).Unix(), auth: id, model: model}).add(one)
	if id != "" {
		a := l.auths[id]
		if a == nil {
			a = &ledgerAuth{}
			l.auths[id] = a
		}
		if p := strings.ToLower(strings.TrimSpace(record.Provider)); p != "" {
			a.Provider = p
		}
		if record.AuthIndex != "" {
			a.Index = record.AuthIndex
		}
	}
	if l.since.IsZero() || at.Before(l.since) {
		l.since = at.UTC()
	}
	l.dirty = true
}

// learn remembers how people know an account or a key, found by its auth
// ID or index, for when it is gone. The ledger learns nothing of accounts
// whose requests it has not seen.
func (l *Ledger) learn(id, index string, a ledgerAuth) {
	l.mu.Lock()
	defer l.mu.Unlock()
	for candidate, known := range l.auths {
		if !(id != "" && candidate == id || index != "" && known.Index == index) {
			continue
		}
		if a.Label != "" && known.Label != a.Label || a.Name != "" && known.Name != a.Name || a.Kind != "" && known.Kind != a.Kind {
			known.Label = cmp.Or(a.Label, known.Label)
			known.Name = cmp.Or(a.Name, known.Name)
			known.Kind = cmp.Or(a.Kind, known.Kind)
			l.dirty = true
		}
	}
}

// prune drops what is older than LedgerRetention.
func (l *Ledger) prune() {
	l.mu.Lock()
	defer l.mu.Unlock()
	cutoff := l.now().Add(-LedgerRetention).Truncate(LedgerSlot).Unix()
	used := map[string]bool{}
	for key := range l.rows {
		if key.hour < cutoff {
			delete(l.rows, key)
			l.dirty = true
			continue
		}
		used[key.auth] = true
	}
	for id := range l.auths {
		if !used[id] {
			delete(l.auths, id)
			l.dirty = true
		}
	}
}

// Save writes the ledger if it changed since it was last written.
func (l *Ledger) Save() error {
	if l.path == "" {
		return nil
	}
	l.prune()
	l.mu.Lock()
	if !l.dirty {
		l.mu.Unlock()
		return nil
	}
	file := ledgerFile{Version: 1, Since: l.since, Auths: make([]ledgerAuth, 0, len(l.auths)), Rows: make([]ledgerRow, 0, len(l.rows))}
	number := map[string]int{"": 0}
	for _, id := range slices.Sorted(maps.Keys(l.auths)) {
		a := *l.auths[id]
		a.ID = id
		file.Auths = append(file.Auths, a)
		number[id] = len(file.Auths)
	}
	for key, t := range l.rows {
		file.Rows = append(file.Rows, ledgerRow{
			Hour: key.hour, Auth: number[key.auth], Model: key.model, Requests: t.Requests, Failed: t.Failed, Latency: t.Latency,
			Input: t.Tokens.Input, CacheRead: t.Tokens.CacheRead, CacheWrite: t.Tokens.CacheWrite, Output: t.Tokens.Output,
			Reasoning: t.Tokens.Reasoning, Other: t.Tokens.Other,
		})
	}
	slices.SortFunc(file.Rows, func(a, b ledgerRow) int {
		return cmp.Or(cmp.Compare(a.Hour, b.Hour), cmp.Compare(a.Auth, b.Auth), strings.Compare(a.Model, b.Model))
	})
	data, err := json.Marshal(file, json.Deterministic(true))
	l.dirty = false
	l.mu.Unlock()
	if err != nil {
		return err
	}
	if err := writeFileAtomic(l.path, data); err != nil {
		l.mu.Lock()
		l.dirty = true
		l.mu.Unlock()
		return err
	}
	return nil
}

// keep saves the ledger every interval until ctx ends, and once more then.
func (l *Ledger) keep(ctx context.Context, interval time.Duration) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			_ = l.Save()
			return
		case <-ticker.C:
			_ = l.Save()
		}
	}
}

// ---------------------------------------------------------------- spend

// Spend is what requests used, and what they would have cost at API
// prices, in US dollars. Unpriced counts the tokens of models without a
// price, which Cost leaves out.
type Spend struct {
	Requests int64   `json:"requests,omitzero"`
	Tokens   Tokens  `json:"tokens"`
	Cost     float64 `json:"cost"`
	Unpriced int64   `json:"unpriced_tokens,omitzero"`
}

func (s *Spend) add(o Spend) {
	s.Requests += o.Requests
	s.Tokens.add(o.Tokens)
	s.Cost += o.Cost
	s.Unpriced += o.Unpriced
}

// spending is what each account's and key's requests of a span came to.
type spending struct {
	byAuth map[string]Spend
	index  map[string]string // the auth index of each auth ID
}

// spendByAuth sums the ledger between from and to by account, each model
// at its price.
func (l *Ledger) spendByAuth(from, to time.Time, price func(model string) ModelPrice) spending {
	s := spending{byAuth: map[string]Spend{}, index: map[string]string{}}
	prices := map[string]ModelPrice{}
	l.mu.Lock()
	defer l.mu.Unlock()
	start, end := from.Truncate(LedgerSlot).Unix(), to.Unix()
	for key, t := range l.rows {
		if key.hour < start || key.hour >= end {
			continue
		}
		p, ok := prices[key.model]
		if !ok {
			p = price(key.model)
			prices[key.model] = p
		}
		spend := Spend{Requests: t.Requests, Tokens: t.Tokens}
		if p.Priced() {
			spend.Cost = p.Of(t.Tokens).Total()
		} else {
			spend.Unpriced = t.Tokens.Total()
		}
		sum := s.byAuth[key.auth]
		sum.add(spend)
		s.byAuth[key.auth] = sum
	}
	for id := range s.byAuth {
		if a := l.auths[id]; a != nil {
			s.index[id] = a.Index
		}
	}
	return s
}

// of sums the spend of the accounts with any of the IDs or auth indexes.
func (s spending) of(ids, indexes []string) Spend {
	var sum Spend
	for id, spend := range s.byAuth {
		if id != "" && slices.Contains(ids, id) || s.index[id] != "" && slices.Contains(indexes, s.index[id]) {
			sum.add(spend)
		}
	}
	return sum
}

// total sums the spend of every account.
func (s spending) total() Spend {
	var sum Spend
	for _, spend := range s.byAuth {
		sum.add(spend)
	}
	return sum
}

// ---------------------------------------------------------------- reports

// UsageRange is a span a report looks back on, in slots of equal length —
// days of the viewer's calendar for the longer spans.
type UsageRange struct {
	Slot  time.Duration // for slots shorter than a day
	Days  bool          // slots of a day each
	Slots int
}

// UsageRanges are the spans the cockpit reports usage over.
var UsageRanges = map[string]UsageRange{
	"24h": {Slot: time.Hour, Slots: 24},
	"7d":  {Slot: 4 * time.Hour, Slots: 42},
	"30d": {Days: true, Slots: 30},
	"90d": {Days: true, Slots: 90},
}

// UsageQuery asks for a report: over a range that ends with the slot in
// progress, in a time zone, of one provider, account or model if any.
type UsageQuery struct {
	Range    string
	Location *time.Location
	Filter   UsageFilter
}

// UsageFilter narrows a report down: to the requests of one provider, one
// account (by the gateway's auth ID) or one model.
type UsageFilter struct {
	Provider string `json:"provider,omitzero"`
	Account  string `json:"account,omitzero"`
	Model    string `json:"model,omitzero"`
}

// UsageTotals sum what requests used, and what it comes to.
type UsageTotals struct {
	Requests int64  `json:"requests"`
	Failed   int64  `json:"failed"`
	Tokens   Tokens `json:"tokens"`
	Cost     Cost   `json:"cost"`
	// Unpriced counts the tokens of models without a price, which Cost
	// leaves out.
	Unpriced int64 `json:"unpriced_tokens,omitzero"`
	// Saved is what caching saved: the cache reads at the input price, less
	// what they cost and what writing the caches cost beyond the input.
	Saved float64 `json:"cache_saved"`
	// Latency is the average of the successful requests, in milliseconds.
	Latency int64 `json:"avg_latency_ms,omitzero"`
	latency int64
}

func (u *UsageTotals) add(t tally, price ModelPrice) {
	u.Requests += t.Requests
	u.Failed += t.Failed
	u.latency += t.Latency
	u.Tokens.add(t.Tokens)
	if !price.Priced() {
		u.Unpriced += t.Tokens.Total()
		return
	}
	u.Cost.add(price.Of(t.Tokens))
	u.Saved += savedBy(t.Tokens, price.Price)
}

func (u *UsageTotals) finish() {
	if ok := u.Requests - u.Failed; ok > 0 {
		u.Latency = u.latency / ok
	}
}

// savedBy is what caching saved on tokens: what the cache reads would have
// cost as input, less what they and the cache writes' premium cost.
func savedBy(t Tokens, p Price) float64 {
	return (float64(t.CacheRead)*(p.Input-p.CacheRead) - float64(t.CacheWrite)*(p.CacheWrite-p.Input)) / 1e6
}

// UsageRow is what one account's requests for one model used in one slot.
type UsageRow struct {
	Slot     int    `json:"s"`
	Account  string `json:"a,omitzero"`
	Model    string `json:"m,omitzero"`
	Requests int64  `json:"n"`
	Failed   int64  `json:"f,omitzero"`
	Latency  int64  `json:"ms,omitzero"` // total of the successful requests
	Tokens   Tokens `json:"t"`
	Cost     Cost   `json:"c"`
}

// HeatCell is what the requests of one hour of the week came to.
type HeatCell struct {
	Requests int64   `json:"n"`
	Tokens   int64   `json:"t"`
	Cost     float64 `json:"c"`
}

// UsageFacet is what one provider, account or model came to over the
// range, whatever the filter.
type UsageFacet struct {
	ID       string  `json:"id"`
	Requests int64   `json:"requests"`
	Tokens   int64   `json:"tokens"`
	Cost     float64 `json:"cost"`
	Unpriced int64   `json:"unpriced_tokens,omitzero"`
}

// UsageFacets are the providers, accounts and models of a range, the most
// costly first, to filter a report by.
type UsageFacets struct {
	Providers []UsageFacet `json:"providers"`
	Accounts  []UsageFacet `json:"accounts"`
	Models    []UsageFacet `json:"models"`
}

// UsageModel is a model of a report, with its price.
type UsageModel struct {
	ID    string     `json:"id"`
	Name  string     `json:"name,omitzero"`
	Price ModelPrice `json:"price"`
	// Served: the gateway serves it now.
	Served bool `json:"served,omitzero"`
}

// UsageAccount is an account or key of a report, as people know it.
type UsageAccount struct {
	ID           string `json:"id"`
	Name         string `json:"name,omitzero"` // the credential's, for an account
	Label        string `json:"label"`
	Provider     string `json:"provider,omitzero"`
	ProviderName string `json:"provider_name,omitzero"`
	// Kind is "account", "endpoint" or "key"; empty for requests that
	// reached no account.
	Kind string `json:"kind,omitzero"`
	// Gone: the gateway no longer has it.
	Gone bool `json:"gone,omitzero"`
}

// UsageReport is what the gateway's requests used over a range, and what
// that comes to at API prices.
type UsageReport struct {
	Range string    `json:"range"`
	Zone  string    `json:"zone"`
	From  time.Time `json:"from"`
	To    time.Time `json:"to"`
	// Starts are the times the slots start; Slot their length in seconds,
	// 86400 for days, which a change of the clocks can make an hour off.
	Starts []time.Time `json:"starts"`
	Slot   int64       `json:"slot_seconds"`
	// Since is when the ledger began, or the oldest it keeps: nothing
	// before it was recorded.
	Since  time.Time   `json:"since,omitzero"`
	Filter UsageFilter `json:"filter"`
	Unit   Unit        `json:"unit"`
	// Totals of the range, and of the range as long before it.
	Totals   UsageTotals `json:"totals"`
	Previous UsageTotals `json:"previous"`
	Rows     []UsageRow  `json:"rows"`
	// Heat is the week's hours, Monday first: 7 × 24 cells.
	Heat     []HeatCell     `json:"heat"`
	Models   []UsageModel   `json:"models"`
	Accounts []UsageAccount `json:"accounts"`
	Facets   UsageFacets    `json:"facets"`
	// PricesError says why the user's prices could not be read.
	PricesError string `json:"prices_error,omitzero"`
}

// bounds are the times the slots of a range start, and the end of the
// last: the slot in progress now is the last.
func (r UsageRange) bounds(now time.Time, loc *time.Location) []time.Time {
	now = now.In(loc)
	out := make([]time.Time, r.Slots+1)
	y, m, d := now.Date()
	if r.Days {
		for i := range out {
			out[i] = time.Date(y, m, d+1+i-r.Slots, 0, 0, 0, 0, loc)
		}
		return out
	}
	midnight := time.Date(y, m, d, 0, 0, 0, 0, loc)
	end := midnight.Add(now.Sub(midnight).Truncate(r.Slot) + r.Slot)
	for i := range out {
		out[i] = end.Add(-time.Duration(r.Slots-i) * r.Slot)
	}
	return out
}

// Report sums the ledger over a range, pricing each model with price.
func (l *Ledger) Report(q UsageQuery, price func(model string) ModelPrice) (UsageReport, error) {
	spec, ok := UsageRanges[q.Range]
	if !ok {
		return UsageReport{}, fmt.Errorf("range is one of 24h, 7d, 30d or 90d, not %q", q.Range)
	}
	loc := q.Location
	if loc == nil {
		loc = time.UTC
	}
	now := l.now()
	bounds := spec.bounds(now, loc)
	from, to := bounds[0], bounds[len(bounds)-1]
	before := from.Add(-to.Sub(from))
	slot := spec.Slot
	if spec.Days {
		slot = 24 * time.Hour
	}
	report := UsageReport{
		Range: q.Range, Zone: loc.String(), From: from.UTC(), To: to.UTC(), Slot: int64(slot / time.Second),
		Filter: q.Filter, Unit: Dollars, Heat: make([]HeatCell, 7*24),
	}
	for _, b := range bounds[:len(bounds)-1] {
		report.Starts = append(report.Starts, b.UTC())
	}

	l.mu.Lock()
	type entry struct {
		key      ledgerKey
		tally    tally
		provider string
	}
	var entries []entry
	for key, t := range l.rows {
		if key.hour < before.Unix() || key.hour >= to.Unix() {
			continue
		}
		provider := ""
		if a := l.auths[key.auth]; a != nil {
			provider = a.Provider
		}
		entries = append(entries, entry{key: key, tally: *t, provider: provider})
	}
	since := l.since
	l.mu.Unlock()
	if !since.IsZero() {
		report.Since = since.UTC()
		if oldest := now.Add(-LedgerRetention).Truncate(LedgerSlot); since.Before(oldest) {
			report.Since = oldest.UTC()
		}
	}

	prices := map[string]ModelPrice{}
	priceOf := func(model string) ModelPrice {
		p, ok := prices[model]
		if !ok {
			p = price(model)
			prices[model] = p
		}
		return p
	}
	type rowKey struct {
		slot        int
		auth, model string
	}
	rows := map[rowKey]*tally{}
	facets := [3]map[string]*UsageFacet{{}, {}, {}}
	facet := func(kind int, id string, t tally, p ModelPrice) {
		f := facets[kind][id]
		if f == nil {
			f = &UsageFacet{ID: id}
			facets[kind][id] = f
		}
		f.Requests += t.Requests
		f.Tokens += t.Tokens.Total()
		if p.Priced() {
			f.Cost += p.Of(t.Tokens).Total()
		} else {
			f.Unpriced += t.Tokens.Total()
		}
	}
	matches := func(e entry) bool {
		f := q.Filter
		return (f.Provider == "" || e.provider == f.Provider) && (f.Account == "" || e.key.auth == f.Account) && (f.Model == "" || e.key.model == f.Model)
	}
	for _, e := range entries {
		at := time.Unix(e.key.hour, 0)
		p := priceOf(e.key.model)
		if at.Before(from) {
			if matches(e) {
				report.Previous.add(e.tally, p)
			}
			continue
		}
		facet(0, e.provider, e.tally, p)
		facet(1, e.key.auth, e.tally, p)
		facet(2, e.key.model, e.tally, p)
		if !matches(e) {
			continue
		}
		report.Totals.add(e.tally, p)
		n := sort.Search(len(bounds), func(i int) bool { return bounds[i].After(at) }) - 1
		n = min(max(n, 0), spec.Slots-1)
		key := rowKey{slot: n, auth: e.key.auth, model: e.key.model}
		if rows[key] == nil {
			rows[key] = &tally{}
		}
		rows[key].add(e.tally)
		local := at.In(loc)
		cell := &report.Heat[(int(local.Weekday())+6)%7*24+local.Hour()]
		cell.Requests += e.tally.Requests
		cell.Tokens += e.tally.Tokens.Total()
		if p.Priced() {
			cell.Cost += p.Of(e.tally.Tokens).Total()
		}
	}
	report.Totals.finish()
	report.Previous.finish()

	report.Rows = make([]UsageRow, 0, len(rows))
	for key, t := range rows {
		row := UsageRow{Slot: key.slot, Account: key.auth, Model: key.model, Requests: t.Requests, Failed: t.Failed, Latency: t.Latency, Tokens: t.Tokens}
		if p := priceOf(key.model); p.Priced() {
			row.Cost = p.Of(t.Tokens)
		}
		report.Rows = append(report.Rows, row)
	}
	slices.SortFunc(report.Rows, func(a, b UsageRow) int {
		return cmp.Or(cmp.Compare(a.Slot, b.Slot), strings.Compare(a.Account, b.Account), strings.Compare(a.Model, b.Model))
	})
	list := func(m map[string]*UsageFacet) []UsageFacet {
		out := make([]UsageFacet, 0, len(m))
		for _, f := range m {
			out = append(out, *f)
		}
		slices.SortFunc(out, func(a, b UsageFacet) int {
			return cmp.Or(cmp.Compare(b.Cost, a.Cost), cmp.Compare(b.Tokens, a.Tokens), cmp.Compare(b.Requests, a.Requests), strings.Compare(a.ID, b.ID))
		})
		return out
	}
	report.Facets = UsageFacets{Providers: list(facets[0]), Accounts: list(facets[1]), Models: list(facets[2])}
	// The models of the range, and the one filtered by, with their prices.
	seen := map[string]bool{}
	for _, f := range report.Facets.Models {
		seen[f.ID] = true
		report.Models = append(report.Models, UsageModel{ID: f.ID, Price: priceOf(f.ID)})
	}
	if m := q.Filter.Model; m != "" && !seen[m] {
		report.Models = append(report.Models, UsageModel{ID: m, Price: priceOf(m)})
	}
	return report, nil
}

// accountsOf describes the accounts of a report from what the ledger
// remembers of them; the host puts in what the gateway has now.
func (l *Ledger) accountsOf(ids []string) []UsageAccount {
	l.mu.Lock()
	defer l.mu.Unlock()
	out := make([]UsageAccount, 0, len(ids))
	for _, id := range ids {
		a := UsageAccount{ID: id}
		if known := l.auths[id]; known != nil {
			a.Provider, a.Label, a.ProviderName, a.Kind = known.Provider, known.Label, known.Name, known.Kind
		}
		if a.Label == "" {
			a.Label = labelOf(id)
		}
		if a.ProviderName == "" && a.Provider != "" {
			a.ProviderName = ProviderName(a.Provider)
		}
		out = append(out, a)
	}
	return out
}

// credentialName is how the gateway names a credential file: the provider,
// sometimes a hash, and the account's e-mail address.
var credentialName = regexp.MustCompile(`^(?:(?:claude|codex|antigravity|xai|kimi|kimi-ai|devin|meta|gemini|gemini-cli|qwen|iflow|vertex|aistudio)-)?(?:[0-9a-f]{6,}-)?([^\s/:@]+@[^\s/:@]+\.[a-z]{2,})$`)

// labelOf names an account by its ID when nothing else does: the e-mail
// address a credential's file name holds, or the ID without its ".json".
func labelOf(id string) string {
	if id == "" {
		return "No account"
	}
	name := strings.TrimSuffix(id, ".json")
	if m := credentialName.FindStringSubmatch(strings.ToLower(name)); m != nil {
		return m[1]
	}
	return name
}
