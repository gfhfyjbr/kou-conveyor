package main

// The relay updates itself. CI publishes a build of it whenever what it is
// built from changes — its own packages, the modules they use, the Go
// release (.github/workflows/relay.yml) — with a manifest that names the
// newest. The relay reads the manifest now and then; when it names a newer
// build for its platform, it downloads it, checks its sum, has it serve on a
// spare port once, puts it in its own place, keeps itself beside as .old,
// and executes it in its stead: the same process, so a service manager sees
// nothing, and the cockpit connects again within seconds.

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
)

// defaultUpdateURL is the manifest CI keeps current.
const defaultUpdateURL = "https://github.com/gfhfyjbr/kou-conveyor/releases/download/relay-latest/manifest.json"

// updatedFrom tells the build executed in a relay's stead what it took the
// place of.
const updatedFrom = "KOU_RELAY_UPDATED_FROM"

// builtCommit and builtTime are set by CI's build (-X main.builtCommit=…):
// the commit built and its time. Other builds say them in their build info.
var builtCommit, builtTime string

// manifest is what CI publishes of the newest build.
type manifest struct {
	Version string `json:"version"`
	Commit  string `json:"commit"`
	// Time is the commit's: a build is newer than another when it is.
	Time time.Time `json:"time"`
	// Source is a hash of what the build is made of.
	Source   string `json:"source,omitempty"`
	Protocol int    `json:"protocol"`
	// Assets are the builds, by platform: linux/amd64, darwin/arm64, …
	Assets map[string]asset `json:"assets"`
}

type asset struct {
	URL    string `json:"url"`
	SHA256 string `json:"sha256"`
	Size   int64  `json:"size"`
}

// build is what a relay knows of the build it runs.
type build struct {
	version, commit string
	time            time.Time
}

func ownBuild() build {
	b := build{version: currentVersion(), commit: builtCommit}
	if t, err := time.Parse(time.RFC3339, builtTime); err == nil {
		b.time = t
	}
	if info, ok := debug.ReadBuildInfo(); ok {
		for _, setting := range info.Settings {
			switch setting.Key {
			case "vcs.revision":
				if b.commit == "" {
					b.commit = setting.Value
				}
			case "vcs.time":
				if b.time.IsZero() {
					b.time, _ = time.Parse(time.RFC3339, setting.Value)
				}
			}
		}
	}
	return b
}

// platform is the key of this relay's builds in a manifest.
func platform() string { return runtime.GOOS + "/" + runtime.GOARCH }

// updater looks for newer builds and puts one in the relay's place.
type updater struct {
	manifestURL string
	every       time.Duration
	exe         string
	// args are the relay's own, to try a build with; secure, whether it
	// serves TLS with them.
	args   []string
	secure bool
	own    build
	// asked is the protocol a cockpit that speaks another one asked for.
	asked  func() int
	client *http.Client
	logf   func(format string, args ...any)

	failed map[string]int    // how often each build failed to be taken
	said   map[string]string // what was said last of each thing, said once
}

// tries is how often a build is tried before the relay gives it up: a
// download may fail once and do the next time.
const tries = 3

func newUpdater(manifestURL string, every time.Duration, exe string, args []string, secure bool, asked func() int, logf func(string, ...any)) *updater {
	return &updater{
		manifestURL: manifestURL, every: every, exe: exe, args: args, secure: secure,
		own: ownBuild(), asked: asked, logf: logf,
		client: &http.Client{Timeout: 2 * time.Minute},
		failed: map[string]int{}, said: map[string]string{},
	}
}

// once logs what is said of a thing unless it was said last.
func (u *updater) once(thing, format string, args ...any) {
	message := fmt.Sprintf(format, args...)
	if u.said[thing] == message {
		return
	}
	u.said[thing] = message
	u.logf("update: %s", message)
}

// run looks for a newer build every so often, from a minute after it
// starts; it gives the version it put in the relay's place, once it did,
// and gives up only with ctx.
func (u *updater) run(ctx context.Context) (string, bool) {
	wait := min(time.Minute, u.every)
	for {
		select {
		case <-ctx.Done():
			return "", false
		case <-time.After(wait):
		}
		wait = u.every
		if version, err := u.step(ctx); err != nil {
			u.once("step", "%v", err)
		} else if version != "" {
			return version, true
		}
	}
}

// step reads the manifest and takes the build it names when it is to be
// taken: it gives the version put in place, or "" when none was.
func (u *updater) step(ctx context.Context) (string, error) {
	m, err := u.read(ctx)
	if err != nil {
		return "", err
	}
	u.said["step"] = ""
	a, ok := u.wanted(m)
	if !ok {
		return "", nil
	}
	u.logf("update: %s (%s) is newer than %s: taking it", m.Version, m.Time.Format(time.RFC3339), u.own.version)
	path, err := u.fetch(ctx, m, a)
	if err == nil {
		err = u.try(ctx, path, m)
		if err == nil {
			err = u.install(path)
		}
		if err != nil {
			os.Remove(path)
		}
	}
	if err != nil {
		if ctx.Err() != nil {
			return "", err
		}
		u.failed[m.Version]++
		if u.failed[m.Version] >= tries {
			return "", fmt.Errorf("%s not taken, given up after %d tries: %w", m.Version, tries, err)
		}
		return "", fmt.Errorf("%s not taken: %w", m.Version, err)
	}
	return m.Version, nil
}

// read reads the manifest.
func (u *updater) read(ctx context.Context) (manifest, error) {
	var m manifest
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.manifestURL, nil)
	if err != nil {
		return m, err
	}
	req.Header.Set("User-Agent", tunnel.Name+"/"+u.own.version)
	req.Header.Set("Cache-Control", "no-cache")
	resp, err := u.client.Do(req)
	if err != nil {
		return m, fmt.Errorf("the manifest could not be read: %w", err)
	}
	defer resp.Body.Close()
	switch {
	case resp.StatusCode == http.StatusNotFound:
		return m, fmt.Errorf("no build is published yet at %s", u.manifestURL)
	case resp.StatusCode != http.StatusOK:
		return m, fmt.Errorf("the manifest at %s: %s", u.manifestURL, resp.Status)
	}
	if err := json.UnmarshalRead(io.LimitReader(resp.Body, 1<<20), &m); err != nil {
		return m, fmt.Errorf("the manifest at %s: %w", u.manifestURL, err)
	}
	return m, nil
}

// wanted says whether the build m names is to be taken, and which.
func (u *updater) wanted(m manifest) (asset, bool) {
	a, has := m.Assets[platform()]
	switch {
	case m.Version == "" || m.Version == u.own.version || u.failed[m.Version] >= tries:
		return a, false
	case m.Commit != "" && m.Commit == u.own.commit:
		// The same commit: this one is that build, or one made from the
		// checkout of it, changes and all, by hand.
		return a, false
	case !u.own.time.IsZero() && !m.Time.After(u.own.time):
		u.once("older", "%s (%s) is not newer than %s (%s): staying", m.Version, m.Time.Format(time.RFC3339), u.own.version, u.own.time.Format(time.RFC3339))
		return a, false
	case !has || a.URL == "" || a.SHA256 == "":
		u.once("platform", "%s has no build for %s: staying", m.Version, platform())
		return a, false
	case m.Protocol != tunnel.Protocol && m.Protocol != u.asked():
		// The cockpit would be turned away until it speaks it too.
		u.once("protocol", "%s speaks protocol %d, the cockpit %d: staying until the cockpit asks for %[2]d", m.Version, m.Protocol, tunnel.Protocol)
		return a, false
	}
	return a, true
}

// fetch downloads a build beside the relay's executable and checks its
// size and sum.
func (u *updater) fetch(ctx context.Context, m manifest, a asset) (string, error) {
	source, err := url.Parse(u.manifestURL)
	if err != nil {
		return "", err
	}
	location, err := source.Parse(a.URL)
	if err != nil {
		return "", err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, location.String(), nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("User-Agent", tunnel.Name+"/"+u.own.version)
	resp, err := u.client.Do(req)
	if err != nil {
		return "", fmt.Errorf("the download: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("the download of %s: %s", location, resp.Status)
	}
	path := filepath.Join(filepath.Dir(u.exe), "."+filepath.Base(u.exe)+".new")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o755)
	if err != nil {
		return "", fmt.Errorf("the relay cannot write beside itself: %w", err)
	}
	hash := sha256.New()
	limit := int64(256 << 20)
	if a.Size > 0 {
		limit = a.Size + 1
	}
	n, err := io.Copy(io.MultiWriter(file, hash), io.LimitReader(resp.Body, limit))
	if closed := file.Close(); err == nil {
		err = closed
	}
	sum := hex.EncodeToString(hash.Sum(nil))
	switch {
	case err != nil:
		err = fmt.Errorf("the download: %w", err)
	case a.Size > 0 && n != a.Size:
		err = fmt.Errorf("the download is %d bytes, not %d", n, a.Size)
	case !strings.EqualFold(sum, a.SHA256):
		err = fmt.Errorf("the download's sha-256 is %s, not %s", sum, a.SHA256)
	}
	if err != nil {
		os.Remove(path)
		return "", err
	}
	return path, os.Chmod(path, 0o755)
}

// try runs a build: it must say the version the manifest gave it, and
// serve, with the relay's own arguments, on a spare port of the loopback.
func (u *updater) try(ctx context.Context, path string, m manifest) error {
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, path, "-version").Output()
	if err != nil {
		return fmt.Errorf("it does not run here: %w", err)
	}
	if fields := strings.Fields(string(out)); len(fields) < 2 || fields[1] != m.Version {
		return fmt.Errorf("it says it is %q, not %s", strings.TrimSpace(string(out)), m.Version)
	}

	spare, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return err
	}
	address := spare.Addr().String()
	spare.Close()
	args := append(append([]string{}, u.args...), "-listen", address, "-update", "off")
	trial := exec.CommandContext(ctx, path, args...)
	trial.Env = withoutVariable(os.Environ(), updatedFrom)
	var output lockedBuffer
	trial.Stdout, trial.Stderr = &output, &output
	trial.WaitDelay = 3 * time.Second
	if err := trial.Start(); err != nil {
		return fmt.Errorf("it does not start: %w", err)
	}
	exited := make(chan error, 1)
	go func() { exited <- trial.Wait() }()
	defer func() {
		select {
		case <-exited:
		default:
			trial.Process.Signal(os.Interrupt)
			select {
			case <-exited:
			case <-time.After(3 * time.Second):
				trial.Process.Kill()
				<-exited
			}
		}
	}()

	scheme := "http"
	if u.secure {
		scheme = "https"
	}
	health := scheme + "://" + address + tunnel.HealthPath
	// The loopback, and a build whose sum was checked: its certificate is
	// the relay's own, which there is no need to check again.
	client := &http.Client{Timeout: 2 * time.Second, Transport: &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}}}
	for {
		select {
		case err := <-exited:
			exited <- err
			return fmt.Errorf("it stopped as it started (%v): %s", err, lastLines(output.String(), 5))
		case <-ctx.Done():
			return fmt.Errorf("it did not serve at %s in time: %s", health, lastLines(output.String(), 5))
		case <-time.After(200 * time.Millisecond):
		}
		resp, err := client.Get(health)
		if err != nil {
			continue
		}
		var said tunnel.Health
		err = json.UnmarshalRead(io.LimitReader(resp.Body, 1<<16), &said)
		resp.Body.Close()
		if err != nil || said.Relay != tunnel.Name {
			return fmt.Errorf("it answers at %s as something else than the relay", health)
		}
		return nil
	}
}

// install puts a build in the relay's place, and keeps the relay beside it
// as .old.
func (u *updater) install(path string) error {
	old := u.exe + ".old"
	os.Remove(old)
	if err := os.Link(u.exe, old); err != nil {
		if err := copyFile(u.exe, old); err != nil {
			return fmt.Errorf("the relay could not be kept as %s: %w", old, err)
		}
	}
	if err := os.Rename(path, u.exe); err != nil {
		return fmt.Errorf("the build could not take the relay's place: %w", err)
	}
	return nil
}

// restore puts the relay kept as .old back in its place.
func (u *updater) restore() error { return os.Rename(u.exe+".old", u.exe) }

func copyFile(from, to string) error {
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(to, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o755)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	return out.Close()
}

func withoutVariable(env []string, name string) []string {
	kept := env[:0:0]
	for _, kv := range env {
		if !strings.HasPrefix(kv, name+"=") {
			kept = append(kept, kv)
		}
	}
	return kept
}

// lockedBuffer keeps what a process writes, to be read while it runs: its
// first 64 KB, which is more than a build tried says.
type lockedBuffer struct {
	mu sync.Mutex
	b  strings.Builder
}

func (l *lockedBuffer) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if room := 64<<10 - l.b.Len(); room > 0 {
		l.b.Write(p[:min(len(p), room)])
	}
	return len(p), nil
}

func (l *lockedBuffer) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.String()
}

func lastLines(s string, n int) string {
	lines := strings.Split(strings.TrimSpace(s), "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	return strings.Join(lines, " | ")
}

// executable is the relay's own file, as it was when it started.
func executable() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	if resolved, err := filepath.EvalSymlinks(exe); err == nil {
		exe = resolved
	}
	return exe, nil
}

// parseEvery reads -update-every.
func parseEvery(s string) (time.Duration, error) {
	if d, err := time.ParseDuration(s); err == nil && d >= 10*time.Second {
		return d, nil
	}
	if n, err := strconv.Atoi(s); err == nil && n >= 10 {
		return time.Duration(n) * time.Second, nil
	}
	return 0, errors.New("-update-every must be a duration of 10s at least, such as 5m")
}
