package accounts

import (
	"cmp"
	"context"
	"slices"
	"strings"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v8/sdk/cliproxy"
)

// Usage reports what the gateway's requests used over a range, and what it
// comes to at API prices: the accounts and keys named as the gateway lists
// them now, and as the ledger remembers those it no longer has; the models
// named as the gateway knows them.
func (h *Host) Usage(ctx context.Context, q UsageQuery) (UsageReport, error) {
	report, err := h.ledger.Report(q, h.prices.Resolve)
	if err != nil {
		return UsageReport{}, err
	}
	report.Unit = h.prices.Unit()
	if err := h.prices.Err(); err != nil {
		report.PricesError = err.Error()
	}
	ids := make([]string, 0, len(report.Facets.Accounts))
	for _, f := range report.Facets.Accounts {
		ids = append(ids, f.ID)
	}
	if a := q.Filter.Account; a != "" && !slices.Contains(ids, a) {
		ids = append(ids, a)
	}
	report.Accounts = h.ledger.accountsOf(ids)
	served := map[string]Model{}
	if client, err := h.running(); err == nil {
		var accounts []Account
		if records, err := client.authFiles(ctx); err == nil {
			now := time.Now()
			for _, r := range records {
				accounts = append(accounts, accountOf(r, now))
			}
		}
		endpoints, _ := client.endpoints(ctx)
		for i := range report.Accounts {
			u := &report.Accounts[i]
			if u.ID == "" {
				continue
			}
			index := h.ledger.indexOf(u.ID)
			u.Gone = true
			for _, a := range accounts {
				if a.ID == u.ID || index != "" && a.Index == index {
					u.Name, u.Label, u.Provider, u.ProviderName, u.Kind, u.Gone = a.Name, a.Label, a.Provider, a.ProviderName, "account", false
					if a.Config {
						u.Kind = "key"
					}
					h.ledger.learn(u.ID, "", ledgerAuth{Label: a.Label, Name: a.ProviderName, Kind: u.Kind})
				}
			}
			for _, e := range endpoints {
				if index != "" && slices.Contains(e.indexes, index) {
					u.Label, u.ProviderName, u.Kind, u.Gone = cmp.Or(e.Name, e.Host), e.KindName, "endpoint", false
					h.ledger.learn(u.ID, "", ledgerAuth{Label: u.Label, Name: e.KindName, Kind: "endpoint"})
				}
			}
			// Keys of the configuration the cockpit does not edit are not
			// listed: they are there while the gateway serves models with them.
			if u.Gone && strings.Contains(u.ID, ":") && len(accountModels(cliproxy.GlobalModelRegistry(), u.ID)) > 0 {
				u.Gone = false
			}
		}
		for _, m := range servedModels(cliproxy.GlobalModelRegistry(), h.clients.list()) {
			served[m.ID] = m
		}
	}
	for i := range report.Models {
		if m, ok := served[report.Models[i].ID]; ok {
			report.Models[i].Name, report.Models[i].Served = m.Name, true
		}
	}
	return report, nil
}

// indexOf is the auth index the ledger knows an auth ID by.
func (l *Ledger) indexOf(id string) string {
	l.mu.Lock()
	defer l.mu.Unlock()
	if a := l.auths[id]; a != nil {
		return a.Index
	}
	return ""
}

// usedModels are the models the ledger has seen, the most recent first.
func (l *Ledger) usedModels() []string {
	l.mu.Lock()
	defer l.mu.Unlock()
	last := map[string]int64{}
	for key := range l.rows {
		if key.model != "" && key.hour > last[key.model] {
			last[key.model] = key.hour
		}
	}
	out := make([]string, 0, len(last))
	for model := range last {
		out = append(out, model)
	}
	slices.SortFunc(out, func(a, b string) int { return cmp.Or(cmp.Compare(last[b], last[a]), strings.Compare(a, b)) })
	return out
}

// PricedModel is a model the price editor lists: one the gateway serves or
// requests used, with the price its tokens count at.
type PricedModel struct {
	ID    string     `json:"id"`
	Name  string     `json:"name,omitzero"`
	Price ModelPrice `json:"price"`
	// Served: the gateway serves it now; Used: requests used it within
	// the ledger's keeping.
	Served bool `json:"served,omitzero"`
	Used   bool `json:"used,omitzero"`
	// Media: it makes pictures or videos, which are not priced by the token.
	Media bool `json:"media,omitzero"`
}

// PriceListing is what the price editor shows: the unit, the user's rules,
// every model the gateway serves or requests used with its price, and the
// prices the cockpit knows, to borrow from.
type PriceListing struct {
	Unit    Unit           `json:"unit"`
	Rules   []PriceRule    `json:"rules"`
	Models  []PricedModel  `json:"models"`
	Builtin []BuiltinPrice `json:"builtin"`
	// OpenRouter says how the prices fetched from OpenRouter stand.
	OpenRouter OpenRouterStatus `json:"openrouter"`
	Error      string           `json:"error,omitzero"`
}

// Prices lists the prices tokens are counted at.
func (h *Host) Prices(ctx context.Context) PriceListing {
	listing := PriceListing{Unit: h.prices.Unit(), Rules: h.prices.Rules(), Builtin: BuiltinPrices(), OpenRouter: h.market.Status()}
	if err := h.prices.Err(); err != nil {
		listing.Error = err.Error()
	}
	byID := map[string]*PricedModel{}
	var order []string
	add := func(id string) *PricedModel {
		m := byID[id]
		if m == nil {
			m = &PricedModel{ID: id, Price: h.prices.Resolve(id)}
			byID[id] = m
			order = append(order, id)
		}
		return m
	}
	for _, id := range h.ledger.usedModels() {
		add(id).Used = true
	}
	if _, err := h.running(); err == nil {
		for _, served := range servedModels(cliproxy.GlobalModelRegistry(), h.clients.list()) {
			m := add(served.ID)
			m.Served, m.Name = true, served.Name
			m.Media = mediaModel(served.ID)
		}
	}
	for _, id := range order {
		listing.Models = append(listing.Models, *byID[id])
	}
	return listing
}

// mediaModel reports whether a model makes pictures or videos, which APIs
// price by the picture or the second rather than the token.
func mediaModel(id string) bool {
	id = strings.ToLower(id)
	for _, word := range []string{"image", "imagine", "video", "dall-e", "sora", "veo", "imagen"} {
		if strings.Contains(id, word) {
			return true
		}
	}
	return false
}

// SetPrice adds a price rule of the user's, or replaces the one of the
// same pattern.
func (h *Host) SetPrice(rule PriceRule) (PriceRule, error) { return h.prices.Set(rule) }

// RemovePrice drops a price rule of the user's.
func (h *Host) RemovePrice(match string) error { return h.prices.Remove(match) }

// Unit is what amounts are shown in.
func (h *Host) Unit() Unit { return h.prices.Unit() }

// SetUnit chooses what amounts are shown in.
func (h *Host) SetUnit(unit Unit) (Unit, error) { return h.prices.SetUnit(unit) }

// RefreshPrices fetches OpenRouter's prices now.
func (h *Host) RefreshPrices(ctx context.Context) error { return h.market.Refresh(ctx) }
