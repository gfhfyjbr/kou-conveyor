package accounts

import (
	"cmp"
	"context"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy"
)

// The models the gateway serves come from its model registry, which runs in
// this process with the gateway. Every account and endpoint registers the
// models it serves there as the gateway takes it up — an account a moment
// after its sign-in, once the gateway has read its credential — and a model
// whose accounts all wait out a limit stays registered, cooling. The
// gateway's own /v1/models lists only the models that can take a request
// now, by their IDs alone; the registry also says what each model is called,
// how much its context window holds, and which accounts serve it.

// Model is a model the gateway serves.
type Model struct {
	ID string `json:"id"`
	// Name is the model's name for people.
	Name string `json:"name,omitzero"`
	// OwnedBy is who makes it, as the gateway says: anthropic, openai, xai…
	OwnedBy string `json:"owned_by,omitzero"`
	Created int64  `json:"created,omitzero"`
	// Context is how many tokens its context window holds, if known.
	Context int64 `json:"context,omitzero"`
	// Cooling: every account and endpoint that serves it waits out a limit;
	// the gateway takes requests for it again once one has.
	Cooling bool `json:"cooling,omitzero"`
	// Clients are the gateway's IDs of the accounts and keys that serve it;
	// Via names them for people.
	Clients []string `json:"clients,omitzero"`
	Via     []string `json:"via,omitzero"`
}

// registry is the part of the gateway's model registry the cockpit reads.
type registry interface {
	GetAvailableModels(handlerType string) []map[string]any
	GetModelsForClient(clientID string) []*cliproxy.ModelInfo
}

// clientWatch learns from the registry which clients — accounts, and keys of
// the configuration — have registered models. The registry does not list
// them; it tells a hook, from goroutines of its own and in no set order, so
// the watch only collects their IDs, and asks the registry what each serves
// now whenever that matters.
type clientWatch struct {
	mu      sync.Mutex
	clients map[string]bool
}

func newClientWatch() *clientWatch { return &clientWatch{clients: map[string]bool{}} }

func (w *clientWatch) OnModelsRegistered(_ context.Context, _, clientID string, _ []*cliproxy.ModelInfo) {
	w.add(clientID)
}

func (w *clientWatch) OnModelsUnregistered(context.Context, string, string) {}

func (w *clientWatch) add(ids ...string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	for _, id := range ids {
		if id != "" {
			w.clients[id] = true
		}
	}
}

func (w *clientWatch) list() []string {
	w.mu.Lock()
	defer w.mu.Unlock()
	ids := make([]string, 0, len(w.clients))
	for id := range w.clients {
		ids = append(ids, id)
	}
	slices.Sort(ids)
	return ids
}

// Models lists what the gateway serves with the accounts and endpoints it
// has, the models that wait out a limit included.
func (h *Host) Models(ctx context.Context) ([]Model, error) {
	client, err := h.running()
	if err != nil {
		return nil, err
	}
	// The accounts are clients too: listing them reaches those whose
	// registration the watch missed, and names them.
	var accounts []Account
	if records, err := client.authFiles(ctx); err == nil {
		now := time.Now()
		for _, r := range records {
			a := accountOf(r, now)
			accounts = append(accounts, a)
			h.clients.add(a.ID)
		}
	}
	models := servedModels(cliproxy.GlobalModelRegistry(), h.clients.list())
	for i := range models {
		for _, id := range models[i].Clients {
			models[i].Via = append(models[i].Via, ClientLabel(id, accounts))
		}
	}
	return models, nil
}

// servedModels gathers the models the clients serve, and the models the
// registry has available, into one list.
func servedModels(reg registry, clients []string) []Model {
	byID := map[string]*Model{}
	var order []string
	add := func(id string) *Model {
		m := byID[id]
		if m == nil {
			m = &Model{ID: id, Cooling: true}
			byID[id] = m
			order = append(order, id)
		}
		return m
	}
	for _, client := range clients {
		for _, info := range reg.GetModelsForClient(client) {
			if info == nil || strings.TrimSpace(info.ID) == "" {
				continue
			}
			id := strings.TrimSpace(info.ID)
			m := add(id)
			m.Name = cmp.Or(m.Name, strings.TrimSpace(info.DisplayName))
			m.OwnedBy = cmp.Or(m.OwnedBy, strings.TrimSpace(info.OwnedBy))
			m.Created = cmp.Or(m.Created, info.Created)
			m.Context = cmp.Or(m.Context, int64(info.ContextLength), int64(info.InputTokenLimit))
			if !slices.Contains(m.Clients, client) {
				m.Clients = append(m.Clients, client)
			}
		}
	}
	// What the registry has available can take a request now; a model it
	// has without the clients seen serves too.
	for _, entry := range reg.GetAvailableModels("openai") {
		r := record(entry)
		id := r.str("id")
		if id == "" {
			continue
		}
		m := add(id)
		m.Cooling = false
		m.Name = cmp.Or(m.Name, r.str("display_name"))
		m.OwnedBy = cmp.Or(m.OwnedBy, r.str("owned_by"))
		m.Created = cmp.Or(m.Created, int64(r.num("created")))
		m.Context = cmp.Or(m.Context, int64(r.num("context_length")))
	}
	models := make([]Model, 0, len(order))
	for _, id := range order {
		models = append(models, *byID[id])
	}
	slices.SortFunc(models, func(a, b Model) int { return strings.Compare(a.ID, b.ID) })
	return models
}

// accountModels are the IDs of the models an account serves.
func accountModels(reg registry, clientID string) []string {
	var ids []string
	for _, info := range reg.GetModelsForClient(clientID) {
		if info != nil && strings.TrimSpace(info.ID) != "" {
			ids = append(ids, strings.TrimSpace(info.ID))
		}
	}
	slices.Sort(ids)
	return slices.Compact(ids)
}

// ClientLabel names a client of the gateway for people: an account by its
// label, a key of the configuration by its kind.
func ClientLabel(id string, accounts []Account) string {
	for _, a := range accounts {
		if a.ID == id || a.Name == id {
			return a.ProviderName + " · " + a.Label
		}
	}
	parts := strings.Split(id, ":")
	switch {
	case len(parts) >= 3 && parts[0] == "openai-compatibility":
		return parts[1]
	case len(parts) >= 3 && parts[1] == "apikey":
		for _, kind := range endpointKinds {
			if kind.section == parts[0]+"-api-key" {
				return kind.Name + " key"
			}
		}
		return ProviderName(parts[0]) + " API key"
	}
	return id
}
