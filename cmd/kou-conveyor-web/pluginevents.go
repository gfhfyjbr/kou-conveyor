package main

import (
	"context"
	"crypto/sha256"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/fswatch"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
)

// Open pages follow the plugins as they change. /api/w/{ws}/plugins/events
// streams the workspace's plugin listing: at once, and again whenever it
// changes — a plugin's file is written, one is added or taken away, the
// workspace is trusted, a plugin turned off, a skill added, removed or
// edited. The page loads what changed and nothing else, without being
// loaded again itself. When the page's own files change (assets read from a
// checkout), a "kernel" event has it load again.

// pluginPollInterval is how often the plugins' files are looked at where
// the system does not tell of their changes; pluginSafetyPoll how often
// where it does, besides when it tells, for what it might not tell of.
const (
	pluginPollInterval = 400 * time.Millisecond
	pluginSafetyPoll   = 5 * time.Second
)

type pluginEvent struct {
	kind string
	data []byte
}

// pluginStream is a page that follows a workspace's plugins.
type pluginStream struct {
	ws   *workspace
	send chan pluginEvent
	// sent is the fingerprint of the listing the page has; watched that of
	// the workspace's plugin files when it was taken.
	sent, watched string
}

// offer queues an event for the page; a newer listing replaces one still
// waiting, as it says all the page needs.
func (stream *pluginStream) offer(event pluginEvent) {
	for {
		select {
		case stream.send <- event:
			return
		default:
		}
		select {
		case <-stream.send:
		default:
		}
	}
}

// pluginWatch keeps the pages that follow plugins, and the goroutine that
// looks at the plugins' files while there are any.
type pluginWatch struct {
	mu      sync.Mutex
	streams map[*pluginStream]struct{}
	running bool
	// interval and safety are pluginPollInterval and pluginSafetyPoll,
	// but in tests.
	interval, safety time.Duration
	wake             chan struct{}
}

func newPluginWatch() *pluginWatch {
	return &pluginWatch{streams: map[*pluginStream]struct{}{}, interval: pluginPollInterval, safety: pluginSafetyPoll, wake: make(chan struct{}, 1)}
}

// poke has the watch look at once, after the server changed plugins.json.
func (watch *pluginWatch) poke() {
	select {
	case watch.wake <- struct{}{}:
	default:
	}
}

func (watch *pluginWatch) snapshot() []*pluginStream {
	watch.mu.Lock()
	defer watch.mu.Unlock()
	list := make([]*pluginStream, 0, len(watch.streams))
	for stream := range watch.streams {
		list = append(list, stream)
	}
	return list
}

// follow adds a page, starting the watch if it is the first.
func (s *server) followPlugins(stream *pluginStream) {
	watch := s.watch
	watch.mu.Lock()
	defer watch.mu.Unlock()
	watch.streams[stream] = struct{}{}
	if !watch.running {
		watch.running = true
		go s.watchPlugins(s.ctx)
	}
}

func (s *server) unfollowPlugins(stream *pluginStream) {
	s.watch.mu.Lock()
	defer s.watch.mu.Unlock()
	delete(s.watch.streams, stream)
}

// watchPlugins looks at the plugins of the workspaces pages follow, and at
// the page's own files when they are read from disk, until ctx ends.
func (s *server) watchPlugins(ctx context.Context) {
	ticker := time.NewTicker(s.watch.interval)
	defer ticker.Stop()
	kernel := ""
	if dir := s.assets.staticDirectory(); dir != "" {
		kernel = plugin.Fingerprint(dir)
	}
	var files followedFiles
	defer files.close()
	looked := time.Now()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		case <-s.watch.wake:
		case <-files.changes():
			if !pace(ctx, looked, s.watch.interval) {
				return
			}
		}
		looked = time.Now()
		streams := s.watch.snapshot()
		// The system tells of changes to the files the listings are made
		// from, where it can: they are looked at then, and seldom besides.
		// They are followed before they are looked at, so that a change
		// between is told.
		if files.follow(func() []string { return s.pluginFiles(streams) }) {
			ticker.Reset(s.watch.safety)
		} else {
			ticker.Reset(s.watch.interval)
		}
		if len(streams) == 0 {
			continue
		}
		if dir := s.assets.staticDirectory(); dir != "" {
			if now := plugin.Fingerprint(dir); now != kernel {
				kernel = now
				for _, stream := range streams {
					stream.offer(pluginEvent{kind: "kernel", data: []byte("{}")})
				}
			}
		}
		// Each workspace's files are looked at once, however many pages
		// follow it; its listing is made again only when they changed.
		watched := map[*workspace]string{}
		listings := map[*workspace][]byte{}
		for _, stream := range streams {
			fingerprint, seen := watched[stream.ws]
			if !seen {
				fingerprint = s.listingFingerprint(stream.ws)
				watched[stream.ws] = fingerprint
			}
			if fingerprint == stream.watched {
				continue
			}
			stream.watched = fingerprint
			encoded, made := listings[stream.ws]
			if !made {
				encoded = s.encodedPlugins(stream.ws)
				listings[stream.ws] = encoded
			}
			if sum := fingerprintOf(encoded); sum != stream.sent {
				stream.sent = sum
				stream.offer(pluginEvent{kind: "plugins", data: encoded})
			}
		}
	}
}

// pluginFiles are the files the listings of the workspaces streams follow
// are made from, as the system can tell of their changes: the plugins'
// sources, the skill directories and the page's own files when they are
// read from disk — with where the links in them lead, which the
// fingerprints follow.
func (s *server) pluginFiles(streams []*pluginStream) []string {
	var roots []string
	if dir := s.assets.staticDirectory(); dir != "" {
		roots = append(roots, dir)
	}
	seen := map[*workspace]bool{}
	for _, stream := range streams {
		if seen[stream.ws] {
			continue
		}
		seen[stream.ws] = true
		roots = append(roots, s.pluginSources(stream.ws)...)
		directories, _ := skill.Directories(cockpit.SkillOptions(s.opt.SettingsFile, stream.ws.Path))
		for _, directory := range directories {
			roots = append(roots, directory.Path)
		}
	}
	for _, root := range slices.Clone(roots) {
		entries, err := os.ReadDir(root)
		if err != nil {
			continue
		}
		for _, entry := range entries {
			if entry.Type()&fs.ModeSymlink == 0 {
				continue
			}
			if target, err := filepath.EvalSymlinks(filepath.Join(root, entry.Name())); err == nil {
				roots = append(roots, target)
			}
		}
	}
	slices.Sort(roots)
	return slices.Compact(roots)
}

// followedFiles are the files a watch has the system tell of the changes
// of, and the watcher that tells.
type followedFiles struct {
	roots   []string
	watcher *fswatch.Watcher
	// unsupported is set once the system turned out to tell of none.
	unsupported bool
}

// changes receives when the followed files change; never, while none are.
func (f *followedFiles) changes() <-chan struct{} {
	if f.watcher == nil {
		return nil
	}
	return f.watcher.Changes()
}

// follow has the system tell of changes to the files roots names from now
// on, unless it does already, and reports whether it does; roots is not
// asked where the system tells of none.
func (f *followedFiles) follow(roots func() []string) bool {
	if f.unsupported {
		return false
	}
	now := roots()
	if slices.Equal(f.roots, now) {
		return f.watcher != nil
	}
	f.close()
	f.roots = now
	if len(now) == 0 {
		return false
	}
	watcher, err := fswatch.Watch(now...)
	if err != nil {
		f.unsupported = errors.Is(err, errors.ErrUnsupported)
		return false
	}
	f.watcher = watcher
	return true
}

func (f *followedFiles) close() {
	if f.watcher != nil {
		f.watcher.Close()
		f.watcher = nil
	}
}

// pace waits, unless ctx ends first, until interval has passed since last:
// a burst of changes the system tells of has the files looked at no more
// often than they would be polled.
func pace(ctx context.Context, last time.Time, interval time.Duration) bool {
	wait := time.Until(last.Add(interval))
	if wait <= 0 {
		return true
	}
	timer := time.NewTimer(wait)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}

func (s *server) encodedPlugins(ws *workspace) []byte {
	encoded, err := json.Marshal(s.pluginsOf(ws), json.Deterministic(true))
	if err != nil {
		return []byte(`{"plugins": [], "errors": ["` + err.Error() + `"]}`)
	}
	return encoded
}

func fingerprintOf(data []byte) string {
	sum := sha256.Sum256(data)
	return fmt.Sprintf("%x", sum[:10])
}

// handlePluginEvents streams the plugin listing of a workspace as it
// changes.
func (s *server) handlePluginEvents(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	h := w.Header()
	h.Set("Content-Type", "text/event-stream; charset=utf-8")
	h.Set("Cache-Control", "no-store")
	h.Set("X-Accel-Buffering", "no")
	w.WriteHeader(http.StatusOK)
	flusher := http.NewResponseController(w)

	stream := &pluginStream{ws: ws, send: make(chan pluginEvent, 1)}
	stream.watched = s.listingFingerprint(ws)
	encoded := s.encodedPlugins(ws)
	stream.sent = fingerprintOf(encoded)
	s.followPlugins(stream)
	defer s.unfollowPlugins(stream)

	fmt.Fprint(w, "retry: 1000\n\n")
	fmt.Fprintf(w, "event: plugins\ndata: %s\n\n", encoded)
	// A page that comes while the server builds itself hears where it is.
	if s.rebuild != nil {
		if status := s.rebuild.current(); status.State != "" && status.State != "idle" {
			if data, err := json.Marshal(status); err == nil {
				fmt.Fprintf(w, "event: server\ndata: %s\n\n", data)
			}
		}
	}
	if err := flusher.Flush(); err != nil {
		return
	}
	heartbeat := time.NewTicker(20 * time.Second)
	defer heartbeat.Stop()
	for {
		select {
		case event := <-stream.send:
			fmt.Fprintf(w, "event: %s\ndata: %s\n\n", event.kind, event.data)
		case <-heartbeat.C:
			fmt.Fprint(w, ": ping\n\n")
		case <-r.Context().Done():
			return
		case <-s.ctx.Done():
			return
		}
		if err := flusher.Flush(); err != nil {
			return
		}
	}
}
