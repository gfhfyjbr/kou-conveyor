package main

import (
	"bufio"
	"bytes"
	"encoding/json/v2"
	"errors"
	"net/http"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

// socketStream is what a page reads of a stream over a socket: its events,
// and how the socket closed.
type socketStream struct {
	streamed
	kinds []string // each data's event type ("" for a message)
	code  int      // the close code; 1006 when the connection just broke
}

// dialEvents opens a stream over a socket, as kernel/events.js does.
func (h *harness) dialEvents(path string, header http.Header) (*websocket.Conn, *http.Response, error) {
	h.Helper()
	return websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(h.http.URL, "http")+path, header)
}

// readSocket reads a stream over a socket until it closes, or until it has
// want data lines (0: until it closes).
func (h *harness) readSocket(conn *websocket.Conn, want int) socketStream {
	h.Helper()
	var all bytes.Buffer
	var out socketStream
	conn.SetReadDeadline(time.Now().Add(10 * time.Second))
	for {
		kind, data, err := conn.ReadMessage()
		if err != nil {
			var closed *websocket.CloseError
			if !errors.As(err, &closed) {
				h.Fatalf("the socket: %v", err)
			}
			out.code = closed.Code
			break
		}
		if kind != websocket.BinaryMessage {
			h.Fatalf("a message of kind %d", kind)
		}
		all.Write(data)
		// Every stream starts with its retry: a data line follows a line.
		if want > 0 && strings.Count(all.String(), "\ndata: ") >= want {
			break
		}
	}
	scanner := bufio.NewScanner(&all)
	scanner.Buffer(nil, 8<<20)
	kind := ""
	for scanner.Scan() {
		line := scanner.Text()
		switch {
		case strings.HasPrefix(line, "event: "):
			kind = strings.TrimPrefix(line, "event: ")
		case strings.HasPrefix(line, "id: "):
			out.ids = append(out.ids, strings.TrimPrefix(line, "id: "))
		case strings.HasPrefix(line, "data: "):
			out.kinds = append(out.kinds, kind)
			kind = ""
			var e event
			if json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &e) == nil {
				out.events = append(out.events, e)
			}
		}
	}
	return out
}

// A run's events read over a socket are those of its stream, and resume from
// the last the page had; once there are no more, the socket says so, as it
// says that there is no such run.
func TestEventSocketRunEvents(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"use a tool","message_id":"5c0b9a3e-8d1f-4a6b-9e2d-3f4a5b6c7d8e"}`)
	run := started["run_id"].(string)
	want, status := h.stream(run, "")
	if status != http.StatusOK || len(want.ids) < 3 {
		t.Fatalf("the stream: %d %v", status, want.ids)
	}

	conn, _, err := h.dialEvents("/api/runs/"+run+"/events", nil)
	if err != nil {
		t.Fatal(err)
	}
	got := h.readSocket(conn, 0)
	conn.Close()
	if got.code != websocket.CloseNormalClosure {
		t.Fatalf("the socket closed with %d, not as a stream ends", got.code)
	}
	if strings.Join(got.ids, ",") != strings.Join(want.ids, ",") || len(got.events) != len(want.events) || got.last().Type != "done" {
		t.Fatalf("over the socket: %v, over the stream: %v", got.ids, want.ids)
	}

	// From the last event the page had: the rest.
	conn, _, err = h.dialEvents("/api/runs/"+run+"/events?last_event_id="+want.ids[1], nil)
	if err != nil {
		t.Fatal(err)
	}
	rest := h.readSocket(conn, 0)
	conn.Close()
	if strings.Join(rest.ids, ",") != strings.Join(want.ids[2:], ",") {
		t.Fatalf("resumed: %v, want %v", rest.ids, want.ids[2:])
	}
	// An EventSource that starts where a socket left off says so the same way.
	res, err := http.Get(h.http.URL + "/api/runs/" + run + "/events?last_event_id=" + want.ids[len(want.ids)-1])
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	if res.StatusCode != http.StatusNoContent {
		t.Fatalf("a stream from its last event: %d", res.StatusCode)
	}

	for path, code := range map[string]int{
		"/api/runs/" + run + "/events?last_event_id=" + want.ids[len(want.ids)-1]: 4204,
		"/api/runs/no-such-run/events":                                            4404,
		eventSocketProbe:                                                          4204,
	} {
		conn, _, err := h.dialEvents(path, nil)
		if err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		got := h.readSocket(conn, 0)
		conn.Close()
		if got.code != code || len(got.ids) > 0 {
			t.Errorf("%s: closed with %d after %v, want %d", path, got.code, got.ids, code)
		}
	}
	// Asked as any GET, the probe has nothing to say, and says nothing.
	if res, _ := h.do("GET", eventSocketProbe, ""); res.StatusCode != http.StatusNoContent {
		t.Errorf("the probe: %d", res.StatusCode)
	}
}

// The plugins' stream over a socket: the listing first, as over HTTP; a
// page of another site is not let in; a restart breaks the socket.
func TestEventSocketPluginEvents(t *testing.T) {
	h := newHarness(t)
	if _, res, err := h.dialEvents("/api/plugins/events", http.Header{"Origin": {"http://evil.example"}}); err == nil || res == nil || res.StatusCode != http.StatusForbidden {
		t.Fatalf("a page of another site: %v %v", err, res)
	}
	conn, _, err := h.dialEvents("/api/plugins/events", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	got := h.readSocket(conn, 1)
	if len(got.kinds) == 0 || got.kinds[0] != "plugins" {
		t.Fatalf("the first event: %v", got.kinds)
	}
	h.server.sockets.drop()
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	for {
		if _, _, err := conn.ReadMessage(); err != nil {
			if !websocket.IsCloseError(err, websocket.CloseAbnormalClosure) {
				t.Fatalf("dropped: %v, not as a connection breaks", err)
			}
			break
		}
	}
	h.server.sockets.mu.Lock()
	left := len(h.server.sockets.conns)
	h.server.sockets.mu.Unlock()
	for deadline := time.Now().Add(5 * time.Second); left > 0 && time.Now().Before(deadline); {
		time.Sleep(20 * time.Millisecond)
		h.server.sockets.mu.Lock()
		left = len(h.server.sockets.conns)
		h.server.sockets.mu.Unlock()
	}
	if left > 0 {
		t.Fatalf("%d sockets left after the drop", left)
	}
}

// A page reads a stream over a socket as an EventSource reads it
// (kernel/events.js): in pieces cut anywhere, its lines ended with LF, CR
// LF or CR, its comments (the pings) and blank lines no events.
func TestEventSocketParser(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("no node")
	}
	file, err := filepath.Abs("static/kernel/events.js")
	if err != nil {
		t.Fatal(err)
	}
	script := `
import assert from 'node:assert/strict';
const events = await import(process.argv[1]);

const read = (pieces, lastEventId = '') => {
  const got = [];
  const retries = [];
  const parser = events.createParser({ lastEventId, onEvent: (e) => got.push(e), onRetry: (ms) => retries.push(ms) });
  for (const piece of pieces) parser.push(piece);
  return { got, retries };
};
const shown = ({ got }) => got.filter((e) => e.data !== null).map((e) => [e.type, e.data, e.lastEventId]);

const text = 'retry: 1500\n\nid: 1\ndata: {"a":1}\n\n: ping\n\nevent: plugins\ndata: x\ndata: y\n\nid: 2\n\n';
const whole = read([text]);
assert.deepEqual(whole.retries, [1500]);
assert.deepEqual(shown(whole), [['message', '{"a":1}', '1'], ['plugins', 'x\ny', '1']]);
assert.equal(whole.got.at(-1).lastEventId, '2', 'an ID alone counts');
for (let i = 1; i < text.length; i++) assert.deepEqual(read([text.slice(0, i), text.slice(i)]), whole, 'cut at ' + i);
assert.deepEqual(read([...text]), whole, 'a character at a time');
const crlf = text.replaceAll('\n', '\r\n');
for (let i = 1; i < crlf.length; i++) assert.deepEqual(read([crlf.slice(0, i), crlf.slice(i)]), whole, 'CR LF cut at ' + i);
assert.deepEqual(read([...crlf]), whole);
assert.deepEqual(read([text.replaceAll('\n', '\r')]), whole);

assert.equal(read(['id: a\0b\ndata: x\n\n'], '7').got[0].lastEventId, '7', 'an ID with NUL is none');
assert.equal(read(['data: x\n\n'], '7').got[0].lastEventId, '7', 'the last ID holds');
assert.equal(read(['data:x\ndata\nfoo: bar\n\n']).got[0].data, 'x\n');
assert.equal(read(['data: \n\n']).got[0].data, '', 'an empty data line is an event');
assert.equal(read(['data: x\n']).got.length, 0, 'an event ends with a blank line');
assert.deepEqual(read(['retry: soon\n\n']).retries, []);
console.log('ok');
`
	cmd := exec.Command(node, "--input-type=module", "-e", script, "file://"+file)
	out, err := cmd.CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "ok" {
		t.Fatalf("node: %v\n%s", err, out)
	}
}
