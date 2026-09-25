package main

import (
	"context"
	"errors"
	"net/http"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Models. Each prompt runs with a model of its own choosing, picked in the
// composer from the models the connection reaches: through the gateway,
// those of every account and endpoint it has, whichever provider serves
// them; directly, those the endpoint lists. The gateway runs in this
// process, so its models come from its registry, with their names, context
// windows and the accounts that serve them, those cooling down included.

// catalogTTL is how long an endpoint's model list serves before it is
// asked again: asking takes a request to the provider.
const catalogTTL = 2 * time.Minute

type catalogCache struct {
	mu      sync.Mutex
	key     string
	at      time.Time
	catalog cockpit.Catalog
}

// catalog lists the models a prompt in the workspace can run with. fresh
// asks an endpoint again rather than answer from what it said last.
func (s *server) catalog(ctx context.Context, ws *workspace, fresh bool) cockpit.Catalog {
	settings, err := s.settings()
	if err != nil {
		return cockpit.Catalog{Models: []cockpit.ModelInfo{}, Error: err.Error()}
	}
	defaultModel := s.connection(settings, ws).Model
	if s.opt.Provider == "" && settings.API == cockpit.APIGateway && s.gateway != nil {
		catalog := cockpit.Catalog{Source: "gateway", Default: defaultModel, Models: []cockpit.ModelInfo{}}
		models, err := s.gatewayModels(ctx)
		if err != nil {
			catalog.Error = err.Error()
		} else {
			catalog.Models = models
		}
		return catalog
	}
	key := string(settings.API) + "\x00" + settings.BaseURL + "\x00" + settings.KeyHint() + "\x00" + ws.Path
	s.models.mu.Lock()
	if !fresh && s.models.key == key && time.Since(s.models.at) < catalogTTL {
		catalog := s.models.catalog
		s.models.mu.Unlock()
		catalog.Default = defaultModel
		return catalog
	}
	s.models.mu.Unlock()
	catalog := cockpit.ListModels(ctx, s.opt.Provider, s.opt.SettingsFile, settings, cockpit.WorkspaceEnv(ws.Path))
	catalog.Default = defaultModel
	if catalog.Error == "" {
		s.models.mu.Lock()
		s.models.key, s.models.at, s.models.catalog = key, time.Now(), catalog
		s.models.mu.Unlock()
	}
	return catalog
}

// gatewayModels are the models the gateway serves, as the composer offers
// them.
func (s *server) gatewayModels(ctx context.Context) ([]cockpit.ModelInfo, error) {
	served, err := s.gateway.Models(ctx)
	if err != nil {
		return nil, err
	}
	models := make([]cockpit.ModelInfo, 0, len(served))
	for _, m := range served {
		models = append(models, modelInfo(m))
	}
	cockpit.SortModels(models)
	return models, nil
}

func modelInfo(m accounts.Model) cockpit.ModelInfo {
	return cockpit.DescribeModel(cockpit.ModelInfo{
		ID: m.ID, Name: m.Name, Provider: m.OwnedBy, API: cockpit.GatewayAPI(m.ID),
		Created: m.Created, Context: m.Context, Cooling: m.Cooling, Via: m.Via,
	}, "")
}

// handleModels lists the models a prompt of the workspace can run with;
// ?fresh=1 asks an endpoint again.
func (s *server) handleModels(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()
	catalog := s.catalog(ctx, ws, r.URL.Query().Get("fresh") != "")
	if errors.Is(ctx.Err(), context.DeadlineExceeded) && catalog.Error == "" {
		catalog.Error = "the model list took too long"
	}
	writeJSON(w, http.StatusOK, catalog)
}
