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

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/fswatch"
	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

// A web cockpit built from a checkout follows its Go code the way it follows
// its plugins. When a .go file of the checkout changes — or go.mod, go.sum,
// or a file of the harness's built-in plugins compiled into the programs,
// such as the guide's skill — it builds itself anew, with the runner and the terminal cockpit that sit
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

// compiledPlugins holds the harness's built-in plugins that are compiled
// into the programs as files (harness/plugin/builtin.go embeds them).
const compiledPlugins = "harness/plugin/builtin/"

// How long the code must stay as it is before a build starts: an editor
// saves several files one after another.
const rebuildSettle = 900 * time.Millisecond

// buildStatus is what the pages are told of the server's own builds.
type buildStatus struct {
	State   string    `json:"state"` // idle, building, failed, waiting, restarting
	Message string    `json:"message,omitzero"`
	Output  string    `json:"output,omitzero"` // the compiler's, when a build failed
	At      time.Time `json:"at"`
	// Going (restarting) is how many runs at work go on, their runners
	// running apart from the server, and Outdated how many of them with the
	// runner build they started with (detached.go).
	Going    int `json:"going,omitzero"`
	Outdated int `json:"outdated,omitzero"`
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
var programs = []string{"kou-conveyor-runner", "kou-conveyor-web", "kou-conveyor-tui", "kou-conveyor-canvas"}

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
// cmd/, harness/ and internal/ but tests, go.mod and go.sum, and every file
// of the harness's built-in plugins compiled in (compiledPlugins).
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
			relative, _ := filepath.Rel(root, path)
			compiledIn := strings.HasPrefix(filepath.ToSlash(relative), compiledPlugins)
			if !compiledIn && (!strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go")) {
				return nil
			}
			info, err := entry.Info()
			if err != nil {
				return nil
			}
			fmt.Fprintf(digest, "%s %d %d\n", filepath.ToSlash(relative), info.Size(), info.ModTime().UnixNano())
			return nil
		})
	}
	return hex.EncodeToString(digest.Sum(nil))[:20]
}

// goSources are the files goFingerprint looks at, as the system can tell
// of their changes.
func goSources(root string) []string {
	return []string{
		filepath.Join(root, "go.mod"), filepath.Join(root, "go.sum"),
		filepath.Join(root, "cmd"), filepath.Join(root, "harness"), filepath.Join(root, "internal"),
	}
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

// goPollInterval is how often the Go code is looked at while a change
// settles, and where the system does not tell of changes; goSafetyPoll how
// often where it does, besides when it tells, for what it might not tell of.
const (
	goPollInterval = 700 * time.Millisecond
	goSafetyPoll   = 10 * time.Second
)

// followGo builds the server anew whenever its Go code changes, and takes
// up each build that succeeds, until ctx ends. The code is looked at when
// the system tells of a change to it, at most every 700 ms, and every
// 700 ms while a change settles; where the system tells of none, every
// 700 ms.
func (s *server) followGo(ctx context.Context, listener net.Listener) {
	r := s.rebuild
	built := sourceFingerprint
	if built == "" {
		built = goFingerprint(r.root)
	}
	var changes <-chan struct{}
	if files, err := fswatch.Watch(goSources(r.root)...); err == nil {
		defer files.Close()
		changes = files.Changes()
	}
	seen, since := "", time.Time{}
	ticker := time.NewTicker(goPollInterval)
	defer ticker.Stop()
	safety := time.NewTicker(goSafetyPoll)
	defer safety.Stop()
	if changes == nil {
		safety.Stop() // the ticker looks
	}
	looked := time.Now()
	for {
		tick := ticker.C
		if changes != nil && seen == "" {
			tick = nil // nothing settles: the system tells of a change
		}
		select {
		case <-ctx.Done():
			return
		case <-tick:
		case <-changes:
			if !pace(ctx, looked, goPollInterval) {
				return
			}
		case <-safety.C:
		}
		looked = time.Now()
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
		if again := s.takeUp(ctx, listener, now, changes); again != "" {
			// The code changed while the build waited: once it settles, it
			// is built, as any change.
			seen, since, looked = again, time.Now(), time.Now()
		}
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
		command := spawn.CommandContext(ctx, gobin, "build", "-trimpath", "-ldflags", flags, "-o", filepath.Join(temporary, name), "./cmd/"+name)
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

// busy reports whether taking up a new build now would cut something short.
func (s *server) busy() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.busyLocked()
}

// busyLocked is busy; the caller holds s.mu. A run whose runner runs attached
// to the server would be cut short, and so would one that starts or ends,
// or is stopped to start again, which the server handles. A runner that runs
// apart from the server goes on, and the new build takes its run up; what
// waits in the queues goes over with it (detached.go).
func (s *server) busyLocked() bool {
	for _, current := range s.active {
		if current.job == nil || current.dir == "" || current.restarting() {
			return true
		}
		select {
		case <-current.job.Done():
			return true
		default:
		}
	}
	return false
}

// takeUp runs the new build in place of this one, once nothing would be
// cut short. A change of the code while it waits is built first: takeUp
// returns the code's new fingerprint, for followGo to build. While it waits,
// it asks every second whether anything would still be cut short, and looks
// at the code as followGo does: when changes, the system's, tell of a
// change, and every goSafetyPoll besides — every second where the system
// tells of none. A walk of the checkout costs more than the question.
func (s *server) takeUp(ctx context.Context, listener net.Listener, fingerprint string, changes <-chan struct{}) string {
	r := s.rebuild
	poll := goSafetyPoll
	if changes == nil {
		poll = time.Second
	}
	looked := time.Now()
	for !s.quiesce() {
		if r.current().State != "waiting" {
			s.setBuild(buildStatus{State: "waiting", Message: "built; the server takes the new build up once the agents it cannot hand over finish"})
		}
		look := false
		select {
		case <-ctx.Done():
			return ""
		case <-time.After(time.Second):
			look = time.Since(looked) >= poll
		case <-changes:
			if !pace(ctx, looked, goPollInterval) {
				return ""
			}
			look = true
		}
		if !look {
			continue
		}
		looked = time.Now()
		if now := goFingerprint(r.root); now != fingerprint {
			return now // the code changed again: followGo builds it first
		}
	}
	// The server starts no runs any more, and takes no prompts: the runs at
	// work go over to the new build, and what waits in the queues (below).
	going, outdated := s.goingOn()
	message := "restarting with the new build"
	if words := goingOnWords(going, outdated); words != "" {
		message += "; " + words
	}
	s.setBuild(buildStatus{State: "restarting", Message: message, Going: going, Outdated: outdated})
	time.Sleep(250 * time.Millisecond) // the pages hear it
	// What the canvases have yet to write is written; the new build reads
	// it, and takes their shells up with the terminals'.
	s.canvas.Flush()
	// The terminals' shells go on in the new build.
	var env []string
	handover, err := s.terminals.Handover()
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: the terminals are not handed over:", err)
	}
	if handover != "" {
		env = append(env, terminalsEnvironment+"="+handover)
	}
	if err := s.saveQueues(); err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: the queues are not handed over:", err)
	}
	if err := execSelf(r.exe, listener, env...); err != nil {
		s.terminals.Resume(handover)
		s.reopen()
		s.setBuild(buildStatus{State: "failed", Message: "built, but the server cannot restart itself (" + err.Error() + "); restart it to take the new build up"})
	}
	return ""
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
