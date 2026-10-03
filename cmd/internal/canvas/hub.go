package canvas

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"strconv"
	"strings"
	"time"
)

// A canvas's pages follow it as server-sent events. The first is a
// snapshot of the canvas; then come its changes as they happen, each with
// an ID — the engine's run, then a number — so that a page that reconnects
// with Last-Event-ID is given what it missed, or, when that is no longer
// kept or comes from another run of the server, a snapshot again.
//
// Every event is published while the canvas's lock is held, so that a
// snapshot, taken under it, and the number of the last event agree.

// hubKeep is how many events are kept for the pages that reconnect.
const hubKeep = 512

type hubEvent struct {
	seq  int64
	data []byte
}

type hub struct {
	instance string
	seq      int64
	events   []hubEvent
	wake     chan struct{}
	closed   bool
}

func newHub(instance string) *hub {
	return &hub{instance: instance, wake: make(chan struct{})}
}

// publish sends an event to the pages; the caller holds the canvas's lock.
func (h *hub) publish(v any) {
	if h.closed {
		return
	}
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	h.seq++
	h.events = append(h.events, hubEvent{h.seq, data})
	if len(h.events) > hubKeep {
		h.events = append(h.events[:0:0], h.events[len(h.events)-hubKeep:]...)
	}
	close(h.wake)
	h.wake = make(chan struct{})
}

// close ends the pages' streams: the canvas is gone.
func (h *hub) close() {
	if !h.closed {
		h.closed = true
		close(h.wake)
	}
}

// since returns the events after seq; ok is false when some of them are no
// longer kept. The caller holds the canvas's lock.
func (h *hub) since(seq int64) (events []hubEvent, ok bool) {
	if seq > h.seq {
		return nil, false
	}
	if seq == h.seq {
		return nil, true
	}
	if len(h.events) == 0 || h.events[0].seq > seq+1 {
		return nil, false
	}
	start := int(seq + 1 - h.events[0].seq)
	return h.events[start:len(h.events):len(h.events)], true
}

// eventID is how a page names the last event it has.
func (h *hub) eventID(seq int64) string { return h.instance + "-" + strconv.FormatInt(seq, 10) }

// parseEventID reads a page's Last-Event-ID; ok is false for one of
// another run of the server.
func (h *hub) parseEventID(id string) (int64, bool) {
	instance, number, found := strings.Cut(id, "-")
	if !found || instance != h.instance {
		return 0, false
	}
	seq, err := strconv.ParseInt(number, 10, 64)
	return seq, err == nil && seq >= 0
}

// serveEvents streams a canvas's events to a page.
func (c *Canvas) serveEvents(w http.ResponseWriter, r *http.Request) {
	c.mu.Lock()
	var events []hubEvent
	var snapshot []byte
	var seq int64
	resumed := false
	if last := r.Header.Get("Last-Event-ID"); last != "" {
		if from, ok := c.hub.parseEventID(last); ok {
			if events, resumed = c.hub.since(from); resumed {
				seq = from
			}
		}
	}
	if !resumed {
		data, err := json.Marshal(c.snapshotLocked("snapshot"))
		if err != nil {
			c.mu.Unlock()
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
		snapshot, seq = data, c.hub.seq
	}
	wake, closed := c.hub.wake, c.hub.closed
	c.mu.Unlock()
	if closed {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	h := w.Header()
	h.Set("Content-Type", "text/event-stream; charset=utf-8")
	h.Set("Cache-Control", "no-store")
	h.Set("X-Accel-Buffering", "no")
	w.WriteHeader(http.StatusOK)
	flusher := http.NewResponseController(w)
	fmt.Fprint(w, "retry: 1500\n\n")
	if snapshot != nil {
		if _, err := fmt.Fprintf(w, "id: %s\ndata: %s\n\n", c.hub.eventID(seq), snapshot); err != nil {
			return
		}
	}
	heartbeat := time.NewTicker(15 * time.Second)
	defer heartbeat.Stop()
	for {
		for _, event := range events {
			if _, err := fmt.Fprintf(w, "id: %s\ndata: %s\n\n", c.hub.eventID(event.seq), event.data); err != nil {
				return
			}
			seq = event.seq
		}
		events = nil
		if err := flusher.Flush(); err != nil {
			return
		}
		select {
		case <-wake:
		case <-heartbeat.C:
			if _, err := fmt.Fprint(w, ": ping\n\n"); err != nil {
				return
			}
			continue
		case <-r.Context().Done():
			return
		}
		c.mu.Lock()
		var ok bool
		events, ok = c.hub.since(seq)
		wake, closed = c.hub.wake, c.hub.closed
		if !ok && !closed {
			// Too far behind: the page starts over.
			data, err := json.Marshal(c.snapshotLocked("snapshot"))
			if err == nil {
				events, seq = []hubEvent{{c.hub.seq, data}}, c.hub.seq-1
			}
		}
		c.mu.Unlock()
		if closed {
			for _, event := range events {
				fmt.Fprintf(w, "id: %s\ndata: %s\n\n", c.hub.eventID(event.seq), event.data)
			}
			fmt.Fprint(w, "data: {\"type\":\"deleted\"}\n\n")
			_ = flusher.Flush()
			return
		}
	}
}
