package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
	"rsc.io/qr"
)

// The tunnel carries this cockpit through a relay on another host, to the
// iPhone app and to browsers elsewhere, while it goes on here as it was
// (cmd/internal/tunnel). tunnel.json, beside the settings, says where the
// relay is; GET /api/tunnel says how the tunnel goes, and POST starts it,
// checks it, or stops it. What comes through it is served by the handler
// the pages here have, past the Host check: the relay checked its token.
//
// The cockpit follows tunnel.json as it runs: what kou-conveyor-web tunnel
// writes there — a relay the agent installed, start, stop — it takes up
// within seconds.

// tunnelKey marks the requests that came through the tunnel.
type tunnelKey struct{}

func viaTunnel(r *http.Request) bool {
	v, _ := r.Context().Value(tunnelKey{}).(bool)
	return v
}

// tunnelConfigPath is where tunnel.json is: KOU_CONVEYOR_TUNNEL_CONFIG, or
// beside the settings; "" when there is nowhere.
func tunnelConfigPath(settingsFile string) string {
	if path := strings.TrimSpace(os.Getenv("KOU_CONVEYOR_TUNNEL_CONFIG")); path != "" {
		if abs, err := filepath.Abs(path); err == nil {
			return abs
		}
		return path
	}
	if settingsFile == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(settingsFile), "tunnel.json")
}

// tunnelStatus is what /api/tunnel says.
type tunnelStatus struct {
	// State is off, setup (wanted, but no relay is configured yet),
	// connecting, up, retrying (after a failure, at RetryAt), or
	// unavailable (nowhere to keep tunnel.json).
	State string `json:"state"`
	// Wanted is that the tunnel was started and not stopped.
	Wanted     bool      `json:"wanted"`
	Configured bool      `json:"configured"`
	Error      string    `json:"error,omitempty"`
	Code       string    `json:"code,omitempty"`
	Since      time.Time `json:"since,omitzero"`
	RetryAt    time.Time `json:"retry_at,omitzero"`
	// What tunnel.json says, and where it is.
	URL         string `json:"url,omitempty"`
	Host        string `json:"host,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
	Config      string `json:"config,omitempty"`
	// PairLink pairs the iPhone app; BrowserLink opens the cockpit through
	// the relay. Both carry the relay's token.
	PairLink    string `json:"pair_link,omitempty"`
	BrowserLink string `json:"browser_link,omitempty"`
	// Remote is that the request asking came through the tunnel.
	Remote bool `json:"remote"`
}

// tunnelCheck is the answer of a check of the relay.
type tunnelCheck struct {
	OK    bool   `json:"ok"`
	Code  string `json:"code,omitempty"`
	Error string `json:"error,omitempty"`
	// The relay's version, and whether a cockpit is connected to it.
	Version string `json:"version,omitempty"`
	Agent   bool   `json:"agent,omitempty"`
}

type tunnelManager struct {
	path string
	// mode is what -tunnel said: on (from the start), off (until it is
	// started), or auto (as tunnel.json says, as it was when it last ran).
	mode string
	// every is how often tunnel.json is read again.
	every time.Duration

	mu   sync.Mutex
	want bool
	// enabled is what tunnel.json said of Enabled when it was last read or
	// written here: a change of it made elsewhere — kou-conveyor-web
	// tunnel start or stop, a relay the agent installed — starts or stops
	// the tunnel.
	enabled bool
	// relay is the relay the tunnel serves through, while it does; stop
	// ends that.
	relay  tunnel.Config
	stop   context.CancelFunc
	status tunnelStatus
	wake   chan struct{}
	// running is the tunnel's goroutines, which end with the context start
	// was given.
	running sync.WaitGroup
}

func newTunnelManager(path, mode string) *tunnelManager {
	m := &tunnelManager{path: path, mode: mode, every: 2 * time.Second, wake: make(chan struct{}, 1)}
	m.status = tunnelStatus{State: "off"}
	if path == "" {
		m.status.State = "unavailable"
	}
	return m
}

// start keeps the tunnel for as long as ctx: it comes up at once when
// -tunnel on says so, or when it was up as the cockpit last ran (auto).
func (m *tunnelManager) start(ctx context.Context, handler http.Handler) {
	if m == nil || m.path == "" {
		return
	}
	c, err := tunnel.LoadConfig(m.path)
	m.mu.Lock()
	m.enabled = err == nil && c.Enabled
	m.want = m.mode == "on" || m.mode == "auto" && m.enabled
	m.mu.Unlock()
	tunneled := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handler.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), tunnelKey{}, true)))
	})
	m.running.Go(func() { m.loop(ctx, tunneled) })
	m.running.Go(func() { m.follow(ctx) })
}

func (m *tunnelManager) wanted() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.want
}

func (m *tunnelManager) poke() {
	select {
	case m.wake <- struct{}{}:
	default:
	}
}

// pause waits d (or until poked, when d is 0).
func (m *tunnelManager) pause(ctx context.Context, d time.Duration) {
	var timer <-chan time.Time
	if d > 0 {
		t := time.NewTimer(d)
		defer t.Stop()
		timer = t.C
	}
	select {
	case <-ctx.Done():
	case <-m.wake:
	case <-timer:
	}
}

// set records the tunnel's state; it reports whether the state changed.
func (m *tunnelManager) set(state string, err error, retry time.Time) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	previous := m.status
	m.status = tunnelStatus{State: state, RetryAt: retry, Since: previous.Since}
	if state != previous.State {
		m.status.Since = time.Now()
	}
	if err != nil {
		m.status.Error, m.status.Code = err.Error(), tunnel.Code(err)
	}
	return state != previous.State
}

// say tells the terminal how the tunnel goes.
func (m *tunnelManager) say(format string, args ...any) {
	fmt.Printf("%s tunnel  %s\n", time.Now().Format("15:04:05"), fmt.Sprintf(format, args...))
}

func (m *tunnelManager) loop(ctx context.Context, handler http.Handler) {
	failures := 0
	for ctx.Err() == nil {
		if !m.wanted() {
			failures = 0
			if m.set("off", nil, time.Time{}) {
				m.say("off")
			}
			m.pause(ctx, 0)
			continue
		}
		config, err := tunnel.LoadConfig(m.path)
		if err != nil {
			// The agent, or the user, is setting it up: it comes up once
			// tunnel.json says where the relay is.
			err = notConfigured(m.path, err)
			if m.set("setup", err, time.Time{}) {
				m.say("waiting for a relay to be set up: %v", err)
			}
			m.pause(ctx, m.every)
			continue
		}
		m.set("connecting", nil, time.Time{})
		dial, cancel := context.WithTimeout(ctx, 20*time.Second)
		session, err := tunnel.Dial(dial, config)
		cancel()
		if err != nil {
			failures++
			delay := tunnelBackoff(failures)
			m.set("retrying", err, time.Now().Add(delay))
			if failures == 1 {
				m.say("%v; trying again", err)
			}
			m.pause(ctx, delay)
			continue
		}
		serve, stop := context.WithCancel(ctx)
		m.mu.Lock()
		wanted := m.want
		if wanted {
			m.stop, m.relay = stop, config
		}
		m.mu.Unlock()
		if !wanted { // stopped while it connected
			stop()
			session.Close()
			continue
		}
		m.set("up", nil, time.Time{})
		m.say("up: %s carries the cockpit", config.Base())
		// The next start of the cockpit brings it up again.
		m.persist(true)
		up := time.Now()
		err = tunnel.Serve(serve, session, handler)
		stop()
		m.mu.Lock()
		m.stop, m.relay = nil, tunnel.Config{}
		m.mu.Unlock()
		if ctx.Err() != nil || !m.wanted() {
			continue
		}
		if now, loadErr := tunnel.LoadConfig(m.path); loadErr == nil && !sameRelay(now, config) {
			// tunnel.json names another relay now: to it, at once.
			failures = 0
			m.say("down: tunnel.json names another relay, %s", now.Base())
			continue
		}
		if time.Since(up) > time.Minute {
			failures = 0
		}
		failures++
		delay := tunnelBackoff(failures)
		m.set("retrying", err, time.Now().Add(delay))
		m.say("down: %v; connecting again", err)
		m.pause(ctx, delay)
	}
}

// follow reads tunnel.json again and again: a change of Enabled made
// elsewhere starts or stops the tunnel, and another relay there has the
// tunnel go to it.
func (m *tunnelManager) follow(ctx context.Context) {
	ticker := time.NewTicker(m.every)
	defer ticker.Stop()
	last, _ := tunnel.LoadConfig(m.path)
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		c, err := tunnel.LoadConfig(m.path)
		enabled := err == nil && c.Enabled
		m.mu.Lock()
		changed := enabled != m.enabled
		m.enabled = enabled
		if changed {
			m.want = enabled
		}
		stop, relay, want := m.stop, m.relay, m.want
		m.mu.Unlock()
		if stop != nil && (!want || err != nil || !sameRelay(c, relay)) {
			stop()
		}
		if changed || err == nil && !sameRelay(c, last) {
			m.poke()
		}
		last = c
	}
}

// persist has tunnel.json say whether the tunnel is wanted: for the next
// start of the cockpit, and for kou-conveyor-web tunnel status.
func (m *tunnelManager) persist(enabled bool) {
	c, err := tunnel.LoadConfig(m.path)
	if err != nil || c.Enabled == enabled {
		return
	}
	c.Enabled = enabled
	if err := tunnel.SaveConfig(m.path, c); err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: tunnel:", err)
		return
	}
	m.mu.Lock()
	m.enabled = enabled
	m.mu.Unlock()
}

func sameRelay(a, b tunnel.Config) bool {
	return a.Base() == b.Base() && a.Token == b.Token && a.Fingerprint == b.Fingerprint
}

func tunnelBackoff(failures int) time.Duration {
	delay := time.Second << min(failures-1, 5)
	return min(delay, 30*time.Second)
}

// setWant starts or stops the tunnel. Stopped, it stays off when the
// cockpit starts again.
func (m *tunnelManager) setWant(want bool) {
	m.mu.Lock()
	m.want = want
	stop := m.stop
	m.mu.Unlock()
	m.persist(want)
	if !want && stop != nil {
		stop()
	}
	m.poke()
}

func (m *tunnelManager) view(remote bool) tunnelStatus {
	m.mu.Lock()
	status := m.status
	status.Wanted = m.want
	m.mu.Unlock()
	status.Remote, status.Config = remote, m.path
	if m.path == "" {
		return status
	}
	if c, err := tunnel.LoadConfig(m.path); err == nil {
		status.Configured = true
		status.URL, status.Host, status.Fingerprint = c.Base(), c.Host, c.Fingerprint
		status.PairLink, status.BrowserLink = c.PairLink(), c.BrowserLink()
	}
	return status
}

// check asks the relay tunnel.json names whether the tunnel can come up.
func (m *tunnelManager) check(ctx context.Context) tunnelCheck {
	config, err := tunnel.LoadConfig(m.path)
	if err != nil {
		err = notConfigured(m.path, err)
		return tunnelCheck{Code: tunnel.Code(err), Error: err.Error()}
	}
	ctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	health, err := tunnel.Check(ctx, config)
	if err != nil {
		return tunnelCheck{Code: tunnel.Code(err), Error: err.Error()}
	}
	return tunnelCheck{OK: true, Version: health.Version, Agent: health.Agent}
}

func notConfigured(path string, err error) error {
	if errors.Is(err, fs.ErrNotExist) {
		return &tunnel.Error{Code: "not_configured", Message: "no relay is set up yet (" + path + " is missing)"}
	}
	return &tunnel.Error{Code: "not_configured", Message: "the tunnel's configuration is unusable", Err: err}
}

// runEnv tells the agent's runs where tunnel.json is and how to call this
// program, for the tunnel's commands (kou-conveyor-web tunnel).
func (m *tunnelManager) runEnv() []string {
	if m == nil {
		return nil
	}
	var env []string
	if m.path != "" {
		env = append(env, "KOU_CONVEYOR_TUNNEL_CONFIG="+m.path)
	}
	if exe, err := os.Executable(); err == nil {
		env = append(env, "KOU_CONVEYOR_WEB="+exe)
	}
	return env
}

func (s *server) handleTunnel(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.tunnel.view(viaTunnel(r)))
}

// handleTunnelAction starts the tunnel ({"action":"start"}: wanted from now
// on, and up as soon as a relay is configured and reached; the answer
// checks the relay, so the page knows whether the agent has to set it up),
// stops it, or checks the relay.
func (s *server) handleTunnelAction(w http.ResponseWriter, r *http.Request) {
	m := s.tunnel
	if m.path == "" {
		writeError(w, http.StatusConflict, "the tunnel needs a user config directory to keep tunnel.json in; set KOU_CONVEYOR_TUNNEL_CONFIG")
		return
	}
	var req struct {
		Action string `json:"action"`
	}
	if err := json.UnmarshalRead(io.LimitReader(r.Body, 4<<10), &req); err != nil {
		writeError(w, http.StatusBadRequest, `expected {"action": "start", "stop" or "check"}`)
		return
	}
	var check *tunnelCheck
	switch req.Action {
	case "start":
		c := m.check(r.Context())
		check = &c
		m.setWant(true)
	case "stop":
		m.setWant(false)
	case "check":
		c := m.check(r.Context())
		check = &c
		m.poke() // a tunnel that waits to try again tries now
	default:
		writeError(w, http.StatusBadRequest, `the action must be "start", "stop" or "check"`)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"tunnel": m.view(viaTunnel(r)), "check": check})
}

// handleTunnelQR draws the link that pairs the iPhone app as a QR code, for
// the phone's camera.
func (s *server) handleTunnelQR(w http.ResponseWriter, r *http.Request) {
	c, err := tunnel.LoadConfig(s.tunnel.path)
	if s.tunnel.path == "" || err != nil {
		writeError(w, http.StatusNotFound, "no relay is set up")
		return
	}
	code, err := qr.Encode(c.PairLink(), qr.M)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	code.Scale = 8
	w.Header().Set("Content-Type", "image/png")
	w.Header().Set("Cache-Control", "no-store")
	w.Write(code.PNG())
}
