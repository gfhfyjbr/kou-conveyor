package main

import (
	"context"
	"crypto/sha256"
	"encoding/json/v2"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Open pages follow the plugins as they change. /api/w/{ws}/plugins/events
// streams the workspace's plugin listing: at once, and again whenever it
// changes — a plugin's file is written, one is added or taken away, the
// workspace is trusted, a plugin turned off. The page loads what changed
// and nothing else, without being loaded again itself. When the page's own
// files change (assets read from a checkout), a "kernel" event has it load
// again.

// pluginPollInterval is how often the plugins' files are looked at.
const pluginPollInterval = 400 * time.Millisecond

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
	mu       sync.Mutex
	streams  map[*pluginStream]struct{}
	running  bool
	interval time.Duration
	wake     chan struct{}
}

func newPluginWatch() *pluginWatch {
	return &pluginWatch{streams: map[*pluginStream]struct{}{}, interval: pluginPollInterval, wake: make(chan struct{}, 1)}
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
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		case <-s.watch.wake:
		}
		streams := s.watch.snapshot()
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
				fingerprint = plugin.Fingerprint(s.pluginSources(stream.ws)...)
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
	stream.watched = plugin.Fingerprint(s.pluginSources(ws)...)
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
