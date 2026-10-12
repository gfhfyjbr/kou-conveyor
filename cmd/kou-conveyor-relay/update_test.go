package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
)

func TestUpdateFlags(t *testing.T) {
	for _, name := range []string{"KOU_RELAY_UPDATE", "KOU_RELAY_UPDATE_URL", "KOU_RELAY_UPDATE_EVERY"} {
		t.Setenv(name, "")
	}
	dir := t.TempDir()
	o, err := parse([]string{"-dir", dir}, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	if o.update != "on" || o.updateURL != defaultUpdateURL || o.updateEvery != 5*time.Minute {
		t.Errorf("defaults: update %q, url %q, every %s", o.update, o.updateURL, o.updateEvery)
	}
	o, err = parse([]string{"-dir", dir, "-update", "off", "-update-url", "http://127.0.0.1:1/m.json", "-update-every", "30"}, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	if o.update != "off" || o.updateURL != "http://127.0.0.1:1/m.json" || o.updateEvery != 30*time.Second {
		t.Errorf("given: update %q, url %q, every %s", o.update, o.updateURL, o.updateEvery)
	}
	t.Setenv("KOU_RELAY_UPDATE", "off")
	if o, err := parse([]string{"-dir", dir}, io.Discard); err != nil || o.update != "off" {
		t.Errorf("KOU_RELAY_UPDATE=off: update %q, %v", o.update, err)
	}
	for _, args := range [][]string{
		{"-update", "maybe"},
		{"-update-url", "ftp://example.com/m.json"},
		{"-update-url", "manifest.json"},
		{"-update-every", "5"},
		{"-update-every", "1s"},
		{"-update-every", "soon"},
	} {
		if _, err := parse(append([]string{"-dir", dir}, args...), io.Discard); err == nil {
			t.Errorf("%q: no error", args)
		}
	}
}

func TestUpdaterWantsOnlyANewerBuildItCanTake(t *testing.T) {
	then := time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)
	own := build{version: "v0.0.0-20261001120000-aaaaaaaaaaaa", commit: "aaaa", time: then}
	newer := func(change func(*manifest)) manifest {
		m := manifest{
			Version: "v0.0.0-20261002120000-bbbbbbbbbbbb", Commit: "bbbb", Time: then.Add(24 * time.Hour),
			Protocol: tunnel.Protocol,
			Assets:   map[string]asset{platform(): {URL: "relay", SHA256: "00", Size: 1}},
		}
		if change != nil {
			change(&m)
		}
		return m
	}
	for _, c := range []struct {
		name   string
		own    build
		asked  int
		failed int
		m      manifest
		want   bool
	}{
		{name: "newer", own: own, m: newer(nil), want: true},
		{name: "the same version", own: own, m: newer(func(m *manifest) { m.Version = own.version })},
		{name: "the same commit", own: own, m: newer(func(m *manifest) { m.Commit = own.commit })},
		{name: "older", own: own, m: newer(func(m *manifest) { m.Time = then.Add(-time.Hour) })},
		{name: "as old", own: own, m: newer(func(m *manifest) { m.Time = then })},
		{name: "own time unknown", own: build{version: "devel"}, m: newer(nil), want: true},
		{name: "no build for this platform", own: own, m: newer(func(m *manifest) { m.Assets = map[string]asset{"plan9/mips": {URL: "x", SHA256: "00"}} })},
		{name: "no sum", own: own, m: newer(func(m *manifest) { m.Assets = map[string]asset{platform(): {URL: "relay"}} })},
		{name: "another protocol", own: own, m: newer(func(m *manifest) { m.Protocol = tunnel.Protocol + 1 })},
		{name: "another protocol, asked for", own: own, asked: tunnel.Protocol + 1, m: newer(func(m *manifest) { m.Protocol = tunnel.Protocol + 1 }), want: true},
		{name: "another protocol, another asked for", own: own, asked: tunnel.Protocol + 2, m: newer(func(m *manifest) { m.Protocol = tunnel.Protocol + 1 })},
		{name: "failed once", own: own, failed: 1, m: newer(nil), want: true},
		{name: "given up", own: own, failed: tries, m: newer(nil)},
		{name: "no version", own: own, m: newer(func(m *manifest) { m.Version = "" })},
	} {
		t.Run(c.name, func(t *testing.T) {
			u := &updater{own: c.own, asked: func() int { return c.asked }, logf: t.Logf,
				failed: map[string]int{c.m.Version: c.failed}, said: map[string]string{}}
			if _, got := u.wanted(c.m); got != c.want {
				t.Errorf("wanted %v, want %v", got, c.want)
			}
		})
	}
}

func TestUpdaterChecksTheDownload(t *testing.T) {
	body := []byte("a build of the relay")
	sum := sha256.Sum256(body)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/builds/relay" {
			http.NotFound(w, r)
			return
		}
		w.Write(body)
	}))
	defer server.Close()
	dir := t.TempDir()
	exe := filepath.Join(dir, tunnel.Name)
	u := &updater{manifestURL: server.URL + "/builds/manifest.json", exe: exe, client: server.Client(), logf: t.Logf}
	newFile := filepath.Join(dir, "."+tunnel.Name+".new")
	for _, c := range []struct {
		name  string
		asset asset
		err   string
	}{
		{name: "another sum", asset: asset{URL: "relay", SHA256: strings.Repeat("0", 64), Size: int64(len(body))}, err: "sha-256"},
		{name: "longer", asset: asset{URL: "relay", SHA256: hex.EncodeToString(sum[:]), Size: int64(len(body)) - 1}, err: "bytes"},
		{name: "shorter", asset: asset{URL: "relay", SHA256: hex.EncodeToString(sum[:]), Size: int64(len(body)) + 1}, err: "bytes"},
		{name: "missing", asset: asset{URL: "gone", SHA256: hex.EncodeToString(sum[:])}, err: "404"},
	} {
		if _, err := u.fetch(t.Context(), manifest{}, c.asset); err == nil || !strings.Contains(err.Error(), c.err) {
			t.Errorf("%s: %v, want an error with %q", c.name, err, c.err)
		}
		if _, err := os.Stat(newFile); !os.IsNotExist(err) {
			t.Errorf("%s: the download is left: %v", c.name, err)
		}
	}
	for _, url := range []string{"relay", server.URL + "/builds/relay", "/builds/relay"} {
		path, err := u.fetch(t.Context(), manifest{}, asset{URL: url, SHA256: strings.ToUpper(hex.EncodeToString(sum[:])), Size: int64(len(body))})
		if err != nil {
			t.Fatalf("%s: %v", url, err)
		}
		if path != newFile {
			t.Errorf("%s: downloaded to %s", url, path)
		}
		got, _ := os.ReadFile(path)
		info, _ := os.Stat(path)
		if !bytes.Equal(got, body) || info.Mode().Perm()&0o100 == 0 {
			t.Errorf("%s: downloaded %q, mode %s", url, got, info.Mode())
		}
	}
}

// buildRelay builds the relay as CI does, into dir.
func buildRelay(t *testing.T, dir, version, commit string, at time.Time) string {
	t.Helper()
	if testing.Short() {
		t.Skip("builds the relay")
	}
	gobin, err := exec.LookPath("go")
	if err != nil {
		t.Skip("no go on the PATH")
	}
	out := filepath.Join(dir, "relay-"+version)
	command := exec.Command(gobin, "build", "-trimpath", "-o", out,
		"-ldflags", "-X main.version="+version+" -X main.builtCommit="+commit+" -X main.builtTime="+at.Format(time.RFC3339), ".")
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("go build: %v\n%s", err, output)
	}
	return out
}

// publish serves a manifest that names build, as version, made at at.
func publish(t *testing.T, build, version, commit string, at time.Time) *httptest.Server {
	t.Helper()
	body, err := os.ReadFile(build)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(body)
	m := manifest{Version: version, Commit: commit, Time: at, Protocol: tunnel.Protocol,
		Assets: map[string]asset{platform(): {URL: "relay", SHA256: hex.EncodeToString(sum[:]), Size: int64(len(body))}}}
	data, err := json.Marshal(m)
	if err != nil {
		t.Fatal(err)
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/manifest.json":
			w.Write(data)
		case "/relay":
			w.Write(body)
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(server.Close)
	return server
}

func TestUpdaterTriesABuildBeforeItTakesIt(t *testing.T) {
	then := time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)
	const version = "v0.0.2-test"
	made := buildRelay(t, t.TempDir(), version, "bbbb", then)
	body, _ := os.ReadFile(made)
	for _, c := range []struct {
		name    string
		args    func(dir string) []string
		secure  bool
		version string
		err     string
	}{
		{name: "over TLS", secure: true, args: func(dir string) []string {
			return []string{"-listen", ":1", "-tls", "self-signed", "-names", "127.0.0.1", "-dir", dir}
		}},
		{name: "plain", args: func(dir string) []string { return []string{"-tls", "off", "-dir", dir} }},
		{name: "another version said", version: "v0.0.3-test", err: "says it is", args: func(dir string) []string {
			return []string{"-tls", "off", "-dir", dir}
		}},
		{name: "not serving", err: "stopped as it started", args: func(dir string) []string {
			return []string{"-tls-cert", filepath.Join(dir, "none.pem"), "-tls-key", filepath.Join(dir, "none.pem"), "-dir", dir}
		}},
	} {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			exe := filepath.Join(dir, tunnel.Name)
			if err := os.WriteFile(exe, []byte("the relay as it was"), 0o755); err != nil {
				t.Fatal(err)
			}
			said := version
			if c.version != "" {
				said = c.version
			}
			server := publish(t, made, said, "bbbb", then)
			u := newUpdater(server.URL+"/manifest.json", time.Minute, exe, c.args(filepath.Join(dir, "config")), c.secure,
				func() int { return 0 }, t.Logf)
			u.own = build{version: "v0.0.1-test", commit: "aaaa", time: then.Add(-time.Hour)}
			taken, err := u.step(t.Context())
			if _, statErr := os.Stat(filepath.Join(dir, "."+tunnel.Name+".new")); !os.IsNotExist(statErr) {
				t.Errorf("the download is left: %v", statErr)
			}
			now, _ := os.ReadFile(exe)
			if c.err != "" {
				if err == nil || !strings.Contains(err.Error(), c.err) {
					t.Fatalf("step: %q, %v; want an error with %q", taken, err, c.err)
				}
				if string(now) != "the relay as it was" || u.failed[said] != 1 {
					t.Errorf("the relay's file is %d bytes; failures %v", len(now), u.failed)
				}
				return
			}
			if err != nil || taken != version {
				t.Fatalf("step: %q, %v", taken, err)
			}
			if !bytes.Equal(now, body) {
				t.Error("the build is not in the relay's place")
			}
			if old, _ := os.ReadFile(exe + ".old"); string(old) != "the relay as it was" {
				t.Errorf("the relay is not kept as .old: %q", old)
			}
			if err := u.restore(); err != nil {
				t.Fatal(err)
			}
			if back, _ := os.ReadFile(exe); string(back) != "the relay as it was" {
				t.Error("restore did not put the relay back")
			}
		})
	}
}

// The relay that finds a newer build runs as it, in the same process and on
// the same port.
func TestRelayRestartsAsANewerBuild(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the relay updates itself on unix only")
	}
	then := time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)
	builds := t.TempDir()
	old := buildRelay(t, builds, "v0.0.1-test", "aaaa", then)
	newer := buildRelay(t, builds, "v0.0.2-test", "bbbb", then.Add(time.Hour))
	server := publish(t, newer, "v0.0.2-test", "bbbb", then.Add(time.Hour))

	dir := t.TempDir()
	exe := filepath.Join(dir, tunnel.Name)
	data, _ := os.ReadFile(old)
	if err := os.WriteFile(exe, data, 0o755); err != nil {
		t.Fatal(err)
	}
	spare, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := spare.Addr().String()
	spare.Close()
	config := filepath.Join(dir, "config")
	relay := exec.Command(exe, "-listen", address, "-tls", "off", "-dir", config,
		"-update-url", server.URL+"/manifest.json", "-update-every", "10s")
	relay.Env = withoutVariable(withoutVariable(os.Environ(), "KOU_RELAY_TOKEN"), "KOU_RELAY_UPDATE")
	var output lockedBuffer
	relay.Stdout, relay.Stderr = &output, &output
	if err := relay.Start(); err != nil {
		t.Fatal(err)
	}
	exited := make(chan error, 1)
	go func() { exited <- relay.Wait() }()
	defer func() {
		relay.Process.Signal(os.Interrupt)
		select {
		case <-exited:
		case <-time.After(5 * time.Second):
			relay.Process.Kill()
			<-exited
		}
		if t.Failed() {
			t.Logf("the relay said:\n%s", output.String())
		}
	}()

	health := func() (tunnel.Health, error) {
		var h tunnel.Health
		token, err := os.ReadFile(filepath.Join(config, "token"))
		if err != nil {
			return h, err
		}
		req, _ := http.NewRequest(http.MethodGet, "http://"+address+tunnel.HealthPath, nil)
		req.Header.Set("Authorization", "Bearer "+strings.TrimSpace(string(token)))
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			return h, err
		}
		defer resp.Body.Close()
		return h, json.UnmarshalRead(resp.Body, &h)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 45*time.Second)
	defer cancel()
	seen := ""
	for seen != "v0.0.2-test" {
		select {
		case err := <-exited:
			exited <- err
			t.Fatalf("the relay stopped: %v", err)
		case <-ctx.Done():
			t.Fatalf("the relay is %q, not the newer build, in time", seen)
		case <-time.After(250 * time.Millisecond):
		}
		if h, err := health(); err == nil {
			seen = h.Version
		}
	}
	said := output.String()
	for _, want := range []string{"taking it", "update: in the place of v0.0.1-test"} {
		if !strings.Contains(said, want) {
			t.Errorf("the relay did not say %q", want)
		}
	}
	if now, _ := os.ReadFile(exe); !bytes.Equal(now, mustRead(t, newer)) {
		t.Error("the newer build is not in the relay's place")
	}
	if kept, _ := os.ReadFile(exe + ".old"); !bytes.Equal(kept, data) {
		t.Error("the relay is not kept as .old")
	}
}

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}
