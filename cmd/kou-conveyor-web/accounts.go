package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The accounts gateway: CLIProxyAPI, run inside the server (package
// accounts), through which runs reach every model: the accounts signed in
// with OAuth and the endpoints added with an API key. Browsers see both and
// how they do, add and remove them, and choose the model runs use; the
// gateway's key, the accounts' tokens and the endpoints' keys stay here.

// startGateway runs the gateway for the server's lifetime, unless the
// server was told not to or has nowhere to keep credentials.
func (s *server) startGateway(ctx context.Context) {
	address := s.opt.accounts
	if address == "" || address == "off" {
		return
	}
	dir := s.opt.accountsDir
	if dir == "" && s.opt.SettingsFile != "" {
		dir = filepath.Join(filepath.Dir(s.opt.SettingsFile), "cliproxy")
	}
	if dir == "" {
		return
	}
	gateway, err := accounts.New(accounts.Options{Dir: dir, Address: address})
	if err != nil {
		s.gatewayErr = err.Error()
		return
	}
	s.gateway, s.gatewayGone = gateway, make(chan struct{})
	go gateway.Run(ctx)
	// Runs of either cockpit find the gateway through its publication
	// beside the settings, for as long as it runs.
	go func() {
		defer close(s.gatewayGone)
		published := cockpit.Gateway{}
		select {
		case <-gateway.Ready():
			published = cockpit.Gateway{URL: gateway.URL(), APIKey: gateway.APIKey()}
			if path := cockpit.GatewayPath(s.opt.SettingsFile); path != "" {
				if err := cockpit.PublishGateway(path, published); err != nil {
					fmt.Fprintln(os.Stderr, "kou-conveyor-web: publish the accounts gateway:", err)
				}
			}
		case <-gateway.Done():
		}
		<-gateway.Done()
		if path := cockpit.GatewayPath(s.opt.SettingsFile); path != "" && published.URL != "" {
			cockpit.WithdrawGateway(path, published)
		}
		if status := gateway.Status(); status.State == accounts.GatewayFailed {
			fmt.Fprintln(os.Stderr, "kou-conveyor-web: accounts gateway:", status.Error)
		}
	}()
}

// gatewayView is what browsers learn about the gateway.
type gatewayView struct {
	accounts.Status
	Enabled bool `json:"enabled"`
	// Use is how runs connect to the gateway, if they do.
	Use *gatewayUse `json:"use,omitzero"`
}

type gatewayUse struct {
	API   cockpit.API `json:"api"`
	Model string      `json:"model,omitzero"`
}

func (s *server) gatewayView() gatewayView {
	if s.gateway == nil {
		view := gatewayView{}
		view.State, view.Error = "off", s.gatewayErr
		return view
	}
	view := gatewayView{Status: s.gateway.Status(), Enabled: true}
	settings, err := s.settings()
	switch {
	case err != nil:
	case settings.API == cockpit.APIGateway:
		api := settings.Protocol
		if api == "" {
			api = cockpit.GatewayAPI(settings.Model)
		}
		view.Use = &gatewayUse{API: api, Model: settings.Model}
	case settings.API != cockpit.APIEnvironment && s.gateway.Serves(settings.BaseURL):
		view.Use = &gatewayUse{API: settings.API, Model: settings.Model}
	}
	return view
}

// connectionView is what runs connect to, as the Accounts tab shows it.
type connectionView struct {
	// Mode is "gateway", "environment", "direct" (settings that name an
	// endpoint of their own, around the gateway) or "flag" (-provider).
	Mode     string      `json:"mode"`
	Model    string      `json:"model,omitzero"`
	Protocol cockpit.API `json:"protocol,omitzero"`
	// API is what runs speak: the protocol, or the model's.
	API     cockpit.API `json:"api,omitzero"`
	BaseURL string      `json:"base_url,omitzero"`
	KeyHint string      `json:"key_hint,omitzero"`
	// ViaGateway: direct settings that name the gateway's own address, as
	// if it were any endpoint.
	ViaGateway bool `json:"via_gateway,omitzero"`
	// Resolved is where runs of the server's first workspace go.
	Resolved cockpit.Connection `json:"resolved"`
	// Available: the settings can be saved.
	Available bool   `json:"available"`
	Error     string `json:"error,omitzero"`
}

func (s *server) connectionView(r *http.Request) connectionView {
	settings, err := s.settings()
	view := connectionView{Available: s.opt.SettingsFile != "", Resolved: s.connection(settings, s.queryWorkspace(r))}
	if err != nil {
		view.Error = err.Error()
	}
	switch {
	case s.opt.Provider != "":
		view.Mode = "flag"
	case settings.API == cockpit.APIGateway:
		view.Mode, view.Model, view.Protocol, view.API = "gateway", settings.Model, settings.Protocol, view.Resolved.API
	case settings.API == cockpit.APIEnvironment:
		view.Mode = "environment"
	default:
		view.Mode, view.Model, view.API, view.BaseURL, view.KeyHint = "direct", settings.Model, settings.API, settings.BaseURL, settings.KeyHint()
		view.ViaGateway = s.gateway != nil && s.gateway.Serves(settings.BaseURL)
	}
	return view
}

// accountsError answers a failed request about accounts.
func accountsError(w http.ResponseWriter, err error) {
	status := http.StatusBadRequest
	switch {
	case errors.Is(err, accounts.ErrNotRunning), errors.Is(err, accounts.ErrUnreachable):
		status = http.StatusServiceUnavailable
	case errors.Is(err, accounts.ErrNoAccount), errors.Is(err, accounts.ErrNoLogin), errors.Is(err, accounts.ErrNoEndpoint):
		status = http.StatusNotFound
	case errors.Is(err, context.DeadlineExceeded):
		status = http.StatusGatewayTimeout
	case accounts.StatusOf(err) >= 500:
		status = http.StatusBadGateway
	case accounts.StatusOf(err) >= 400:
		status = accounts.StatusOf(err)
	}
	writeError(w, status, err.Error())
}

// withGateway runs a handler that needs the gateway.
func (s *server) withGateway(handle func(w http.ResponseWriter, r *http.Request, gateway *accounts.Host)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.gateway == nil {
			message := "the accounts gateway is off: start the server with -accounts 127.0.0.1:8318"
			if s.gatewayErr != "" {
				message = s.gatewayErr
			}
			writeError(w, http.StatusServiceUnavailable, message)
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 90*time.Second)
		defer cancel()
		handle(w, r.WithContext(ctx), s.gateway)
	}
}

// handleAccounts lists the accounts, with their uptime over ?range= (24h
// or 7d), and the sign-ins the gateway offers.
func (s *server) handleAccounts(w http.ResponseWriter, r *http.Request) {
	span := r.URL.Query().Get("range")
	if span == "" {
		span = "24h"
	}
	if !accounts.ValidSpan(span) {
		writeError(w, http.StatusBadRequest, "range is 24h or 7d")
		return
	}
	answer := map[string]any{
		"providers": accounts.Providers(), "endpoint_kinds": accounts.EndpointKinds(), "range": span,
		"accounts": []accounts.Account{}, "endpoints": []accounts.Endpoint{},
	}
	if s.gateway != nil {
		ctx, cancel := context.WithTimeout(r.Context(), 10*time.Second)
		defer cancel()
		o, err := s.gateway.Overview(ctx, span)
		switch {
		case err == nil:
			answer["accounts"], answer["endpoints"] = o.Accounts, o.Endpoints
			// The models come with the accounts, so an account signed in
			// shows its models with the next listing that shows it.
			if models, err := s.gatewayModels(ctx); err == nil {
				answer["models"] = models
			}
		case !errors.Is(err, accounts.ErrNotRunning):
			answer["error"] = err.Error()
		}
	}
	// The gateway's summary is the overview's.
	answer["gateway"] = s.gatewayView()
	answer["connection"] = s.connectionView(r)
	writeJSON(w, http.StatusOK, answer)
}

func (s *server) handleAccountQuota(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	quota, err := gateway.Quota(r.Context(), r.PathValue("name"), r.URL.Query().Get("refresh") != "")
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, quota)
}

func (s *server) handleUpdateAccount(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var change struct {
		Disabled *bool `json:"disabled"`
	}
	if !decodeJSON(w, r, &change) {
		return
	}
	if change.Disabled == nil {
		writeError(w, http.StatusBadRequest, "nothing to change: send disabled")
		return
	}
	if err := gateway.SetDisabled(r.Context(), r.PathValue("name"), *change.Disabled); err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"name": r.PathValue("name"), "disabled": *change.Disabled})
}

func (s *server) handleRemoveAccount(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.Remove(r.Context(), r.PathValue("name")); err != nil {
		accountsError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *server) handleRefreshAccount(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.Refresh(r.Context(), r.PathValue("name")); err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"name": r.PathValue("name")})
}

// handleImportAccount adds an account from a CLIProxyAPI credential file
// the browser read.
func (s *server) handleImportAccount(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var file struct {
		Name    string `json:"name"`
		Content string `json:"content"`
	}
	if !decodeJSON(w, r, &file) {
		return
	}
	var credential map[string]any
	if json.Unmarshal([]byte(file.Content), &credential) != nil || len(credential) == 0 {
		writeError(w, http.StatusBadRequest, file.Name+" is not a credential: it should hold a JSON object")
		return
	}
	if err := gateway.Import(r.Context(), file.Name, []byte(file.Content)); err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"name": file.Name})
}

// handleGatewayModels lists the models the gateway serves: those of every
// account and endpoint, cooling ones too.
func (s *server) handleGatewayModels(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	models, err := s.gatewayModels(r.Context())
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"models": models})
}

// handleSaveConnection chooses what runs connect to: the gateway, with a
// model it serves, or the runner's environment.
func (s *server) handleSaveConnection(w http.ResponseWriter, r *http.Request) {
	var choice struct {
		Mode     string      `json:"mode"`
		Model    string      `json:"model"`
		Protocol cockpit.API `json:"protocol"`
	}
	if !decodeJSON(w, r, &choice) {
		return
	}
	var next cockpit.Settings
	switch choice.Mode {
	case "gateway":
		if s.gateway == nil {
			writeError(w, http.StatusConflict, "the accounts gateway is off: start the server with -accounts 127.0.0.1:8318")
			return
		}
		next = cockpit.Settings{API: cockpit.APIGateway, Model: choice.Model, Protocol: choice.Protocol}
	case "environment":
	default:
		writeError(w, http.StatusBadRequest, "mode is gateway or environment")
		return
	}
	s.saveConnection(w, r, next)
}

func (s *server) saveConnection(w http.ResponseWriter, r *http.Request, next cockpit.Settings) {
	if s.opt.SettingsFile == "" {
		writeError(w, http.StatusConflict, "settings are unavailable: set KOU_CONVEYOR_CONFIG")
		return
	}
	s.settingsMu.Lock()
	next, err := (cockpit.Settings{}).Update(cockpit.SettingsUpdate{API: next.API, Model: next.Model, Protocol: next.Protocol})
	if err == nil {
		err = cockpit.SaveSettings(s.opt.SettingsFile, next)
	}
	s.settingsMu.Unlock()
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"connection": s.connectionView(r), "gateway": s.gatewayView()})
}

// handleAdoptConnection moves a direct connection into the gateway: its
// endpoint becomes one of the gateway's, with its key, and runs go through
// the gateway to the same model. Settings that already point at the
// gateway only change their form.
func (s *server) handleAdoptConnection(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	settings, err := s.settings()
	if err != nil {
		writeError(w, http.StatusConflict, err.Error())
		return
	}
	if settings.API != cockpit.APIMessages && settings.API != cockpit.APIResponses {
		writeError(w, http.StatusConflict, "runs do not connect directly to an endpoint")
		return
	}
	model := settings.Model
	if model == "" {
		model = cockpit.DefaultsFor(settings.API).Model
	}
	next := cockpit.Settings{API: cockpit.APIGateway, Model: model, Protocol: settings.API}
	if gateway.Serves(settings.BaseURL) {
		s.saveConnection(w, r, next)
		return
	}
	if settings.APIKey == "" {
		writeError(w, http.StatusConflict, "the key comes from the environment: add the endpoint with its key, then choose the model")
		return
	}
	in := accounts.EndpointInput{Kind: "anthropic", BaseURL: settings.BaseURL, APIKey: &settings.APIKey}
	if settings.API == cockpit.APIResponses {
		in.Kind = "openai"
	}
	// The endpoint serves what it lists, else the model runs use.
	if models, err := gateway.ProbeEndpoint(r.Context(), "", in); err == nil && len(models) > 0 {
		for _, m := range models {
			in.Models = append(in.Models, accounts.EndpointModel{Name: m})
		}
	} else {
		in.Models = []accounts.EndpointModel{{Name: model}}
	}
	if !slices.ContainsFunc(in.Models, func(m accounts.EndpointModel) bool { return m.Name == model }) {
		in.Models = append(in.Models, accounts.EndpointModel{Name: model})
	}
	if _, err := gateway.AddEndpoint(r.Context(), in); err != nil && !strings.Contains(err.Error(), "already") {
		accountsError(w, err)
		return
	}
	s.saveConnection(w, r, next)
}

// ---------------------------------------------------------------- endpoints

// endpointRequest is an endpoint as the browser's form sends it.
func (s *server) handleAddEndpoint(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var in accounts.EndpointInput
	if !decodeJSON(w, r, &in) {
		return
	}
	endpoint, err := gateway.AddEndpoint(r.Context(), in)
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, endpoint)
}

func (s *server) handleUpdateEndpoint(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var in accounts.EndpointInput
	if !decodeJSON(w, r, &in) {
		return
	}
	endpoint, err := gateway.UpdateEndpoint(r.Context(), r.PathValue("id"), in)
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, endpoint)
}

func (s *server) handleSwitchEndpoint(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var change struct {
		Disabled *bool `json:"disabled"`
	}
	if !decodeJSON(w, r, &change) {
		return
	}
	if change.Disabled == nil {
		writeError(w, http.StatusBadRequest, "nothing to change: send disabled")
		return
	}
	if err := gateway.SetEndpointDisabled(r.Context(), r.PathValue("id"), *change.Disabled); err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"id": r.PathValue("id"), "disabled": *change.Disabled})
}

func (s *server) handleRemoveEndpoint(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.RemoveEndpoint(r.Context(), r.PathValue("id")); err != nil {
		accountsError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// handleProbeEndpoint lists the models an upstream serves, with the key the
// form gives or, for an endpoint the gateway has (id), with its own.
func (s *server) handleProbeEndpoint(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var probe struct {
		accounts.EndpointInput
		ID string `json:"id"`
	}
	if !decodeJSON(w, r, &probe) {
		return
	}
	models, err := gateway.ProbeEndpoint(r.Context(), probe.ID, probe.EndpointInput)
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"models": models})
}

// ---------------------------------------------------------------- sign-ins

func (s *server) handleStartSignIn(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var start struct {
		Provider string `json:"provider"`
	}
	if !decodeJSON(w, r, &start) {
		return
	}
	login, err := gateway.Login(r.Context(), start.Provider)
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, login)
}

func (s *server) handleSignIn(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	state, err := gateway.LoginState(r.Context(), r.PathValue("state"))
	if err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, state)
}

// handleFinishSignIn takes the address the provider sent the browser to,
// when its callback could not reach this machine.
func (s *server) handleFinishSignIn(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var callback struct {
		URL string `json:"url"`
	}
	if !decodeJSON(w, r, &callback) {
		return
	}
	if err := gateway.FinishLogin(r.Context(), r.PathValue("state"), callback.URL); err != nil {
		accountsError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"status": "wait"})
}

func (s *server) handleCancelSignIn(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.CancelLogin(r.Context(), r.PathValue("state")); err != nil {
		accountsError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
