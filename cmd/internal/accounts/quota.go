package accounts

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"math"
	"net/http"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Quota sources.
const (
	// SourceLive is a quota the provider was asked for.
	SourceLive = "live"
	// SourceObserved is a quota read from the headers of the account's
	// latest response, which the gateway keeps.
	SourceObserved = "observed"
)

// Quota is what is left of an account's rate limits.
type Quota struct {
	Source  string    `json:"source"`
	At      time.Time `json:"at"`
	Plan    string    `json:"plan,omitzero"`
	Windows []Window  `json:"windows,omitzero"`
	// Notes are facts beside the windows, such as a credit balance.
	Notes []string `json:"notes,omitzero"`
	// Error says why the provider could not be asked; Status is the HTTP
	// status it answered with.
	Error  string `json:"error,omitzero"`
	Status int    `json:"status,omitzero"`
}

// Window is one limit of an account.
type Window struct {
	Label string `json:"label"`
	// Used is the share of the limit spent, from 0 to 100.
	Used     float64   `json:"used"`
	ResetsAt time.Time `json:"resets_at,omitzero"`
	// Amount is the use in the limit's own units, such as "120 / 500".
	Amount string `json:"amount,omitzero"`
	// Reached is set when the provider says the limit stops requests.
	Reached bool `json:"reached,omitzero"`
}

// target is what a quota fetch knows of an account.
type target struct {
	name, index, provider string
	// chatgptAccount is a Codex account's ChatGPT workspace.
	chatgptAccount string
	project        string // an Antigravity account's Google Cloud project
}

type fetcher func(ctx context.Context, c *Client, t target) (Quota, error)

var fetchers = map[string]fetcher{
	"codex":       fetchCodex,
	"claude":      fetchClaude,
	"antigravity": fetchAntigravity,
	"kimi":        fetchKimi("https://api.kimi.com/coding/v1/usages"),
	"kimi-ai":     fetchKimi("https://api.kimi.ai/coding/v1/usages"),
	"xai":         fetchXAI,
}

// quotaSupported reports whether the cockpit can ask provider for quotas.
func quotaSupported(provider string) bool {
	_, ok := fetchers[strings.ToLower(provider)]
	return ok
}

// ---------------------------------------------------------------- cache

const (
	quotaFresh      = 3 * time.Minute // how long a fetched quota is shown as is
	quotaRetryAfter = time.Minute     // how long a failed fetch is not repeated
	quotaParallel   = 4
)

// quotas remembers the latest quota of each account and fetches at most a
// few at a time, each account's once however many ask.
type quotas struct {
	mu       sync.Mutex
	entries  map[string]Quota
	inflight map[string]chan struct{}
	slots    chan struct{}
}

func newQuotas() *quotas {
	return &quotas{entries: map[string]Quota{}, inflight: map[string]chan struct{}{}, slots: make(chan struct{}, quotaParallel)}
}

// cached is the latest quota of an account, if any.
func (q *quotas) cached(name string) (Quota, bool) {
	q.mu.Lock()
	defer q.mu.Unlock()
	quota, ok := q.entries[name]
	return quota, ok
}

func (q *quotas) forget(name string) {
	q.mu.Lock()
	defer q.mu.Unlock()
	delete(q.entries, name)
}

// get returns an account's quota, asking the provider unless a recent
// answer will do.
func (q *quotas) get(ctx context.Context, c *Client, t target, refresh bool) Quota {
	for {
		q.mu.Lock()
		entry, ok := q.entries[t.name]
		fresh := ok && time.Since(entry.At) < quotaFresh
		if ok && entry.Error != "" {
			fresh = time.Since(entry.At) < quotaRetryAfter
		}
		if fresh && !refresh {
			q.mu.Unlock()
			return entry
		}
		wait, busy := q.inflight[t.name]
		if !busy {
			done := make(chan struct{})
			q.inflight[t.name] = done
			q.mu.Unlock()
			quota := q.fetch(ctx, c, t)
			q.mu.Lock()
			if ctx.Err() == nil {
				q.entries[t.name] = quota
			}
			delete(q.inflight, t.name)
			close(done)
			q.mu.Unlock()
			return quota
		}
		q.mu.Unlock()
		select {
		case <-wait:
			refresh = false // the fetch that just ended is fresh enough
		case <-ctx.Done():
			return Quota{Source: SourceLive, At: time.Now().UTC(), Error: ctx.Err().Error()}
		}
	}
}

func (q *quotas) fetch(ctx context.Context, c *Client, t target) Quota {
	select {
	case q.slots <- struct{}{}:
		defer func() { <-q.slots }()
	case <-ctx.Done():
		return Quota{Source: SourceLive, At: time.Now().UTC(), Error: ctx.Err().Error()}
	}
	ctx, cancel := context.WithTimeout(ctx, 45*time.Second)
	defer cancel()
	fetch := fetchers[strings.ToLower(t.provider)]
	if fetch == nil {
		return Quota{Source: SourceLive, At: time.Now().UTC(), Error: "the provider does not report quotas"}
	}
	quota, err := fetch(ctx, c, t)
	quota.Source, quota.At = SourceLive, time.Now().UTC()
	if err != nil {
		quota.Error = err.Error()
		var failed *fetchError
		if errors.As(err, &failed) {
			quota.Status = failed.status
		}
	}
	return quota
}

// fetchError is a provider's refusal to tell a quota.
type fetchError struct {
	status  int
	message string
}

func (e *fetchError) Error() string {
	switch {
	case e.message != "" && e.status != 0:
		return fmt.Sprintf("%d: %s", e.status, e.message)
	case e.message != "":
		return e.message
	default:
		return fmt.Sprintf("the provider answered %d %s", e.status, http.StatusText(e.status))
	}
}

// ask sends a request for an account through the gateway and decodes the
// provider's JSON answer.
func ask(ctx context.Context, c *Client, t target, method, url string, header map[string]string, data string) (map[string]any, http.Header, error) {
	answer, err := c.call(ctx, apiCall{AuthIndex: t.index, Method: method, URL: url, Header: header, Data: data})
	if err != nil {
		return nil, nil, err
	}
	headers := http.Header(answer.Header)
	if answer.StatusCode < 200 || answer.StatusCode > 299 {
		return nil, headers, &fetchError{status: answer.StatusCode, message: errorMessage(answer.Body)}
	}
	var body map[string]any
	if err := json.Unmarshal([]byte(answer.Body), &body); err != nil {
		return nil, headers, fmt.Errorf("the provider's answer is not JSON: %w", err)
	}
	return body, headers, nil
}

// ---------------------------------------------------------------- Codex

const codexUserAgent = "codex_cli_rs/0.149.1 (Mac OS 26.5.2; arm64) kou-conveyor"

func fetchCodex(ctx context.Context, c *Client, t target) (Quota, error) {
	header := map[string]string{"Authorization": "Bearer $TOKEN$", "Accept": "application/json", "User-Agent": codexUserAgent}
	if t.chatgptAccount != "" {
		header["Chatgpt-Account-Id"] = t.chatgptAccount
	}
	body, _, err := ask(ctx, c, t, http.MethodGet, "https://chatgpt.com/backend-api/wham/usage", header, "")
	if err != nil {
		return Quota{}, err
	}
	return codexQuota(record(body), time.Now()), nil
}

// codexQuota reads the answer of ChatGPT's usage endpoint: a rate limit
// with a primary and a secondary window (5 hours and a week, or a month for
// some workspaces), the same for code review, limits of their own for some
// models, and credits.
func codexQuota(body record, now time.Time) Quota {
	quota := Quota{Plan: planName(body.str("plan_type"))}
	add := func(prefix string, limit record) {
		if limit == nil {
			return
		}
		reached := limit.flag("limit_reached") || (limit["allowed"] == false)
		for _, key := range []string{"primary_window", "secondary_window"} {
			w := limit.obj(key)
			if w == nil {
				continue
			}
			window := Window{Label: prefix + windowLabel(w.num("limit_window_seconds")), Reached: reached}
			window.Used = clampPercent(w.num("used_percent"))
			if _, ok := w["used_percent"]; !ok && reached {
				window.Used = 100
			}
			window.ResetsAt = w.when("reset_at")
			if window.ResetsAt.IsZero() && w.num("reset_after_seconds") > 0 {
				window.ResetsAt = now.Add(time.Duration(w.num("reset_after_seconds")) * time.Second).UTC()
			}
			quota.Windows = append(quota.Windows, window)
		}
	}
	add("", body.obj("rate_limit"))
	add("Code review · ", body.obj("code_review_rate_limit"))
	for _, item := range body.list("additional_rate_limits") {
		extra, _ := item.(map[string]any)
		name := firstNonEmpty(record(extra).str("limit_name"), record(extra).str("metered_feature"))
		if name == "" {
			continue
		}
		add(name+" · ", record(extra).obj("rate_limit"))
	}
	if credits := body.obj("credits"); credits != nil {
		switch {
		case credits.flag("unlimited"):
			quota.Notes = append(quota.Notes, "Credits: unlimited")
		case credits.flag("has_credits") || credits.num("balance") > 0:
			quota.Notes = append(quota.Notes, "Credits: "+strings.TrimSpace(credits.str("balance")))
		}
	}
	return quota
}

// ---------------------------------------------------------------- Claude

var claudeWindows = []struct{ key, label string }{
	{"five_hour", "5h"},
	{"seven_day", "Weekly"},
	{"seven_day_opus", "Weekly · Opus"},
	{"seven_day_sonnet", "Weekly · Sonnet"},
	{"iguana_necktie", "Weekly · Fable"},
	{"seven_day_oauth_apps", "Weekly · apps"},
	{"seven_day_cowork", "Weekly · Cowork"},
}

func fetchClaude(ctx context.Context, c *Client, t target) (Quota, error) {
	header := map[string]string{"Authorization": "Bearer $TOKEN$", "Content-Type": "application/json", "anthropic-beta": "oauth-2025-04-20"}
	body, _, err := ask(ctx, c, t, http.MethodGet, "https://api.anthropic.com/api/oauth/usage", header, "")
	if err != nil {
		return Quota{}, err
	}
	quota := claudeQuota(record(body))
	if profile, _, err := ask(ctx, c, t, http.MethodGet, "https://api.anthropic.com/api/oauth/profile", header, ""); err == nil {
		quota.Plan = claudePlan(record(profile))
	}
	return quota, nil
}

// claudeQuota reads the answer of Claude's usage endpoint: a utilization
// in percent and a reset time for each window the subscription has, and
// extra usage when it is on.
func claudeQuota(body record) Quota {
	var quota Quota
	scoped := map[string]bool{}
	for _, item := range body.list("limits") {
		limit := record(asMap(item))
		name := record(asMap(record(asMap(limit["scope"]))["model"])).str("display_name")
		if limit.str("kind") != "weekly_scoped" || name == "" || limit["percent"] == nil {
			continue
		}
		scoped[strings.ToLower(name)] = true
		quota.Windows = append(quota.Windows, Window{Label: "Weekly · " + name, Used: clampPercent(limit.num("percent")), ResetsAt: limit.when("resets_at")})
	}
	for _, w := range claudeWindows {
		window := body.obj(w.key)
		if window == nil || window["utilization"] == nil {
			continue
		}
		if w.key == "iguana_necktie" && (scoped["fable"] || scoped["fable 5"]) {
			continue
		}
		quota.Windows = append(quota.Windows, Window{Label: w.label, Used: clampPercent(window.num("utilization")), ResetsAt: window.when("resets_at")})
	}
	// The scoped limits were read first to recognize the Fable window; they
	// follow the shared ones.
	slices.SortStableFunc(quota.Windows, func(a, b Window) int { return claudeRank(a.Label) - claudeRank(b.Label) })
	if extra := body.obj("extra_usage"); extra != nil && extra.flag("is_enabled") && extra["utilization"] != nil {
		quota.Windows = append(quota.Windows, Window{Label: "Extra usage", Used: clampPercent(extra.num("utilization"))})
	}
	return quota
}

func claudeRank(label string) int {
	for i, w := range claudeWindows {
		if w.label == label {
			return i
		}
	}
	return len(claudeWindows)
}

func claudePlan(profile record) string {
	org, account := profile.obj("organization"), profile.obj("account")
	switch {
	case strings.EqualFold(org.str("organization_type"), "claude_team"):
		return "Team"
	case account.flag("has_claude_max"):
		return "Max"
	case account.flag("has_claude_pro"):
		return "Pro"
	case account != nil && account["has_claude_max"] == false && account["has_claude_pro"] == false:
		return "Free"
	}
	return ""
}

// ---------------------------------------------------------------- Antigravity

var antigravityQuotaURLs = []string{
	"https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
	"https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:retrieveUserQuotaSummary",
	"https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
}

func fetchAntigravity(ctx context.Context, c *Client, t target) (Quota, error) {
	if t.project == "" {
		return Quota{}, errors.New("the account has no Google Cloud project yet; it gets one with its first request")
	}
	header := map[string]string{
		"Authorization": "Bearer $TOKEN$", "Content-Type": "application/json",
		"User-Agent": "antigravity/cli/1.0.13 (aidev_client; os_type=darwin; arch=arm64)",
	}
	data := `{"project":` + strconv.Quote(t.project) + `}`
	var last error
	for _, url := range antigravityQuotaURLs {
		body, _, err := ask(ctx, c, t, http.MethodPost, url, header, data)
		if err != nil {
			last = err
			if ctx.Err() != nil {
				break
			}
			continue
		}
		return antigravityQuota(record(body)), nil
	}
	return Quota{}, last
}

// antigravityQuota reads groups of models, each with buckets that say what
// share of a window is left.
func antigravityQuota(body record) Quota {
	var quota Quota
	for i, item := range body.list("groups") {
		group := record(asMap(item))
		name := firstNonEmpty(group.str("displayName"), group.str("display_name"), fmt.Sprintf("Group %d", i+1))
		for _, raw := range group.list("buckets") {
			bucket := record(asMap(raw))
			remaining, ok := bucket["remainingFraction"]
			if !ok {
				remaining, ok = bucket["remaining_fraction"]
			}
			if !ok {
				continue
			}
			fraction := record{"v": remaining}.num("v")
			label := name + " · " + firstNonEmpty(antigravityWindow(bucket.str("window")), bucket.str("displayName"), bucket.str("display_name"))
			quota.Windows = append(quota.Windows, Window{
				Label: strings.TrimSuffix(label, " · "), Used: clampPercent((1 - fraction) * 100),
				ResetsAt: parseTime(firstNonEmpty(bucket.str("resetTime"), bucket.str("reset_time"))),
			})
		}
	}
	return quota
}

func antigravityWindow(window string) string {
	switch strings.ToLower(strings.TrimSpace(window)) {
	case "5h", "five-hour", "five_hour":
		return "5h"
	case "weekly", "week":
		return "Weekly"
	case "daily", "day":
		return "Daily"
	}
	return window
}

// ---------------------------------------------------------------- Kimi

func fetchKimi(url string) fetcher {
	return func(ctx context.Context, c *Client, t target) (Quota, error) {
		body, _, err := ask(ctx, c, t, http.MethodGet, url, map[string]string{"Authorization": "Bearer $TOKEN$"}, "")
		if err != nil {
			return Quota{}, err
		}
		return kimiQuota(record(body), time.Now()), nil
	}
}

// kimiQuota reads Kimi's usages: limits counted in requests, each over a
// window of some duration, and the membership's weekly total.
func kimiQuota(body record, now time.Time) Quota {
	var quota Quota
	row := func(label string, detail record) {
		limit := detail.num("limit")
		used := detail.num("used")
		if _, ok := detail["used"]; !ok {
			used = limit - detail.num("remaining")
		}
		if limit <= 0 {
			return
		}
		window := Window{Label: label, Used: clampPercent(used / limit * 100), Amount: fmt.Sprintf("%s / %s", count(used), count(limit))}
		window.ResetsAt = parseTime(firstNonEmpty(detail.str("reset_at"), detail.str("resetAt"), detail.str("reset_time"), detail.str("resetTime")))
		if window.ResetsAt.IsZero() {
			if in := firstNonZero(detail.num("reset_in"), detail.num("resetIn"), detail.num("ttl")); in > 0 {
				window.ResetsAt = now.Add(time.Duration(in) * time.Second).UTC()
			}
		}
		quota.Windows = append(quota.Windows, window)
	}
	for i, item := range body.list("limits") {
		limit := record(asMap(item))
		detail := limit.obj("detail")
		if detail == nil {
			detail = limit
		}
		w := limit.obj("window")
		label := firstNonEmpty(limit.str("name"), limit.str("title"), limit.str("scope"))
		if label == "" {
			if d := firstNonZero(w.num("duration"), limit.num("duration")); d > 0 {
				label = kimiDuration(d, firstNonEmpty(w.str("timeUnit"), limit.str("timeUnit")))
			} else {
				label = fmt.Sprintf("Limit %d", i+1)
			}
		}
		row(label, detail)
	}
	if usage := body.obj("usage"); usage != nil {
		row("Weekly", usage)
	}
	return quota
}

func kimiDuration(duration float64, unit string) string {
	switch strings.TrimPrefix(strings.ToUpper(unit), "TIME_UNIT_") {
	case "SECOND", "SECONDS":
		return windowLabel(duration)
	case "HOUR", "HOURS":
		return windowLabel(duration * 3600)
	case "DAY", "DAYS":
		return windowLabel(duration * 86400)
	case "WEEK", "WEEKS":
		return windowLabel(duration * 7 * 86400)
	default: // minutes
		return windowLabel(duration * 60)
	}
}

// ---------------------------------------------------------------- Grok

func fetchXAI(ctx context.Context, c *Client, t target) (Quota, error) {
	header := map[string]string{
		"Authorization": "Bearer $TOKEN$", "x-xai-token-auth": "xai-grok-cli", "x-grok-client-version": "0.2.91",
		"accept": "*/*", "user-agent": "grok-pager/0.2.91 grok-shell/0.2.91 (macos; aarch64)",
	}
	// The gateway's account list leaves the user ID out; the credential
	// has it, and the billing service wants it.
	if file, err := c.download(ctx, t.name); err == nil {
		if sub := firstNonEmpty(record(file).str("sub"), record(file).str("subject"), record(file).str("user_id")); sub != "" {
			header["x-userid"] = sub
		}
	}
	weekly, _, errWeekly := ask(ctx, c, t, http.MethodGet, "https://cli-chat-proxy.grok.com/v1/billing?format=credits", header, "")
	monthly, _, errMonthly := ask(ctx, c, t, http.MethodGet, "https://cli-chat-proxy.grok.com/v1/billing", header, "")
	if errWeekly != nil && errMonthly != nil {
		return Quota{}, errWeekly
	}
	return xaiQuota(record(weekly).obj("config"), record(monthly).obj("config")), nil
}

// xaiQuota reads Grok's billing: credits over the current period, often a
// week, their use by product, and a monthly spending limit.
func xaiQuota(weekly, monthly record) Quota {
	var quota Quota
	if weekly != nil && weekly["creditUsagePercent"] != nil {
		period := weekly.obj("currentPeriod")
		label := "Credits"
		if kind := strings.ToLower(period.str("type")); strings.Contains(kind, "week") {
			label = "Weekly credits"
		} else if strings.Contains(kind, "month") {
			label = "Monthly credits"
		}
		quota.Windows = append(quota.Windows, Window{Label: label, Used: clampPercent(weekly.num("creditUsagePercent")), ResetsAt: period.when("end")})
		for _, item := range weekly.list("productUsage") {
			product := record(asMap(item))
			if name := product.str("product"); name != "" && product["usagePercent"] != nil {
				quota.Windows = append(quota.Windows, Window{Label: name, Used: clampPercent(product.num("usagePercent")), ResetsAt: period.when("end")})
			}
		}
	}
	source := monthly
	if source == nil {
		source = weekly
	}
	if source != nil {
		limit, used := cents(source["monthlyLimit"]), cents(source["used"])
		if limit > 0 {
			quota.Windows = append(quota.Windows, Window{
				Label: "Monthly", Used: clampPercent(math.Min(used, limit) / limit * 100),
				Amount:   fmt.Sprintf("$%.2f / $%.2f", math.Min(used, limit)/100, limit/100),
				ResetsAt: source.when("billingPeriodEnd"),
			})
		}
	}
	return quota
}

// cents reads an amount of cents, bare or as {"val": n}.
func cents(v any) float64 {
	if m, ok := v.(map[string]any); ok {
		v = m["val"]
	}
	return record{"v": v}.num("v")
}

// ---------------------------------------------------------------- observed

// observedQuota reads the rate-limit headers of an account's latest
// response, which the gateway keeps for Codex and Claude accounts.
func observedQuota(provider string, observation record) (Quota, bool) {
	signals := observation.obj("signals")
	if len(signals) == 0 {
		return Quota{}, false
	}
	at := observation.when("observed_at")
	get := func(name string) string {
		for key := range signals {
			if strings.EqualFold(key, name) {
				return signals.str(key)
			}
		}
		return ""
	}
	number := func(name string) (float64, bool) {
		f, err := strconv.ParseFloat(get(name), 64)
		return f, err == nil && !math.IsNaN(f) && !math.IsInf(f, 0)
	}
	quota := Quota{Source: SourceObserved, At: at}
	switch strings.ToLower(provider) {
	case "codex":
		quota.Plan = planName(get("X-Codex-Plan-Type"))
		reached := strings.EqualFold(get("X-Codex-Limit-Reached"), "true")
		for _, name := range []string{"Primary", "Secondary"} {
			used, ok := number("X-Codex-" + name + "-Used-Percent")
			if !ok {
				continue
			}
			minutes, _ := number("X-Codex-" + name + "-Window-Minutes")
			window := Window{Label: windowLabel(minutes * 60), Used: clampPercent(used), Reached: reached && used >= 100}
			if resetAt, ok := number("X-Codex-" + name + "-Reset-At"); ok && resetAt > 0 {
				window.ResetsAt = parseTime(resetAt)
			} else if after, ok := number("X-Codex-" + name + "-Reset-After-Seconds"); ok && !at.IsZero() {
				window.ResetsAt = at.Add(time.Duration(after) * time.Second)
			}
			quota.Windows = append(quota.Windows, window)
		}
		if balance := get("X-Codex-Credits-Balance"); balance != "" && !strings.EqualFold(get("X-Codex-Credits-Has-Credits"), "false") {
			quota.Notes = append(quota.Notes, "Credits: "+balance)
		}
	case "claude":
		for _, w := range []struct{ key, label string }{{"5h", "5h"}, {"7d", "Weekly"}} {
			utilization, ok := number("Anthropic-Ratelimit-Unified-" + w.key + "-Utilization")
			if !ok {
				continue
			}
			window := Window{Label: w.label, Used: clampPercent(utilization * 100)}
			window.Reached = strings.EqualFold(get("Anthropic-Ratelimit-Unified-"+w.key+"-Status"), "rejected")
			if reset, ok := number("Anthropic-Ratelimit-Unified-" + w.key + "-Reset"); ok {
				window.ResetsAt = parseTime(reset)
			}
			quota.Windows = append(quota.Windows, window)
		}
	}
	return quota, len(quota.Windows) > 0 || quota.Plan != ""
}

// ---------------------------------------------------------------- helpers

// windowLabel names a limit's window by its length.
func windowLabel(seconds float64) string {
	switch {
	case seconds <= 0:
		return "Limit"
	case seconds >= 28*86400 && seconds <= 31*86400:
		return "Monthly"
	case seconds == 7*86400:
		return "Weekly"
	case seconds == 86400:
		return "Daily"
	case seconds >= 86400 && math.Mod(seconds, 86400) == 0:
		return fmt.Sprintf("%.0fd", seconds/86400)
	case seconds >= 3600 && math.Mod(seconds, 3600) == 0:
		return fmt.Sprintf("%.0fh", seconds/3600)
	case seconds >= 60:
		return fmt.Sprintf("%.0fm", seconds/60)
	}
	return fmt.Sprintf("%.0fs", seconds)
}

// planName is a subscription's name as its owner knows it.
func planName(plan string) string {
	plan = strings.TrimSpace(strings.TrimPrefix(strings.ToLower(plan), "plan_"))
	switch plan {
	case "":
		return ""
	case "prolite":
		return "Pro Lite"
	case "self-serve-business", "self_serve_business":
		return "Business"
	}
	words := strings.FieldsFunc(plan, func(r rune) bool { return r == '_' || r == '-' || r == ' ' })
	for i, w := range words {
		words[i] = strings.ToUpper(w[:1]) + w[1:]
	}
	return strings.Join(words, " ")
}

func clampPercent(v float64) float64 {
	if math.IsNaN(v) || v < 0 {
		return 0
	}
	return math.Min(100, math.Round(v*10)/10)
}

func count(v float64) string {
	if v == math.Trunc(v) {
		return strconv.FormatFloat(v, 'f', 0, 64)
	}
	return strconv.FormatFloat(v, 'f', 1, 64)
}

func firstNonZero(values ...float64) float64 {
	for _, v := range values {
		if v != 0 {
			return v
		}
	}
	return 0
}

func asMap(v any) map[string]any {
	m, _ := v.(map[string]any)
	return m
}
