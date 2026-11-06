package main

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httputil"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

const gatewayHostMode = "--internal-accounts-host"

type gatewayHostDescription struct {
	Dir     string          `json:"dir"`
	Address string          `json:"address"`
	Token   string          `json:"token"`
	Gateway cockpit.Gateway `json:"gateway"`
}

func gatewayHostPath(dir string) string { return filepath.Join(dir, "host.json") }

func loadGatewayHost(dir string) (gatewayHostDescription, error) {
	data, err := os.ReadFile(gatewayHostPath(dir))
	if err != nil {
		return gatewayHostDescription{}, err
	}
	var d gatewayHostDescription
	if err := json.Unmarshal(data, &d); err != nil {
		return d, err
	}
	if d.Dir != dir || d.Token == "" || d.Gateway.Owner == "" {
		return d, errors.New("the accounts host descriptor is incomplete")
	}
	return d, nil
}

func saveGatewayHost(d gatewayHostDescription) error {
	data, err := json.Marshal(d)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(d.Dir, ".host-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), gatewayHostPath(d.Dir))
}

type gatewayPeer struct {
	description gatewayHostDescription
	lease       *os.File
	client      *http.Client
}

func newGatewayPeer(d gatewayHostDescription, lease *os.File) *gatewayPeer {
	transport := &http.Transport{DisableKeepAlives: true, DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "unix", d.Gateway.LeaseSocket)
	}}
	return &gatewayPeer{description: d, lease: lease, client: &http.Client{Transport: transport}}
}

func (p *gatewayPeer) request(ctx context.Context, method, path string, value, into any) error {
	var body io.Reader
	if value != nil {
		data, err := json.Marshal(value)
		if err != nil {
			return err
		}
		body = bytes.NewReader(data)
	}
	req, err := http.NewRequestWithContext(ctx, method, "http://accounts"+path, body)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+p.description.Token)
	req.Header.Set("X-Kou-Gateway-Owner", p.description.Gateway.Owner)
	req.Header.Set("Content-Type", "application/json")
	res, err := p.client.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	data, err := io.ReadAll(io.LimitReader(res.Body, 8<<20))
	if err != nil {
		return err
	}
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		return fmt.Errorf("accounts host: %s: %s", res.Status, strings.TrimSpace(string(data)))
	}
	if into != nil && len(data) != 0 {
		return json.Unmarshal(data, into)
	}
	return nil
}

func (p *gatewayPeer) forward(w http.ResponseWriter, r *http.Request) {
	proxy := &httputil.ReverseProxy{
		Director: func(req *http.Request) {
			req.URL.Scheme, req.URL.Host, req.Host = "http", "accounts", "accounts"
			req.Header.Set("Authorization", "Bearer "+p.description.Token)
			req.Header.Set("X-Kou-Gateway-Owner", p.description.Gateway.Owner)
		},
		Transport: p.client.Transport,
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
			writeError(w, http.StatusServiceUnavailable, "the accounts host did not answer: "+err.Error())
		},
	}
	proxy.ServeHTTP(w, r)
}

func (s *server) hasGateway() bool { return s.gateway != nil || s.gatewayPeer != nil }

func (s *server) gatewayURL() string {
	if s.gatewayPeer != nil {
		return s.gatewayPeer.description.Gateway.URL
	}
	if s.gateway != nil {
		return s.gateway.URL()
	}
	return ""
}

func (s *server) gatewayServes(base string) bool {
	if s.gateway != nil {
		return s.gateway.Serves(base)
	}
	return s.gatewayPeer != nil && strings.TrimSuffix(strings.TrimRight(base, "/"), "/v1") == strings.TrimRight(s.gatewayURL(), "/")
}

func (s *server) gatewayStatus() accounts.Status {
	if s.gatewayPeer != nil {
		var status accounts.Status
		ctx, cancel := context.WithTimeout(s.ctx, 10*time.Second)
		defer cancel()
		if err := s.gatewayPeer.request(ctx, "GET", "/internal/status", nil, &status); err != nil {
			return accounts.Status{State: accounts.GatewayFailed, URL: s.gatewayURL(), Error: err.Error()}
		}
		return status
	}
	return s.gateway.Status()
}

func (s *server) gatewayRunEnv(ctx context.Context, model string) ([]string, error) {
	if s.gatewayPeer == nil {
		return append(s.contextEnv(model), s.cacheEnv(model)...), nil
	}
	var response struct {
		Env []string `json:"env"`
	}
	err := s.gatewayPeer.request(ctx, "POST", "/internal/run-environment", map[string]string{"model": model}, &response)
	if err == nil {
		return response.Env, nil
	}
	// Model overrides are persisted atomically. Reading these books never
	// writes them, and keeps unrelated direct providers independent of the
	// helper's transport without dropping their configured window or TTL.
	dir := s.gatewayPeer.description.Dir
	windows := accounts.NewContextBook(filepath.Join(dir, "context-windows.json"))
	ttls := accounts.NewCacheBook(filepath.Join(dir, "cache-ttls.json"))
	if readErr := errors.Join(windows.Err(), ttls.Err()); readErr != nil {
		return nil, errors.Join(err, readErr)
	}
	var env []string
	if window := windows.Resolve(model); window > 0 {
		env = append(env, runconfig.ContextWindowEnvironment+"="+strconv.FormatInt(window, 10))
	}
	if ttl := ttls.Resolve(model); ttl != "" {
		env = append(env, runconfig.CacheTTLEnvironment+"="+ttl)
	}
	return env, nil
}

// These routes depend only on the accounts host. Connection settings and the
// build/update actions stay on each front-end, outside the helper's mux.
func (s *server) gatewayRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/accounts/models", s.withGateway(s.handleGatewayModels))
	mux.HandleFunc("POST /api/accounts/import", s.withGateway(s.handleImportAccount))
	mux.HandleFunc("POST /api/endpoints", s.withGateway(s.handleAddEndpoint))
	mux.HandleFunc("POST /api/endpoints/probe", s.withGateway(s.handleProbeEndpoint))
	mux.HandleFunc("PUT /api/endpoints/{id}", s.withGateway(s.handleUpdateEndpoint))
	mux.HandleFunc("PATCH /api/endpoints/{id}", s.withGateway(s.handleSwitchEndpoint))
	mux.HandleFunc("DELETE /api/endpoints/{id}", s.withGateway(s.handleRemoveEndpoint))
	mux.HandleFunc("GET /api/accounts/{name}/quota", s.withGateway(s.handleAccountQuota))
	mux.HandleFunc("POST /api/accounts/{name}/refresh", s.withGateway(s.handleRefreshAccount))
	mux.HandleFunc("PATCH /api/accounts/{name}", s.withGateway(s.handleUpdateAccount))
	mux.HandleFunc("DELETE /api/accounts/{name}", s.withGateway(s.handleRemoveAccount))
	mux.HandleFunc("POST /api/sign-ins", s.withGateway(s.handleStartSignIn))
	mux.HandleFunc("GET /api/sign-ins/{state}", s.withGateway(s.handleSignIn))
	mux.HandleFunc("POST /api/sign-ins/{state}/callback", s.withGateway(s.handleFinishSignIn))
	mux.HandleFunc("DELETE /api/sign-ins/{state}", s.withGateway(s.handleCancelSignIn))
	mux.HandleFunc("GET /api/usage", s.withGateway(s.handleUsage))
	mux.HandleFunc("GET /api/usage/prices", s.withGateway(s.handlePrices))
	mux.HandleFunc("GET /api/usage/price", s.withGateway(s.handleModelPrices))
	mux.HandleFunc("POST /api/usage/prices", s.withGateway(s.handleSetPrice))
	mux.HandleFunc("DELETE /api/usage/prices", s.withGateway(s.handleRemovePrice))
	mux.HandleFunc("PUT /api/usage/unit", s.withGateway(s.handleSetUnit))
	mux.HandleFunc("POST /api/usage/prices/refresh", s.withGateway(s.handleRefreshPrices))
	mux.HandleFunc("GET /api/context-windows", s.withGateway(s.handleContextWindows))
	mux.HandleFunc("POST /api/context-windows", s.withGateway(s.handleSetContextWindow))
	mux.HandleFunc("DELETE /api/context-windows", s.withGateway(s.handleRemoveContextWindow))
	mux.HandleFunc("GET /api/context-windows/openrouter", s.withGateway(s.handleOpenRouterWindow))
	mux.HandleFunc("POST /api/cache-ttls", s.withGateway(s.handleSetCacheTTL))
	mux.HandleFunc("DELETE /api/cache-ttls", s.withGateway(s.handleRemoveCacheTTL))
}
