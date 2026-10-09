package main

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
)

const (
	tunnelTestToken = "tunnel-test-token-0123456789"
	// tunnelTestHost is the host a client of the relay names: no name of
	// the cockpit's own.
	tunnelTestHost = "phone.example:8420"
)

// tunnelRig is a cockpit whose tunnel goes to relays in this process.
type tunnelRig struct {
	*harness
	m    *tunnelManager
	path string // tunnel.json
}

func newTunnelRig(t *testing.T) *tunnelRig {
	t.Helper()
	// The tunnel of a cockpit that runs these tests is not theirs.
	t.Setenv("KOU_CONVEYOR_TUNNEL_CONFIG", "")
	h := newHarness(t)
	m := h.server.tunnel
	if m.path == "" || filepath.Dir(m.path) != filepath.Dir(h.server.opt.SettingsFile) {
		t.Fatalf("tunnel.json at %q, not beside the settings", m.path)
	}
	m.every = 20 * time.Millisecond
	ctx, cancel := context.WithCancel(t.Context())
	t.Cleanup(func() {
		cancel()
		m.running.Wait()
	})
	m.start(ctx, h.server.handler())
	return &tunnelRig{harness: h, m: m, path: m.path}
}

// newTestRelay runs a relay of token.
func newTestRelay(t *testing.T, token string) (*tunnel.Relay, *httptest.Server) {
	t.Helper()
	relay := tunnel.NewRelay(token, "test", nil)
	server := httptest.NewServer(relay)
	t.Cleanup(func() {
		relay.Close()
		server.Close()
	})
	return relay, server
}

// await waits until ok holds.
func (r *tunnelRig) await(what string, ok func() bool) {
	r.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for !ok() {
		if time.Now().After(deadline) {
			r.Fatalf("waited in vain for %s; the tunnel: %+v", what, r.m.view(false))
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func (r *tunnelRig) state() string { return r.m.view(false).State }

// viaRelay sends a request to the cockpit through a relay, as the phone
// does: with the relay's token, naming the relay's host.
func viaRelay(t *testing.T, base, method, path, body, token string) (*http.Response, map[string]any) {
	t.Helper()
	req, err := http.NewRequest(method, base+path, strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	req.Host = tunnelTestHost
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	if body != "" {
		req.Header.Set("Content-Type", "application/json")
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var decoded map[string]any
	if strings.HasPrefix(res.Header.Get("Content-Type"), "application/json") {
		if err := json.UnmarshalRead(res.Body, &decoded); err != nil {
			t.Fatal(err)
		}
	}
	return res, decoded
}

func runTunnelCLI(args ...string) (int, string, string) {
	var stdout, stderr bytes.Buffer
	code := tunnelMain(args, &stdout, &stderr)
	return code, stdout.String(), stderr.String()
}

// /tunnel's way: started before a relay is set up, the tunnel waits for
// one; once the agent has set it up (kou-conveyor-web tunnel set or
// install), the cockpit is carried through it, past its Host check, and
// follows tunnel.json from then on: stop, start, another relay.
func TestTunnelCarriesTheCockpit(t *testing.T) {
	r := newTunnelRig(t)
	if res, body := r.do("GET", "/api/tunnel", ""); res.StatusCode != http.StatusOK || body["state"] != "off" || body["configured"] != false || body["wanted"] != false {
		t.Fatalf("at first: %d %v", res.StatusCode, body)
	}

	// Started with no relay: the page is told to have the agent set it up.
	res, body := r.do("POST", "/api/tunnel", `{"action":"start"}`)
	check, _ := body["check"].(map[string]any)
	status, _ := body["tunnel"].(map[string]any)
	if res.StatusCode != http.StatusOK || check["ok"] != false || check["code"] != "not_configured" || status["wanted"] != true {
		t.Fatalf("start without a relay: %d %v", res.StatusCode, body)
	}
	r.await("setup", func() bool { return r.state() == "setup" })
	if res, _ := r.do("GET", "/api/tunnel/qr", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("a pairing code without a relay: %d", res.StatusCode)
	}

	// The agent sets it up.
	relay, relayServer := newTestRelay(t, tunnelTestToken)
	if code, out, stderr := runTunnelCLI("set", "-config", r.path, "-url", relayServer.URL+"/", "-token", tunnelTestToken, "-host", "me@relay"); code != 0 || !strings.Contains(out, "saved") {
		t.Fatalf("set: %d %s %s", code, out, stderr)
	}
	r.await("the tunnel up", func() bool { return r.state() == "up" })
	if connected, _ := relay.Connected(); !connected {
		t.Fatal("up, and the relay has no cockpit")
	}
	_, body = r.do("GET", "/api/tunnel", "")
	pair, _ := body["pair_link"].(string)
	if body["configured"] != true || body["url"] != relayServer.URL || body["host"] != "me@relay" || body["remote"] != false ||
		!strings.HasPrefix(pair, "kouconveyor://pair?") || !strings.Contains(pair, "token="+tunnelTestToken) {
		t.Fatalf("up: %v", body)
	}
	if res, _ := r.do("GET", "/api/tunnel/qr", ""); res.StatusCode != http.StatusOK || res.Header.Get("Content-Type") != "image/png" {
		t.Fatalf("the pairing code: %d %s", res.StatusCode, res.Header.Get("Content-Type"))
	}

	// Through the relay, the cockpit answers a host of the relay's, which
	// it refuses to anyone else.
	if res, body := viaRelay(t, relayServer.URL, "GET", "/api/tunnel", "", tunnelTestToken); res.StatusCode != http.StatusOK || body["remote"] != true || body["state"] != "up" {
		t.Fatalf("through the relay: %d %v", res.StatusCode, body)
	}
	if res, body := viaRelay(t, relayServer.URL, "POST", "/api/tunnel", `{"action":"check"}`, tunnelTestToken); res.StatusCode != http.StatusOK || body["check"].(map[string]any)["agent"] != true {
		t.Fatalf("a POST through the relay: %d %v", res.StatusCode, body)
	}
	if res, _ := viaRelay(t, relayServer.URL, "GET", "/api/sessions", "", ""); res.StatusCode != http.StatusUnauthorized {
		t.Fatalf("through the relay without its token: %d", res.StatusCode)
	}
	req, _ := http.NewRequest("GET", r.http.URL+"/api/sessions", nil)
	req.Host = tunnelTestHost
	if res, err := http.DefaultClient.Do(req); err != nil || res.StatusCode != http.StatusForbidden {
		t.Fatalf("the relay's host, not through it: %v %v", res, err)
	} else {
		res.Body.Close()
	}

	// Stopped, it lets the relay go, and says so in tunnel.json.
	if res, body := r.do("POST", "/api/tunnel", `{"action":"stop"}`); res.StatusCode != http.StatusOK || body["tunnel"].(map[string]any)["wanted"] != false {
		t.Fatalf("stop: %d %v", res.StatusCode, body)
	}
	r.await("the tunnel off", func() bool {
		connected, _ := relay.Connected()
		return r.state() == "off" && !connected
	})
	if c, err := tunnel.LoadConfig(r.path); err != nil || c.Enabled {
		t.Fatalf("tunnel.json after stop: %+v %v", c, err)
	}
	if res, body := viaRelay(t, relayServer.URL, "GET", "/api/sessions", "", tunnelTestToken); res.StatusCode != http.StatusServiceUnavailable || body["code"] != "agent_offline" {
		t.Fatalf("through the relay while off: %d %v", res.StatusCode, body)
	}

	// kou-conveyor-web tunnel start, from a terminal: the cockpit follows.
	if code, out, stderr := runTunnelCLI("start", "-config", r.path); code != 0 {
		t.Fatalf("start: %d %s %s", code, out, stderr)
	}
	r.await("the tunnel up again", func() bool {
		connected, _ := relay.Connected()
		return r.state() == "up" && connected
	})

	// Another relay in tunnel.json: the cockpit goes to it.
	const otherToken = "another-relay-token-9876543210"
	other, otherServer := newTestRelay(t, otherToken)
	if code, out, stderr := runTunnelCLI("set", "-config", r.path, "-url", otherServer.URL, "-token", otherToken); code != 0 {
		t.Fatalf("set another relay: %d %s %s", code, out, stderr)
	}
	r.await("the other relay", func() bool {
		left, _ := relay.Connected()
		joined, _ := other.Connected()
		return r.state() == "up" && joined && !left
	})
	if res, _ := viaRelay(t, otherServer.URL, "GET", "/api/config", "", otherToken); res.StatusCode != http.StatusOK {
		t.Fatalf("through the other relay: %d", res.StatusCode)
	}
	if v := r.m.view(false); v.URL != otherServer.URL || v.Host != "me@relay" {
		t.Fatalf("after the other relay: %+v", v)
	}
}

// Runs the agent makes are told where tunnel.json is, and how to call the
// tunnel's commands: the tunnel's skill uses them.
func TestTunnelRunEnvironment(t *testing.T) {
	m := newTunnelManager("/somewhere/tunnel.json", "auto")
	env := strings.Join(m.runEnv(), "\n")
	if !strings.Contains(env, "KOU_CONVEYOR_TUNNEL_CONFIG=/somewhere/tunnel.json") || !strings.Contains(env, "KOU_CONVEYOR_WEB=") {
		t.Fatalf("env = %q", env)
	}
	if (*tunnelManager)(nil).runEnv() != nil {
		t.Fatal("a cockpit without a tunnel gives its runs a tunnel")
	}
	if v := newTunnelManager("", "auto").view(false); v.State != "unavailable" {
		t.Fatalf("nowhere to keep tunnel.json: %+v", v)
	}
	t.Setenv("KOU_CONVEYOR_TUNNEL_CONFIG", "")
	if got := tunnelConfigPath("/config/kou-conveyor/settings.json"); got != "/config/kou-conveyor/tunnel.json" {
		t.Fatalf("tunnel.json at %q", got)
	}
	t.Setenv("KOU_CONVEYOR_TUNNEL_CONFIG", "/elsewhere/t.json")
	if got := tunnelConfigPath("/config/kou-conveyor/settings.json"); got != "/elsewhere/t.json" {
		t.Fatalf("tunnel.json at %q", got)
	}
}

func TestTunnelCommands(t *testing.T) {
	path := filepath.Join(t.TempDir(), "tunnel.json")
	var report tunnelReport
	decode := func(out string) tunnelReport {
		t.Helper()
		var r tunnelReport
		if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &r); err != nil {
			t.Fatalf("%v: %q", err, out)
		}
		return r
	}

	// Nothing set up yet.
	code, out, _ := runTunnelCLI("status", "-config", path, "-json")
	if report = decode(out); code != 1 || report.Code != "not_configured" || !strings.Contains(report.Hint, "tunnel install") {
		t.Fatalf("status of nothing: %d %s", code, out)
	}
	for _, command := range []string{"start", "stop", "link"} {
		if code, _, stderr := runTunnelCLI(command, "-config", path); code != 1 || !strings.Contains(stderr, "no relay is set up yet") {
			t.Fatalf("%s of nothing: %d %s", command, code, stderr)
		}
	}

	// A relay set by hand is kept, though it does not answer.
	code, out, _ = runTunnelCLI("set", "-config", path, "-url", "http://127.0.0.1:1/", "-token", tunnelTestToken,
		"-fingerprint", strings.Repeat("AB:", 31)+"AB", "-host", "me@relay", "-enable=false", "-json")
	if report = decode(out); code != 1 || report.Code != "unreachable" || report.Enabled || report.URL != "http://127.0.0.1:1" {
		t.Fatalf("set: %d %s", code, out)
	}
	saved, err := tunnel.LoadConfig(path)
	if err != nil || saved.Token != tunnelTestToken || saved.Fingerprint != strings.Repeat("ab", 32) || saved.Host != "me@relay" || saved.Enabled {
		t.Fatalf("saved: %+v %v", saved, err)
	}
	if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("tunnel.json, which holds the token: %v %v", info.Mode(), err)
	}
	// What set is not given stays as it was; what is wrong is refused.
	runTunnelCLI("set", "-config", path, "-host", "other@relay", "-enable=false")
	if saved, err = tunnel.LoadConfig(path); err != nil || saved.Token != tunnelTestToken || saved.Host != "other@relay" || saved.Fingerprint == "" {
		t.Fatalf("after another set: %+v %v", saved, err)
	}
	if code, _, stderr := runTunnelCLI("set", "-config", path, "-fingerprint", "xyz"); code != 2 || !strings.Contains(stderr, "fingerprint") {
		t.Fatalf("a bad fingerprint: %d %s", code, stderr)
	}
	if code, _, _ := runTunnelCLI("set", "-config", path, "-token", "short"); code != 1 {
		t.Fatalf("a short token: %d", code)
	}

	for _, command := range []string{"start", "stop"} {
		if code, _, stderr := runTunnelCLI(command, "-config", path); code != 0 {
			t.Fatalf("%s: %d %s", command, code, stderr)
		}
		if saved, _ := tunnel.LoadConfig(path); saved.Enabled != (command == "start") {
			t.Fatalf("after %s: %+v", command, saved)
		}
	}

	saved, _ = tunnel.LoadConfig(path)
	if code, out, _ := runTunnelCLI("link", "-config", path, "-qr=false"); code != 0 || strings.TrimSpace(out) != saved.PairLink() {
		t.Fatalf("link: %d %q", code, out)
	}
	if code, out, _ := runTunnelCLI("link", "-config", path, "-browser", "-qr=false"); code != 0 || strings.TrimSpace(out) != "http://127.0.0.1:1/?kou_token="+tunnelTestToken {
		t.Fatalf("link -browser: %d %q", code, out)
	}
	if code, out, _ := runTunnelCLI("link", "-config", path, "-qr"); code != 0 || !strings.ContainsAny(out, "▀▄█") || !strings.Contains(out, saved.PairLink()) {
		t.Fatalf("link -qr: %d %q", code, out)
	}

	// install takes nothing a shell on the host would read as more than an
	// ssh destination, and checks its options before it calls ssh.
	for _, args := range [][]string{
		{"me@host; rm -rf ~"},
		{"-port", "0", "me@host"},
		{"-tls", "maybe", "me@host"},
		{"-service", "cron", "me@host"},
		{"-binary", filepath.Join(t.TempDir(), "none"), "me@host"},
		{"-oProxyCommand=sh"},
		{},
	} {
		if code, _, _ := runTunnelCLI(append([]string{"install", "-config", path}, args...)...); code != 2 {
			t.Fatalf("install %v: %d", args, code)
		}
	}
	if code, _, _ := runTunnelCLI("dance"); code != 2 {
		t.Fatalf("an unknown command: %d", code)
	}
}

func TestRelayCommandLine(t *testing.T) {
	in := &relayInstall{port: 9000, tls: "self-signed"}
	if got, want := in.relayCommand("https://relay.example.com:9000", true), "%h/.local/bin/kou-conveyor-relay -listen :9000 -tls self-signed -dir %h/.config/kou-conveyor-relay -names relay.example.com"; got != want {
		t.Fatalf("unit: %q, want %q", got, want)
	}
	in.tls = "off"
	if got, want := in.relayCommand("http://relay:9000", false), `"$HOME"/.local/bin/kou-conveyor-relay -listen :9000 -tls off -dir "$HOME"/.config/kou-conveyor-relay`; got != want {
		t.Fatalf("shell: %q, want %q", got, want)
	}
	in.url = "ftp://relay"
	if _, err := in.relayURL(); err == nil {
		t.Fatal("an ftp relay")
	}
	in.url = "https://relay.example.com:9000"
	if got, err := in.relayURL(); err != nil || got != in.url {
		t.Fatalf("-url: %q %v", got, err)
	}
}
