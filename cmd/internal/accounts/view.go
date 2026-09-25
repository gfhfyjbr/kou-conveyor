package accounts

import (
	"slices"
	"strings"
	"time"
)

// Account states, as the cockpit shows them.
const (
	// StateReady takes requests. Some of its models may be cooling down.
	StateReady = "ready"
	// StateCooling waits out a rate limit or a transient failure; the
	// gateway tries it again at RetryAt.
	StateCooling = "cooling"
	// StateError failed in a way that waiting does not fix, such as a
	// revoked or expired token.
	StateError = "error"
	// StateDisabled was turned off by its owner.
	StateDisabled = "disabled"
	// StateRefreshing is getting a new token.
	StateRefreshing = "refreshing"
	// StateConfigured is a key of the gateway's configuration, which the
	// gateway does not list: the cockpit knows it from its requests only.
	StateConfigured = "configured"
)

// Account is an account as the cockpit shows it. It carries no token and
// no key.
type Account struct {
	// Name identifies the account in the cockpit's routes: its credential
	// file, or the gateway's ID for one its configuration defines.
	Name     string `json:"name"`
	ID       string `json:"id"`
	Index    string `json:"index,omitzero"`
	Provider string `json:"provider"`
	// ProviderName is the provider's name for people.
	ProviderName string `json:"provider_name"`
	// Label is how the account is recognized: its e-mail, or a label.
	Label string `json:"label"`
	Plan  string `json:"plan,omitzero"`
	// Kind is "oauth" for a signed-in account, "api_key" for a key.
	Kind  string `json:"kind,omitzero"`
	State string `json:"state"`
	// Message is the gateway's word on the account's state.
	Message string    `json:"message,omitzero"`
	RetryAt time.Time `json:"retry_at,omitzero"`
	// Config marks an account the gateway's configuration defines, which
	// the cockpit neither disables nor removes.
	Config    bool       `json:"config,omitzero"`
	Created   time.Time  `json:"created,omitzero"`
	Refreshed time.Time  `json:"refreshed,omitzero"`
	Priority  *int       `json:"priority,omitzero"`
	Note      string     `json:"note,omitzero"`
	Cooldowns []Cooldown `json:"cooldowns,omitzero"`
	// Uptime is the account's requests over the span asked for.
	Uptime Uptime        `json:"uptime"`
	Errors []ErrorSample `json:"errors,omitzero"` // newest first
	// Quota is the account's latest known quota: fetched live, else read
	// from its latest response.
	Quota          *Quota `json:"quota,omitzero"`
	QuotaSupported bool   `json:"quota_supported,omitzero"`
	// Models are the IDs of the models the gateway serves with the account.
	Models []string `json:"models,omitzero"`

	target target
}

// Cooldown is a wait the gateway imposes on an account or one of its
// models after a failure.
type Cooldown struct {
	Model  string    `json:"model,omitzero"` // empty for the whole account
	Reason string    `json:"reason"`
	Until  time.Time `json:"until"`
	Status int       `json:"status,omitzero"`
}

// accountOf reads one entry of the gateway's account list.
func accountOf(r record, now time.Time) Account {
	a := Account{
		Name: firstNonEmpty(r.str("name"), r.str("id")), ID: r.str("id"), Index: r.str("auth_index"),
		Provider: strings.ToLower(firstNonEmpty(r.str("provider"), r.str("type"))),
		Kind:     r.str("account_type"), Message: r.str("status_message"), Note: r.str("note"),
		Config:  r.flag("runtime_only") || r.str("source") == "memory" && r.str("path") == "",
		Created: r.when("created_at"), Refreshed: r.when("last_refresh"),
	}
	a.ProviderName = ProviderName(a.Provider)
	a.Label = firstNonEmpty(r.str("email"), r.str("label"))
	if a.Label == "" && a.Kind == "api_key" {
		a.Label = maskKey(r.str("account"))
	}
	if a.Label == "" {
		a.Label = a.Name
	}
	if _, ok := r["priority"].(float64); ok {
		p := int(r.num("priority"))
		a.Priority = &p
	}
	claims := r.obj("id_token")
	a.Plan = planName(claims.str("plan_type"))
	a.target = target{name: a.Name, index: a.Index, provider: a.Provider, chatgptAccount: claims.str("chatgpt_account_id"), project: r.str("project_id")}
	a.QuotaSupported = a.Kind != "api_key" && quotaSupported(a.Provider) && a.Index != ""

	for _, raw := range r.list("cooldowns") {
		c := record(asMap(raw))
		until := c.when("retry_at")
		if !until.After(now) {
			continue
		}
		a.Cooldowns = append(a.Cooldowns, Cooldown{Model: c.str("model_key"), Reason: c.str("reason"), Until: until, Status: int(c.num("http_status"))})
	}
	slices.SortFunc(a.Cooldowns, func(x, y Cooldown) int { return x.Until.Compare(y.Until) })

	status := strings.ToLower(r.str("status"))
	retry := r.when("next_retry_after")
	accountWide := slices.ContainsFunc(a.Cooldowns, func(c Cooldown) bool { return c.Model == "" })
	switch {
	case r.flag("disabled") || status == "disabled":
		a.State = StateDisabled
	case status == "refreshing" || status == "pending":
		a.State = StateRefreshing
	case accountWide || r.flag("unavailable") && retry.After(now):
		a.State, a.RetryAt = StateCooling, retry
		for _, c := range a.Cooldowns {
			if c.Model == "" && c.Until.After(a.RetryAt) {
				a.RetryAt = c.Until
			}
		}
	case status == "error" || r.flag("unavailable"):
		a.State = StateError
	default:
		a.State = StateReady
		if status == "active" {
			a.Message = ""
		}
	}
	return a
}

// configuredAccount is a key of the gateway's configuration that the
// history saw serve requests. The gateway names such keys by their kind and
// a hash: "claude:apikey:1a2b3c4d5e6f", "openai-compatibility:<name>:…".
func configuredAccount(k known) Account {
	a := Account{
		Name: k.id, ID: k.id, Index: k.index, Provider: k.provider, Kind: "api_key", State: StateConfigured, Config: true,
		Message: "A key in the gateway's config.yaml",
	}
	parts := strings.Split(k.id, ":")
	hash := parts[len(parts)-1]
	if len(hash) > 6 {
		hash = hash[:6]
	}
	switch {
	case len(parts) >= 3 && parts[0] == "openai-compatibility":
		a.Label, a.ProviderName = parts[1]+" · key "+hash, "OpenAI-compatible"
		a.Provider = "openai-compatibility"
	case len(parts) >= 3 && parts[1] == "apikey":
		a.Label = "API key " + hash
		if a.Provider == "" {
			a.Provider = parts[0]
		}
	default:
		a.Label = k.id
	}
	if a.ProviderName == "" {
		a.ProviderName = ProviderName(a.Provider)
	}
	return a
}

// maskKey shows enough of an API key to recognize it.
func maskKey(key string) string {
	key = strings.TrimSpace(key)
	if len(key) < 12 {
		return ""
	}
	return key[:4] + "…" + key[len(key)-4:]
}

// Summary counts the accounts by state, and their requests.
type Summary struct {
	Accounts int `json:"accounts"`
	// Keys counts the keys of the gateway's configuration that served
	// requests and are not endpoints the cockpit edits; Accounts leaves
	// them out.
	Keys int `json:"keys,omitzero"`
	// Endpoints counts the endpoints, FailingEndpoints those whose latest
	// request failed.
	Endpoints        int   `json:"endpoints"`
	FailingEndpoints int   `json:"failing_endpoints,omitzero"`
	Ready            int   `json:"ready"`
	Cooling          int   `json:"cooling"`
	Failing          int   `json:"failing"`
	Disabled         int   `json:"disabled"`
	Refreshing       int   `json:"refreshing,omitzero"`
	OK               int64 `json:"ok"`
	Failed           int64 `json:"failed"`
}

// Summarize counts accounts and their requests over the span they report.
func Summarize(list []Account) Summary {
	s := Summary{Accounts: len(list)}
	for _, a := range list {
		switch a.State {
		case StateReady:
			s.Ready++
		case StateCooling:
			s.Cooling++
		case StateError:
			s.Failing++
		case StateDisabled:
			s.Disabled++
		case StateRefreshing:
			s.Refreshing++
		case StateConfigured:
			s.Keys++
			s.Accounts--
		}
		s.OK += a.Uptime.OK
		s.Failed += a.Uptime.Failed
	}
	return s
}

// sortAccounts orders accounts by provider, as sign-in lists them, then
// by label, and the keys of the configuration last.
func sortAccounts(list []Account) {
	rank := func(provider string) int {
		for i, p := range providers {
			if p.ID == provider {
				return i
			}
		}
		return len(providers)
	}
	slices.SortStableFunc(list, func(a, b Account) int {
		// Keys of the configuration follow the accounts signed in here.
		if ac, bc := a.State == StateConfigured, b.State == StateConfigured; ac != bc {
			if ac {
				return 1
			}
			return -1
		}
		if d := rank(a.Provider) - rank(b.Provider); d != 0 {
			return d
		}
		if a.Provider != b.Provider {
			return strings.Compare(a.Provider, b.Provider)
		}
		return strings.Compare(strings.ToLower(a.Label), strings.ToLower(b.Label))
	})
}
