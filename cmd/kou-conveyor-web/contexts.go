package main

import (
	"errors"
	"net/http"
	"strconv"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

// Context windows: the window each model's runs take, set on the Accounts
// page, typed or as OpenRouter lists it (accounts.ContextBook).

// handleContextWindows lists the models the gateway serves and those a
// window is set for, with the windows the user, the gateway and OpenRouter
// give them.
func (s *server) handleContextWindows(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	writeJSON(w, http.StatusOK, gateway.ContextWindows(r.Context()))
}

// handleSetContextWindow sets a model's window: {model, tokens}, with
// openrouter, OpenRouter's ID of the model, when the window is the one it
// lists.
func (s *server) handleSetContextWindow(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var window accounts.ContextWindow
	if !decodeJSON(w, r, &window) {
		return
	}
	if _, err := gateway.SetContextWindow(window); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.ContextWindows(r.Context()))
}

// handleRemoveContextWindow drops the window set for ?model=: its runs go
// back to the window the runner knows or assumes.
func (s *server) handleRemoveContextWindow(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.RemoveContextWindow(r.URL.Query().Get("model")); err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.ContextWindows(r.Context()))
}

// handleOpenRouterWindow gets the context window OpenRouter lists for
// ?model=, from its list fetched anew unless that was minutes ago. It sets
// nothing: setting the window is the user's call.
func (s *server) handleOpenRouterWindow(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	model := strings.TrimSpace(r.URL.Query().Get("model"))
	if model == "" {
		writeError(w, http.StatusBadRequest, "name a model: ?model=")
		return
	}
	window, err := gateway.OpenRouterWindow(r.Context(), model)
	switch {
	case errors.Is(err, accounts.ErrOpenRouterOff):
		writeError(w, http.StatusServiceUnavailable, err.Error())
	case errors.Is(err, accounts.ErrNotOnOpenRouter):
		writeError(w, http.StatusNotFound, err.Error())
	case err != nil:
		writeError(w, http.StatusBadGateway, err.Error())
	default:
		writeJSON(w, http.StatusOK, window)
	}
}

// contextEnv sets a run's context window: the one set for its model on the
// Accounts page, over the runner's own.
func (s *server) contextEnv(model string) []string {
	if s.gateway == nil {
		return nil
	}
	if tokens := s.gateway.ContextWindow(model); tokens > 0 {
		return []string{runconfig.ContextWindowEnvironment + "=" + strconv.FormatInt(tokens, 10)}
	}
	return nil
}
