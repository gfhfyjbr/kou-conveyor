package main

import (
	"bytes"
	"context"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// A page follows the server over event streams: the plugins of its
// workspace (pluginevents.go), the run of its session (runs.go), the canvas
// in view (canvas/hub.go). Each is a GET of …/events that stays open, and
// over HTTP/1.1 a browser holds no more than six connections to a host, for
// all its tabs together: a canvas whose kou agents show their chats — each
// a page of the cockpit in a frame, with streams of its own — runs out of
// them, and every other read of its pages waits for one. A WebSocket is not
// one of the six, so a page takes its streams over sockets instead
// (kernel/events.js): a GET of …/events that asks to be upgraded is
// answered by the same handler as ever, and what the handler writes goes,
// flush by flush, as the socket's binary messages — the same server-sent
// events, which the page reads as an EventSource would. The ID of the last
// event the page had comes as last_event_id, where an EventSource that
// reconnects sends Last-Event-ID. A stream that ends closes its socket
// normally, and the page connects again; any other answer closes it with
// 4000 and its status (4204: there is nothing more; 4404: not found), and
// the page gives up on it, as an EventSource does.

// eventSocketProbe is what a page asks first, to know whether its streams
// may come over sockets: a socket opened to it says yes. An older server
// does not upgrade it, and answers 404.
const eventSocketProbe = "/api/sockets/events"

// eventSocketUpgrader takes the pages of the cockpit's own origin only, as
// terminalUpgrader does.
var eventSocketUpgrader = websocket.Upgrader{ReadBufferSize: 1 << 10, WriteBufferSize: 32 << 10}

const (
	// The server pings the page every eventSocketPing, and takes the page
	// for gone when it has not answered for eventSocketIdle.
	eventSocketPing = 30 * time.Second
	eventSocketIdle = 75 * time.Second
	// How long a message may take to go.
	eventSocketWrite = 20 * time.Second
	// What a handler writes goes when it flushes, or once this much waits.
	eventSocketChunk = 256 << 10
)

// eventSockets lets the page read the streams of h over sockets.
func (s *server) eventSockets(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || !strings.HasSuffix(r.URL.Path, "/events") {
			h.ServeHTTP(w, r)
			return
		}
		if last := r.URL.Query().Get("last_event_id"); last != "" && r.Header.Get("Last-Event-ID") == "" {
			r = r.Clone(r.Context())
			r.Header.Set("Last-Event-ID", last)
		}
		if !websocket.IsWebSocketUpgrade(r) {
			h.ServeHTTP(w, r)
			return
		}
		s.serveEventSocket(w, r, h)
	})
}

func (s *server) serveEventSocket(w http.ResponseWriter, r *http.Request, h http.Handler) {
	conn, err := eventSocketUpgrader.Upgrade(w, r, nil)
	if err != nil {
		return // the upgrader answered
	}
	defer conn.Close()
	s.sockets.add(conn)
	defer s.sockets.remove(conn)
	// The connection is the socket's now: the request's context no longer
	// ends with it. The handler's ends when the page goes, or the server.
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	defer context.AfterFunc(s.ctx, cancel)()

	// The page sends nothing; reading takes its pongs and its close.
	conn.SetReadLimit(1 << 10)
	conn.SetReadDeadline(time.Now().Add(eventSocketIdle))
	conn.SetPongHandler(func(string) error { return conn.SetReadDeadline(time.Now().Add(eventSocketIdle)) })
	read := make(chan struct{})
	go func() {
		defer close(read)
		defer cancel()
		for {
			if _, _, err := conn.NextReader(); err != nil {
				return
			}
		}
	}()
	go func() {
		ticker := time.NewTicker(eventSocketPing)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				if err := conn.WriteControl(websocket.PingMessage, nil, time.Now().Add(eventSocketWrite)); err != nil {
					cancel()
					return
				}
			}
		}
	}()

	out := &socketWriter{conn: conn, header: http.Header{}, cancel: cancel}
	h.ServeHTTP(out, r.Clone(ctx))
	code, reason := out.end()
	if conn.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(code, reason), time.Now().Add(time.Second)) == nil {
		// The page answers the close; then the connection goes.
		select {
		case <-read:
		case <-time.After(time.Second):
		}
	}
}

// socketWriter is the response a handler of …/events writes when the page
// reads it over a socket.
type socketWriter struct {
	conn   *websocket.Conn
	header http.Header
	cancel context.CancelFunc
	status int  // 0 until the handler answers
	stream bool // the answer is an event stream: it goes to the page
	buf    bytes.Buffer
	err    error
}

func (w *socketWriter) Header() http.Header { return w.header }

func (w *socketWriter) WriteHeader(status int) {
	if w.status != 0 || status < 200 {
		return
	}
	w.status = status
	w.stream = status == http.StatusOK && strings.HasPrefix(w.header.Get("Content-Type"), "text/event-stream")
}

func (w *socketWriter) Write(p []byte) (int, error) {
	w.WriteHeader(http.StatusOK)
	if w.err != nil {
		return 0, w.err
	}
	if !w.stream {
		return len(p), nil // what is no stream does not go; its status does
	}
	w.buf.Write(p)
	if w.buf.Len() >= eventSocketChunk {
		if err := w.FlushError(); err != nil {
			return 0, err
		}
	}
	return len(p), nil
}

// FlushError sends what was written since the last flush, as one message
// (http.ResponseController's Flush).
func (w *socketWriter) FlushError() error {
	w.WriteHeader(http.StatusOK)
	if w.err != nil || w.buf.Len() == 0 {
		return w.err
	}
	w.conn.SetWriteDeadline(time.Now().Add(eventSocketWrite))
	if err := w.conn.WriteMessage(websocket.BinaryMessage, w.buf.Bytes()); err != nil {
		w.err = err
		w.cancel()
		return err
	}
	w.buf.Reset()
	return nil
}

func (w *socketWriter) Flush() { _ = w.FlushError() }

// end sends what is left, and says how the socket closes: normally after a
// stream, with 4000 and its status after any other answer.
func (w *socketWriter) end() (int, string) {
	w.WriteHeader(http.StatusOK)
	if !w.stream {
		return 4000 + min(w.status, 999), http.StatusText(w.status)
	}
	_ = w.FlushError()
	return websocket.CloseNormalClosure, "the stream ended"
}

// eventSockets are the streams the pages read over sockets now: the
// server's own connections no longer, which it closes as it goes.
type eventSockets struct {
	mu    sync.Mutex
	conns map[*websocket.Conn]bool
}

func (e *eventSockets) add(conn *websocket.Conn) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.conns == nil {
		e.conns = map[*websocket.Conn]bool{}
	}
	e.conns[conn] = true
}

func (e *eventSockets) remove(conn *websocket.Conn) {
	e.mu.Lock()
	defer e.mu.Unlock()
	delete(e.conns, conn)
}

// drop breaks every one as a restart does; the pages connect again.
func (e *eventSockets) drop() {
	e.mu.Lock()
	defer e.mu.Unlock()
	for conn := range e.conns {
		conn.Close()
	}
}
