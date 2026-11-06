package canvas

import (
	"context"
	"net/http"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type agentModelCatalog struct {
	cockpit.Catalog
	Omitted int `json:"omitted,omitzero"`
}

func (e *Engine) handleAgentModels(w http.ResponseWriter, r *http.Request, a *agentCall) {
	ctx, cancel := context.WithTimeout(r.Context(), 25*time.Second)
	defer cancel()
	listed := e.o.Host.Models(ctx, a.c.ws, truthy(r.URL.Query().Get("fresh")))
	catalog := agentModelCatalog{Catalog: cockpit.Catalog{Source: listed.Source, Default: listed.Default, Models: []cockpit.ModelInfo{}}}
	for _, model := range listed.Models {
		if model.Media {
			continue
		}
		if model.ID == "" || cockpit.ValidModel(model.ID) != nil {
			catalog.Omitted++
			continue
		}
		catalog.Models = append(catalog.Models, model)
	}
	if listed.Error != "" {
		// Connection errors can contain private upstream URLs or credentials.
		catalog.Error = "the model catalog is unavailable; check the connection in the cockpit and retry with fresh: true"
	}
	if ctx.Err() != nil {
		catalog.Error = "the model catalog request timed out or was cancelled; try again"
	}
	writeJSON(w, http.StatusOK, catalog)
}
