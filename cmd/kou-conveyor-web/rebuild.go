package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// A web cockpit built from a checkout follows its Go code the way it follows
// its plugins. When a .go file of the checkout changes — or go.mod, go.sum —
// it builds itself anew, with the runner and the terminal cockpit that sit
// beside it, and, once no agent runs and nothing waits in a queue, takes up
// the new build in place: the same process, the same port, whose socket the
// new build inherits (rebuild_unix.go). Open pages reconnect by themselves
// and keep what they show. A build that fails leaves the server running as
// it was and says why, in the terminal and in the pages.

// listenerEnvironment hands the listening socket to the new build.
const listenerEnvironment = "KOU_CONVEYOR_WEB_LISTENER_FD"

// sourceFingerprint fingerprints the Go code a self-made build was made of
// (-ldflags -X); a build made otherwise has none, and counts as the code
// the checkout has when it starts.
var sourceFingerprint string

// How long the code must stay as it is before a build starts: an editor
// saves several files one after another.
const rebuildSettle = 900 * time.Millisecond

// buildStatus is what the pages are told of the server's own builds.
type buildStatus struct {
	State   string    `json:"state"` // idle, building, failed, waiting, restarting
	Message string    `json:"message,omitzero"`
	Output  string    `json:"output,omitzero"` // the compiler's, when a build failed
	At      time.Time `json:"at"`
}

// rebuilder builds the server from its checkout as the checkout changes.
type rebuilder struct {
	root string // the checkout
	dir  string // where the programs are: the server's own directory
	exe  string // the server's own program

	mu     sync.Mutex
	status buildStatus
}

// programs are built in this order; beside the server, those there are.
var programs = []string{"kou-conveyor-runner", "kou-conveyor-web", "kou-conveyor-tui"}

// newRebuilder returns a rebuilder for a server whose assets are read from
// a checkout, or nil when there is none to build from.
func newRebuilder(a *assets) *rebuilder {
	if a == nil || a.dir == "" {
		return nil
	}
	root := filepath.Dir(filepath.Dir(a.dir))
	module, err := os.ReadFile(filepath.Join(root, "go.mod"))
	if err != nil || !strings.Contains(string(module), "module github.com/gfhfyjbr/kou-conveyor\n") {
		return nil
	}
	exe, err := os.Executable()
	if err != nil {
		return nil
	}
	if resolved, err := filepath.EvalSymlinks(exe); err == nil {
		exe = resolved
	}
	return &rebuilder{root: root, dir: filepath.Dir(exe), exe: exe, status: buildStatus{State: "idle", At: time.Now()}}
}

// goFingerprint fingerprints the Go code of the programs: the .go files of
// cmd/, harness/ and internal/ but tests, and go.mod and go.sum.
func goFingerprint(root string) string {
	digest := sha256.New()
	for _, file := range []string{"go.mod", "go.sum"} {
		if info, err := os.Stat(filepath.Join(root, file)); err == nil {
			fmt.Fprintf(digest, "%s %d %d\n", file, info.Size(), info.ModTime().UnixNano())
		}
	}
	for _, top := range []string{"cmd", "harness", "internal"} {
		_ = filepath.WalkDir(filepath.Join(root, top), func(path string, entry fs.DirEntry, err error) error {
			if err != nil {
				return nil
			}
			name := entry.Name()
			if entry.IsDir() {
				if strings.HasPrefix(name, ".") || name == "testdata" || name == "node_modules" || name == "plugins" || name == "static" {
					return filepath.SkipDir
				}
				return nil
			}
			if !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
				return nil
			}
			info, err := entry.Info()
			if err != nil {
				return nil
			}
			relative, _ := filepath.Rel(root, path)
			fmt.Fprintf(digest, "%s %d %d\n", filepath.ToSlash(relative), info.Size(), info.ModTime().UnixNano())
			return nil
		})
	}
	return hex.EncodeToString(digest.Sum(nil))[:20]
}

func (r *rebuilder) current() buildStatus {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.status
}

// set records what the build does now, says it in the terminal, and tells
// the pages.
func (s *server) setBuild(status buildStatus) {
	r := s.rebuild
	status.At = time.Now()
	r.mu.Lock()
	r.status = status
	r.mu.Unlock()
	if status.Message != "" {
		fmt.Printf("%s build  %s\n", status.At.Format("15:04:05"), status.Message)
	}
	if status.Output != "" {
		fmt.Println(indent(status.Output))
	}
	encoded, err := json.Marshal(status)
	if err != nil {
		return
	}
	for _, stream := range s.watch.snapshot() {
		stream.offer(pluginEvent{kind: "server", data: encoded})
	}
}

func indent(text string) string {
	lines := strings.Split(strings.TrimRight(text, "\n"), "\n")
	for i, line := range lines {
		lines[i] = "    " + line
	}
	return strings.Join(lines, "\n")
}

// followGo builds the server anew whenever its Go code changes, and takes
// up each build that succeeds, until ctx ends.
func (s *server) followGo(ctx context.Context, listener net.Listener) {
	r := s.rebuild
	built := sourceFingerprint
	if built == "" {
		built = goFingerprint(r.root)
	}
	seen, since := "", time.Time{}
	ticker := time.NewTicker(700 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		now := goFingerprint(r.root)
		if now == built {
			seen = ""
			continue
		}
		if now != seen {
			seen, since = now, time.Now() // the code is still changing
			continue
		}
		if time.Since(since) < rebuildSettle {
			continue
		}
		built, seen = now, ""
		if err := s.build(ctx, now); err != nil {
			continue // said; the next change builds again
		}
		s.takeUp(ctx, listener, now)
	}
}

// build builds the programs from the checkout, and puts them in place of
// those beside the server. A running program keeps the file it started
// from; the next to start gets the new one.
func (s *server) build(ctx context.Context, fingerprint string) error {
	r := s.rebuild
	s.setBuild(buildStatus{State: "building", Message: "the Go code changed: building the server anew"})
	gobin, err := exec.LookPath("go")
	if err != nil {
		s.setBuild(buildStatus{State: "failed", Message: "the Go code changed, but there is no go to build it with: " + err.Error()})
		return err
	}
	temporary, err := os.MkdirTemp("", "kou-conveyor-build-")
	if err != nil {
		s.setBuild(buildStatus{State: "failed", Message: "cannot build: " + err.Error()})
		return err
	}
	defer os.RemoveAll(temporary)
	var made []string
	for _, name := range programs {
		if name != "kou-conveyor-web" {
			if _, err := os.Stat(filepath.Join(r.dir, name)); err != nil {
				continue // not installed beside the server
			}
		}
		flags := ""
		if name == "kou-conveyor-web" {
			flags = fmt.Sprintf(`-X "main.builtFrom=%s" -X "main.sourceFingerprint=%s"`, r.root, fingerprint)
		}
		command := exec.CommandContext(ctx, gobin, "build", "-trimpath", "-ldflags", flags, "-o", filepath.Join(temporary, name), "./cmd/"+name)
		command.Dir = r.root
		output, err := command.CombinedOutput()
		if err != nil {
			s.setBuild(buildStatus{State: "failed", Message: fmt.Sprintf("the build of %s failed; the server goes on as it was", name), Output: strings.TrimSpace(lastLines(string(output), 30))})
			return err
		}
		made = append(made, name)
	}
	for _, name := range made {
		if err := place(filepath.Join(temporary, name), filepath.Join(r.dir, name)); err != nil {
			s.setBuild(buildStatus{State: "failed", Message: "cannot put the new build in place: " + err.Error()})
			return err
		}
	}
	return nil
}

func lastLines(text string, n int) string {
	lines := strings.Split(strings.TrimRight(text, "\n"), "\n")
	if len(lines) > n {
		lines = append([]string{"…"}, lines[len(lines)-n:]...)
	}
	return strings.Join(lines, "\n")
}

// place puts a program in place of another: copied beside it, then renamed
// over it, so a copy that runs keeps working.
func place(from, to string) error {
	source, err := os.Open(from)
	if err != nil {
		return err
	}
	defer source.Close()
	temporary := filepath.Join(filepath.Dir(to), "."+filepath.Base(to)+".new")
	target, err := os.OpenFile(temporary, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o755)
	if err != nil {
		return err
	}
	if _, err := io.Copy(target, source); err != nil {
		target.Close()
		os.Remove(temporary)
		return err
	}
	if err := target.Close(); err != nil {
		os.Remove(temporary)
		return err
	}
	if err := os.Rename(temporary, to); err != nil {
		os.Remove(temporary)
		return err
	}
	return nil
}

// busy reports whether taking up a new build now would cut something short:
// an agent at work, or messages that wait in a queue, which the server
// keeps in memory.
func (s *server) busy() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.active) > 0 {
		return true
	}
	for _, q := range s.queues {
		if q != nil && len(q.Items) > 0 {
			return true
		}
	}
	return false
}

// takeUp runs the new build in place of this one, once nothing would be
// cut short. A change of the code while it waits is built first.
func (s *server) takeUp(ctx context.Context, listener net.Listener, fingerprint string) {
	r := s.rebuild
	for s.busy() {
		if r.current().State != "waiting" {
			s.setBuild(buildStatus{State: "waiting", Message: "built; the server takes the new build up once the agents at work finish"})
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(time.Second):
		}
		if goFingerprint(r.root) != fingerprint {
			return // the code changed again: followGo builds it first
		}
	}
	s.setBuild(buildStatus{State: "restarting", Message: "restarting with the new build"})
	time.Sleep(250 * time.Millisecond) // the pages hear it
	if err := execSelf(r.exe, listener); err != nil {
		s.setBuild(buildStatus{State: "failed", Message: "built, but the server cannot restart itself (" + err.Error() + "); restart it to take the new build up"})
	}
}

// inheritedListener is the listening socket a build before this one handed
// over, if any.
func inheritedListener() (net.Listener, error) {
	value := os.Getenv(listenerEnvironment)
	if value == "" {
		return nil, nil
	}
	os.Unsetenv(listenerEnvironment)
	var fd uintptr
	if _, err := fmt.Sscan(value, &fd); err != nil {
		return nil, fmt.Errorf("%s=%q: %w", listenerEnvironment, value, err)
	}
	file := os.NewFile(fd, "listener")
	if file == nil {
		return nil, errors.New("the inherited socket is gone")
	}
	defer file.Close()
	return net.FileListener(file)
}
