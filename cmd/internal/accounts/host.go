package accounts

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"runtime/debug"
	"strings"
	"sync"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy"
	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/usage"
	log "github.com/sirupsen/logrus"
	"gopkg.in/natefinch/lumberjack.v2"
)

// Module is the Go module of the gateway.
const Module = "github.com/router-for-me/CLIProxyAPI/v7"

// Gateway states.
const (
	GatewayStarting = "starting"
	GatewayRunning  = "running"
	GatewayFailed   = "failed"
	GatewayStopped  = "stopped"
)

// Options configure a Host.
type Options struct {
	// Dir holds the gateway's config.yaml, the credentials (auths/), its log
	// (logs/) and the cockpit's request history.
	Dir string
	// Address is where the gateway listens, such as 127.0.0.1:8318.
	Address string
}

// Host runs the gateway inside the cockpit and answers for its accounts.
// The gateway keeps process-wide state, so a process runs one Host, once.
type Host struct {
	opt        Options
	host       string
	port       int
	password   string // the management API's, for this process only
	history    *History
	quotas     *quotas
	configPath string
	// callbacks has redirect sign-ins listen for the provider's redirect.
	callbacks bool
	// edits takes the gateway's lists one change at a time (putList).
	edits sync.Mutex
	// clients are the accounts and keys that registered models (models.go).
	clients *clientWatch

	mu      sync.Mutex
	state   string
	err     string
	since   time.Time
	apiKey  string
	client  *Client
	logins  map[string]time.Time // sign-ins started here: when, by state
	summary Summary              // as of the latest listing
	ready   chan struct{}        // closed once the gateway answers
	done    chan struct{}
}

// New prepares a gateway; Run starts it.
func New(opt Options) (*Host, error) {
	host, port, err := listenAddress(opt.Address)
	if err != nil {
		return nil, err
	}
	if opt.Dir == "" {
		return nil, errors.New("the gateway needs a directory")
	}
	dir, err := filepath.Abs(opt.Dir)
	if err != nil {
		return nil, err
	}
	opt.Dir = dir
	return &Host{
		opt: opt, host: host, port: port, password: rand.Text() + rand.Text(),
		history: NewHistory(filepath.Join(dir, "history.json")), quotas: newQuotas(),
		configPath: filepath.Join(dir, "config.yaml"), callbacks: true, clients: newClientWatch(),
		state: GatewayStarting, logins: map[string]time.Time{}, ready: make(chan struct{}), done: make(chan struct{}),
	}, nil
}

// Run runs the gateway until ctx ends. A gateway that fails stays failed:
// the gateway's request accounting stops for good with the first service
// that stops, so it is never started twice in a process.
func (h *Host) Run(ctx context.Context) {
	defer close(h.done)
	err := h.run(ctx)
	h.mu.Lock()
	defer h.mu.Unlock()
	h.client = nil
	switch {
	case err != nil && ctx.Err() == nil:
		h.state, h.err = GatewayFailed, err.Error()
	default:
		h.state = GatewayStopped
	}
}

// Ready is closed once the gateway answers.
func (h *Host) Ready() <-chan struct{} { return h.ready }

// Done is closed once Run has returned.
func (h *Host) Done() <-chan struct{} { return h.done }

// Wait waits for Run to return, or for ctx to end.
func (h *Host) Wait(ctx context.Context) bool {
	select {
	case <-h.done:
		return true
	case <-ctx.Done():
		return false
	}
}

func (h *Host) run(ctx context.Context) error {
	authDir := filepath.Join(h.opt.Dir, "auths")
	if err := os.MkdirAll(authDir, 0o700); err != nil {
		return err
	}
	cfg, key, err := ensureConfig(h.configPath, authDir, h.host, h.port)
	if err != nil {
		return err
	}
	// The gateway logs through logrus, to standard output unless told
	// otherwise; the cockpit's console is for the cockpit.
	log.SetOutput(&lumberjack.Logger{Filename: filepath.Join(h.opt.Dir, "logs", "gateway.log"), MaxSize: 10, MaxBackups: 2})
	// A service that fails to listen stops the accounting for the process,
	// so the port is tried first.
	listener, err := net.Listen("tcp", net.JoinHostPort(h.host, fmt.Sprint(h.port)))
	if err != nil {
		return fmt.Errorf("%w; start the cockpit with -accounts <host:port> to use another port", err)
	}
	listener.Close()

	usage.RegisterNamedPlugin("kou-conveyor-history", h.history)
	// The registry says which accounts and keys serve models as they
	// register them, before the service takes up the first.
	cliproxy.SetGlobalModelRegistryHook(h.clients)
	service, err := cliproxy.NewBuilder().WithConfig(cfg).WithConfigPath(h.configPath).WithLocalManagementPassword(h.password).Build()
	if err != nil {
		return err
	}
	runCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	stopped := make(chan error, 1)
	go func() { stopped <- service.Run(runCtx) }()
	go h.history.keep(runCtx, 30*time.Second)

	client := newClient("http://"+loopback(h.host, h.port), h.password)
	deadline := time.Now().Add(30 * time.Second)
	for {
		probe, stop := context.WithTimeout(ctx, 2*time.Second)
		err := client.ready(probe)
		stop()
		if err == nil {
			break
		}
		select {
		case err := <-stopped:
			return startError(err)
		case <-ctx.Done():
			return nil
		case <-time.After(100 * time.Millisecond):
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("the gateway did not answer: %w", err)
		}
	}
	h.mu.Lock()
	h.client, h.apiKey, h.state, h.since = client, key, GatewayRunning, time.Now().UTC()
	h.mu.Unlock()
	close(h.ready)

	err = <-stopped
	_ = h.history.Save()
	if errors.Is(err, context.Canceled) || ctx.Err() != nil {
		return nil
	}
	return startError(err)
}

func startError(err error) error {
	if err == nil {
		return errors.New("the gateway stopped")
	}
	return err
}

// Status is how the gateway stands.
type Status struct {
	State   string    `json:"state"`
	Error   string    `json:"error,omitzero"`
	Since   time.Time `json:"since,omitzero"`
	Address string    `json:"address"`
	// URL is where clients reach the gateway, without /v1.
	URL     string `json:"url"`
	Version string `json:"version,omitzero"`
	Dir     string `json:"dir"`
	Config  string `json:"config"`
	// KeyHint shows enough of the API key runs present to recognize it.
	KeyHint string  `json:"key_hint,omitzero"`
	Summary Summary `json:"summary"`
}

// Status reports on the gateway.
func (h *Host) Status() Status {
	h.mu.Lock()
	defer h.mu.Unlock()
	s := Status{
		State: h.state, Error: h.err, Since: h.since, Address: net.JoinHostPort(h.host, fmt.Sprint(h.port)),
		URL: h.URL(), Version: Version(), Dir: h.opt.Dir, Config: h.configPath, Summary: h.summary,
	}
	if h.apiKey != "" {
		s.KeyHint = "…" + h.apiKey[max(0, len(h.apiKey)-4):]
	}
	return s
}

// URL is where clients reach the gateway, without /v1.
func (h *Host) URL() string { return "http://" + loopback(h.host, h.port) }

// APIKey is the key runs present to the gateway, once it runs.
func (h *Host) APIKey() string {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.apiKey
}

// Serves reports whether a base URL points at the gateway.
func (h *Host) Serves(base string) bool {
	u, err := url.Parse(strings.TrimSpace(base))
	if err != nil || u.Host == "" {
		return false
	}
	host, port := u.Hostname(), u.Port()
	if port == "" {
		return false
	}
	if host == "localhost" {
		host = "127.0.0.1"
	}
	mine, err := url.Parse(h.URL())
	if err != nil {
		return false
	}
	ip := net.ParseIP(host)
	return port == mine.Port() && (host == mine.Hostname() || ip != nil && ip.IsLoopback() && net.ParseIP(mine.Hostname()).IsLoopback())
}

// ErrNotRunning is the answer while the gateway does not run.
var ErrNotRunning = errors.New("the accounts gateway is not running")

func (h *Host) running() (*Client, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.client == nil {
		if h.err != "" {
			return nil, fmt.Errorf("%w: %s", ErrNotRunning, h.err)
		}
		return nil, ErrNotRunning
	}
	return h.client, nil
}

// Spans the cockpit shows an account's uptime over, in slots.
var spans = map[string]struct {
	span  time.Duration
	slots int
}{
	"24h": {24 * time.Hour, 48},
	"7d":  {7 * 24 * time.Hour, 42},
}

// ValidSpan reports whether the cockpit shows uptime over span.
func ValidSpan(span string) bool {
	_, ok := spans[span]
	return ok
}

// Overview is everything the gateway serves from: the accounts signed in,
// the endpoints reached with a key, and a summary of both.
type Overview struct {
	Accounts  []Account  `json:"accounts"`
	Endpoints []Endpoint `json:"endpoints"`
	// Summary counts both, and their requests of the last day.
	Summary Summary `json:"summary"`
}

// Accounts lists the gateway's accounts with their uptime over span ("24h"
// or "7d"), their latest errors and their latest known quota.
func (h *Host) Accounts(ctx context.Context, span string) ([]Account, error) {
	o, err := h.Overview(ctx, span)
	return o.Accounts, err
}

// Overview lists the accounts and the endpoints, with their uptime over
// span ("24h" or "7d") and their latest errors, the accounts with their
// latest known quota.
func (h *Host) Overview(ctx context.Context, span string) (Overview, error) {
	client, err := h.running()
	if err != nil {
		return Overview{}, err
	}
	records, err := client.authFiles(ctx)
	if err != nil {
		return Overview{}, err
	}
	endpoints, err := client.endpoints(ctx)
	if err != nil {
		return Overview{}, err
	}
	window, ok := spans[span]
	if !ok {
		span, window = "24h", spans["24h"]
	}
	now := time.Now()
	reg := cliproxy.GlobalModelRegistry()
	list := make([]Account, 0, len(records))
	for _, r := range records {
		a := accountOf(r, now)
		// The gateway registers an account's models a moment after it takes
		// the account up; until then the account serves none.
		a.Models = accountModels(reg, a.ID)
		h.clients.add(a.ID)
		a.Uptime = h.history.Uptime(a.ID, a.Index, window.span, window.slots)
		a.Errors = h.history.Errors(a.ID, a.Index, 5)
		if quota, ok := h.quotas.cached(a.Name); ok && quota.Error == "" {
			a.Quota = &quota
		} else if observed, ok := observedQuota(a.Provider, r.obj("quota")); ok {
			a.Quota = &observed
		} else if ok {
			a.Quota = &quota
		}
		if a.Plan == "" && a.Quota != nil {
			a.Plan = a.Quota.Plan
		}
		list = append(list, a)
	}
	// An endpoint's state comes from its requests: failing while its latest
	// failed.
	known := map[string]bool{}
	for i := range endpoints {
		e := &endpoints[i]
		e.Uptime = h.history.UptimeOf(e.indexes, window.span, window.slots)
		e.Errors = h.history.ErrorsOf(e.indexes, 5)
		if e.State == StateReady && e.Uptime.Failed > 0 && e.Uptime.LastFailure.After(e.Uptime.LastOK) {
			e.State = StateError
		}
		for _, index := range e.indexes {
			known["index:"+index] = true
		}
	}
	// Keys of the configuration the cockpit does not edit, such as Vertex
	// keys, show from the requests they served in the span.
	for _, a := range list {
		known[a.ID], known["index:"+a.Index] = true, true
	}
	for _, k := range h.history.Known() {
		if known[k.id] || k.index != "" && known["index:"+k.index] || strings.HasSuffix(strings.ToLower(k.id), ".json") {
			continue
		}
		a := configuredAccount(k)
		if a.Uptime = h.history.Uptime(k.id, k.index, window.span, window.slots); a.Uptime.OK+a.Uptime.Failed == 0 {
			continue
		}
		a.Errors = h.history.Errors(k.id, k.index, 5)
		a.Models = accountModels(reg, a.ID)
		list = append(list, a)
	}
	sortAccounts(list)
	o := Overview{Accounts: list, Endpoints: endpoints}
	o.Summary = h.summarize(o, span)
	h.mu.Lock()
	h.summary = o.Summary
	h.mu.Unlock()
	return o, nil
}

// summarize counts an overview, with the requests of the last day whatever
// span it shows.
func (h *Host) summarize(o Overview, span string) Summary {
	day := spans["24h"]
	accounts := make([]Account, len(o.Accounts))
	for i, a := range o.Accounts {
		accounts[i] = a
		if span != "24h" {
			accounts[i].Uptime = h.history.Uptime(a.ID, a.Index, day.span, day.slots)
		}
	}
	s := Summarize(accounts)
	s.Endpoints = len(o.Endpoints)
	for _, e := range o.Endpoints {
		u := e.Uptime
		if span != "24h" {
			u = h.history.UptimeOf(e.indexes, day.span, day.slots)
		}
		s.OK += u.OK
		s.Failed += u.Failed
		if e.State == StateError {
			s.FailingEndpoints++
		}
	}
	return s
}

// account finds one account by name.
func (h *Host) account(ctx context.Context, name string) (Account, *Client, error) {
	client, err := h.running()
	if err != nil {
		return Account{}, nil, err
	}
	records, err := client.authFiles(ctx)
	if err != nil {
		return Account{}, nil, err
	}
	for _, r := range records {
		if a := accountOf(r, time.Now()); a.Name == name {
			return a, client, nil
		}
	}
	return Account{}, nil, ErrNoAccount
}

// ErrNoAccount is the answer for an account the gateway does not have.
var ErrNoAccount = errors.New("no such account")

// Quota asks an account's provider what is left of its limits, unless a
// recent answer will do.
func (h *Host) Quota(ctx context.Context, name string, refresh bool) (Quota, error) {
	a, client, err := h.account(ctx, name)
	if err != nil {
		return Quota{}, err
	}
	if !a.QuotaSupported {
		return Quota{}, fmt.Errorf("%s accounts do not report their limits", a.ProviderName)
	}
	return h.quotas.get(ctx, client, a.target, refresh), nil
}

// Login starts signing in to provider.
func (h *Host) Login(ctx context.Context, provider string) (Login, error) {
	p, ok := ProviderByID(provider)
	if !ok {
		return Login{}, fmt.Errorf("unknown provider %q", provider)
	}
	client, err := h.running()
	if err != nil {
		return Login{}, err
	}
	login, err := client.startLogin(ctx, p, p.Flow == FlowRedirect && h.callbacks)
	// The callback port may be taken, by another sign-in or another
	// program; the user then brings the address the provider redirects to.
	if err != nil && p.Flow == FlowRedirect && StatusOf(err) >= 500 {
		login, err = client.startLogin(ctx, p, false)
	}
	if err != nil {
		return Login{}, err
	}
	h.mu.Lock()
	// The gateway gives up on a sign-in after minutes; one abandoned here
	// is forgotten after an hour.
	for state, started := range h.logins {
		if time.Since(started) > time.Hour {
			delete(h.logins, state)
		}
	}
	h.logins[login.State] = time.Now()
	h.mu.Unlock()
	return login, nil
}

// ErrNoLogin is the answer for a sign-in this cockpit did not start.
var ErrNoLogin = errors.New("no such sign-in")

// login is the gateway's client, for a sign-in started here.
func (h *Host) login(state string) (*Client, error) {
	client, err := h.running()
	if err != nil {
		return nil, err
	}
	h.mu.Lock()
	_, ok := h.logins[state]
	h.mu.Unlock()
	if !ok {
		return nil, ErrNoLogin
	}
	return client, nil
}

// LoginState reports how a sign-in stands.
func (h *Host) LoginState(ctx context.Context, state string) (LoginState, error) {
	client, err := h.login(state)
	if err != nil {
		return LoginState{}, err
	}
	s, err := client.loginState(ctx, state)
	if err == nil && s.Status != "wait" {
		h.mu.Lock()
		delete(h.logins, state)
		h.mu.Unlock()
	}
	return s, err
}

// FinishLogin completes a sign-in with the address the provider sent the
// browser to, for when that address is not reachable from the browser.
func (h *Host) FinishLogin(ctx context.Context, state, redirect string) error {
	client, err := h.login(state)
	if err != nil {
		return err
	}
	u, err := url.Parse(strings.TrimSpace(redirect))
	if err != nil || u.RawQuery == "" {
		return errors.New("paste the whole address from the browser, with its ?code=… part")
	}
	query := u.Query()
	if s := query.Get("state"); s != "" && s != state {
		return errors.New("that address belongs to another sign-in")
	}
	if query.Get("code") == "" && query.Get("error") == "" {
		return errors.New("the address has no code: sign in first, then copy the address the browser ends up on")
	}
	return client.finishLogin(ctx, state, u.String())
}

// CancelLogin abandons a sign-in.
func (h *Host) CancelLogin(ctx context.Context, state string) error {
	client, err := h.login(state)
	if err != nil {
		return err
	}
	h.mu.Lock()
	delete(h.logins, state)
	h.mu.Unlock()
	return client.cancelLogin(ctx, state)
}

// SetDisabled turns an account off or back on.
func (h *Host) SetDisabled(ctx context.Context, name string, disabled bool) error {
	a, client, err := h.account(ctx, name)
	if err != nil {
		return err
	}
	if a.Config {
		return errors.New("the gateway's configuration defines this account; change it there")
	}
	return client.setDisabled(ctx, name, disabled)
}

// Remove deletes an account's credential and its history.
func (h *Host) Remove(ctx context.Context, name string) error {
	a, client, err := h.account(ctx, name)
	if err != nil {
		return err
	}
	if a.Config {
		return errors.New("the gateway's configuration defines this account; remove it there")
	}
	if err := client.remove(ctx, name); err != nil {
		return err
	}
	h.history.Forget(a.ID)
	h.quotas.forget(name)
	return nil
}

// Refresh has the gateway get a new token for an account now.
func (h *Host) Refresh(ctx context.Context, name string) error {
	_, client, err := h.account(ctx, name)
	if err != nil {
		return err
	}
	return client.refresh(ctx, name)
}

// Import adds an account from a credential file of CLIProxyAPI's, such as
// one another installation signed in.
func (h *Host) Import(ctx context.Context, name string, data []byte) error {
	client, err := h.running()
	if err != nil {
		return err
	}
	name = strings.TrimSpace(name)
	if strings.ContainsAny(name, `/\`) || strings.HasPrefix(name, ".") || len(name) > 200 || !strings.HasSuffix(strings.ToLower(name), ".json") {
		return errors.New("a credential is a .json file, named without a folder")
	}
	return client.upload(ctx, name, data)
}

// Version is the gateway's version, as the build recorded it.
func Version() string {
	if info, ok := debug.ReadBuildInfo(); ok {
		for _, dep := range info.Deps {
			if dep.Path == Module {
				if dep.Replace != nil {
					return dep.Replace.Version
				}
				return dep.Version
			}
		}
	}
	return ""
}

// ValidAddress reports whether the gateway can listen on address.
func ValidAddress(address string) error {
	_, _, err := listenAddress(address)
	return err
}

// BaseURL is the base URL runs use to reach the gateway over an API: the
// Messages API's clients add /v1 themselves, the Responses API's do not.
func (h *Host) BaseURL(api string) string {
	if api == "responses" {
		return h.URL() + "/v1"
	}
	return h.URL()
}
