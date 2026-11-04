package main

import (
	"net/http"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

// Prompt-cache TTLs: the TTL each model's runs ask for, set on the Accounts
// page (accounts.CacheBook). The models and their current settings are listed
// with the context windows, so the Accounts page needs no second model list.

// handleSetCacheTTL sets a model's TTL: {model, ttl}, where ttl is 5m or 1h.
func (s *server) handleSetCacheTTL(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var ttl accounts.CacheTTL
	if !decodeJSON(w, r, &ttl) {
		return
	}
	if _, err := gateway.SetCacheTTL(ttl); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.ContextWindows(r.Context()))
}

// handleRemoveCacheTTL drops the TTL set for ?model=: its runs go back to the
// provider's default.
func (s *server) handleRemoveCacheTTL(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.RemoveCacheTTL(r.URL.Query().Get("model")); err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.ContextWindows(r.Context()))
}

// cacheEnv sets a run's prompt-cache TTL: the one set for its model on the
// Accounts page, over the provider's own default.
func (s *server) cacheEnv(model string) []string {
	if s.gateway == nil {
		return nil
	}
	if ttl := s.gateway.CacheTTL(model); ttl != "" {
		return []string{runconfig.CacheTTLEnvironment + "=" + ttl}
	}
	return nil
}
