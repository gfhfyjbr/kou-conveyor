package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/v2"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func TestMain(m *testing.M) {
	cockpittest.Main()
	os.Exit(m.Run())
}

type harness struct {
	*testing.T
	server *server
	http   *httptest.Server
	cancel context.CancelFunc
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	return newHarnessWith(t, nil)
}

// newHarnessWith starts a server with options that configure changes.
func newHarnessWith(t *testing.T, configure func(*options)) *harness {
	t.Helper()
	workspace := t.TempDir()
	ctx, cancel := context.WithCancel(t.Context())
	o := options{thinking: "high"}
	o.Runner, o.Workspace, o.SessionDir = cockpittest.Runner(t), workspace, workspace+"/sessions"
	o.SettingsFile = t.TempDir() + "/settings.json"
	if configure != nil {
		configure(&o)
	}
	s := newServer(ctx, o, &net.TCPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 8080})
	h := &harness{T: t, server: s, http: httptest.NewServer(s.handler()), cancel: cancel}
	t.Cleanup(func() {
		cancel()
		h.http.Close()
		shutdown, stop := context.WithTimeout(context.Background(), 10*time.Second)
		defer stop()
		if !s.close(shutdown) {
			t.Error("runs did not stop")
		}
	})
	return h
}

func (h *harness) do(method, path, body string, headers ...string) (*http.Response, map[string]any) {
	h.Helper()
	req, err := http.NewRequest(method, h.http.URL+path, strings.NewReader(body))
	if err != nil {
		h.Fatal(err)
	}
	if body != "" {
		req.Header.Set("Content-Type", "application/json")
	}
	for i := 0; i+1 < len(headers); i += 2 {
		req.Header.Set(headers[i], headers[i+1])
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	var decoded map[string]any
	if strings.HasPrefix(res.Header.Get("Content-Type"), "application/json") {
		var raw any
		if err := json.UnmarshalRead(res.Body, &raw); err != nil {
			h.Fatal(err)
		}
		decoded, _ = raw.(map[string]any)
	}
	return res, decoded
}

func (h *harness) start(body string) map[string]any {
	h.Helper()
	res, decoded := h.do("POST", "/api/runs", body)
	if res.StatusCode != http.StatusAccepted {
		h.Fatalf("start: %d %v", res.StatusCode, decoded)
	}
	return decoded
}

type streamed struct {
	ids    []string
	events []event
}

// stream reads a run's events until the server ends the stream.
func (h *harness) stream(runID string, lastEventID string) (streamed, int) {
	h.Helper()
	req, _ := http.NewRequest("GET", h.http.URL+"/api/runs/"+runID+"/events", nil)
	if lastEventID != "" {
		req.Header.Set("Last-Event-ID", lastEventID)
	}
	ctx, cancel := context.WithTimeout(h.Context(), 10*time.Second)
	defer cancel()
	res, err := http.DefaultClient.Do(req.WithContext(ctx))
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	var out streamed
	scanner := bufio.NewScanner(res.Body)
	scanner.Buffer(nil, 1<<20)
	for scanner.Scan() {
		line := scanner.Text()
		switch {
		case strings.HasPrefix(line, "id: "):
			out.ids = append(out.ids, strings.TrimPrefix(line, "id: "))
		case strings.HasPrefix(line, "data: "):
			var e event
			if err := json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &e); err != nil {
				h.Fatal(err)
			}
			out.events = append(out.events, e)
		}
	}
	return out, res.StatusCode
}

func (s streamed) entries() map[string]*cockpit.Entry {
	latest := make(map[string]*cockpit.Entry)
	for _, e := range s.events {
		if e.Type == "entry" {
			latest[e.Entry.ID] = e.Entry
		}
	}
	return latest
}

func (s streamed) last() event { return s.events[len(s.events)-1] }

func TestRunStreamsTranscriptAndPersistsSession(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"use a tool","message_id":"1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b"}`)
	sessionID := started["session_id"].(string)

	got, status := h.stream(started["run_id"].(string), "")
	if status != http.StatusOK || got.last().Type != "done" || got.last().Error != "" {
		t.Fatalf("status %d, events %#v", status, got.events)
	}
	entries := got.entries()
	prompt := entries["input:1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b"]
	call := entries["tool:call-1"]
	if prompt == nil || prompt.State != "" || call == nil || call.Tool.State != cockpit.ToolDone || call.Tool.Output != "hi\n" {
		t.Fatalf("entries = %#v", entries)
	}

	_, session := h.do("GET", "/api/sessions/"+sessionID, "")
	if session["title"] != "use a tool" || len(session["entries"].([]any)) != 3 || session["run"] != nil {
		t.Fatalf("session = %#v", session)
	}
	// The browser compares sizes to notice writers other than its own run.
	if size, _ := session["size"].(float64); size == 0 || int64(size) != got.last().Size {
		t.Fatalf("session size %v, done size %d", session["size"], got.last().Size)
	}
	res, _ := h.do("GET", "/api/sessions/missing-session", "")
	if res.StatusCode != http.StatusNotFound {
		t.Fatalf("missing session: %d", res.StatusCode)
	}
}

func TestBusySessionConflictsUntilRunEnds(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	runID := started["run_id"].(string)

	res, conflict := h.do("POST", "/api/runs", `{"prompt":"again","session_id":"session-1"}`)
	if res.StatusCode != http.StatusConflict || conflict["run_id"] != runID {
		t.Fatalf("second run: %d %v", res.StatusCode, conflict)
	}
	req, _ := http.NewRequest("GET", h.http.URL+"/api/sessions", nil)
	listed, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var sessions []sessionSummary
	json.UnmarshalRead(listed.Body, &sessions)
	listed.Body.Close()
	if len(sessions) != 1 || sessions[0].RunID != runID {
		t.Fatalf("sessions = %#v", sessions)
	}

	if res, _ := h.do("POST", "/api/runs/"+runID+"/cancel", ""); res.StatusCode != http.StatusAccepted {
		t.Fatalf("cancel: %d", res.StatusCode)
	}
	got, _ := h.stream(runID, "")
	if done := got.last(); done.Type != "done" || !done.Stopped {
		t.Fatalf("events = %#v", got.events)
	}
	// Browsers hear "done" only once the session can take the next prompt.
	h.start(`{"prompt":"again","session_id":"session-1"}`)
}

func TestEventsResumeAfterLastEventID(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"hello"}`)
	runID := started["run_id"].(string)
	all, _ := h.stream(runID, "")
	if len(all.ids) < 3 {
		t.Fatalf("events = %#v", all.events)
	}

	tail, status := h.stream(runID, all.ids[len(all.ids)-3])
	if status != http.StatusOK || len(tail.events) != 2 || tail.ids[0] != all.ids[len(all.ids)-2] {
		t.Fatalf("resumed stream: %d %#v", status, tail.ids)
	}
	if _, status := h.stream(runID, all.ids[len(all.ids)-1]); status != http.StatusNoContent {
		t.Fatalf("drained stream status = %d", status)
	}
	if res, _ := h.do("GET", "/api/runs/unknown/events", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("unknown run: %d", res.StatusCode)
	}
}

func TestFailedRunReportsRunnerError(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"please fail"}`)
	got, _ := h.stream(started["run_id"].(string), "")
	var errors, logs int
	for _, e := range got.events {
		if e.Type == "entry" && e.Entry.Kind == cockpit.KindError {
			errors++
		}
		if e.Type == "log" {
			logs++
		}
	}
	if errors != 1 || logs != 1 || got.last().Error == "" {
		t.Fatalf("events = %#v", got.events)
	}
}

func TestRejectsForeignRequests(t *testing.T) {
	h := newHarness(t)
	for name, test := range map[string]struct {
		method, path, body string
		headers            []string
		want               int
	}{
		"cross-site post":   {"POST", "/api/runs", `{"prompt":"x"}`, []string{"Sec-Fetch-Site", "cross-site"}, http.StatusForbidden},
		"rebinding host":    {"GET", "/api/sessions", "", []string{"Host", "attacker.example:8080"}, http.StatusForbidden},
		"form post":         {"POST", "/api/runs", "", []string{"Content-Type", "text/plain"}, http.StatusUnsupportedMediaType},
		"bad session":       {"POST", "/api/runs", `{"prompt":"x","session_id":"../etc"}`, nil, http.StatusBadRequest},
		"bad thinking":      {"POST", "/api/runs", `{"prompt":"x","thinking_level":"huge"}`, nil, http.StatusBadRequest},
		"empty prompt":      {"POST", "/api/runs", `{"prompt":"  "}`, nil, http.StatusBadRequest},
		"invisible prompt":  {"POST", "/api/runs", `{"prompt":"\u001b[0m\u0007"}`, nil, http.StatusBadRequest},
		"unknown api route": {"GET", "/api/nope", "", nil, http.StatusNotFound},
	} {
		t.Run(name, func(t *testing.T) {
			req, _ := http.NewRequest(test.method, h.http.URL+test.path, bytes.NewBufferString(test.body))
			if test.body != "" {
				req.Header.Set("Content-Type", "application/json")
			}
			for i := 0; i+1 < len(test.headers); i += 2 {
				if test.headers[i] == "Host" {
					req.Host = test.headers[i+1]
				} else {
					req.Header.Set(test.headers[i], test.headers[i+1])
				}
			}
			res, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			res.Body.Close()
			if res.StatusCode != test.want {
				t.Fatalf("status = %d, want %d", res.StatusCode, test.want)
			}
		})
	}
}

func TestShutdownStopsRuns(t *testing.T) {
	h := newHarness(t)
	h.start(`{"prompt":"wait forever"}`)
	h.cancel()
	shutdown, stop := context.WithTimeout(t.Context(), 10*time.Second)
	defer stop()
	if !h.server.close(shutdown) {
		t.Fatal("runner survived shutdown")
	}
	res, _ := h.do("POST", "/api/runs", `{"prompt":"late"}`)
	if res.StatusCode != http.StatusServiceUnavailable {
		t.Fatalf("start after shutdown: %d", res.StatusCode)
	}
}

func TestServesCockpit(t *testing.T) {
	h := newHarness(t)
	for path, want := range map[string]string{
		"/": "text/html", "/kernel/host.js": "text/javascript", "/kernel/dom.js": "text/javascript", "/kernel/kernel.css": "text/css",
		"/theme.js": "text/javascript", "/favicon.svg": "image/svg+xml",
	} {
		res, _ := h.do("GET", path, "")
		if res.StatusCode != http.StatusOK || !strings.HasPrefix(res.Header.Get("Content-Type"), want) {
			t.Errorf("%s: %d %s", path, res.StatusCode, res.Header.Get("Content-Type"))
		}
		if res.Header.Get("Content-Security-Policy") == "" {
			t.Errorf("%s has no CSP", path)
		}
	}
}

func TestSessionRunElsewhereIsReported(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"hello","session_id":"session-1"}`)
	h.stream(started["run_id"].(string), "")

	// A terminal takes the session.
	unlock, err := cockpit.LockSession(h.server.opt.SessionDir, "session-1")
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	if _, session := h.do("GET", "/api/sessions/session-1", ""); session["external"] != true || session["run"] != nil {
		t.Fatalf("session = %#v", session)
	}
	res, body := h.do("POST", "/api/runs", `{"prompt":"again","session_id":"session-1"}`)
	if res.StatusCode != http.StatusConflict || !strings.Contains(body["error"].(string), "another window") {
		t.Fatalf("start: %d %v", res.StatusCode, body)
	}
	unlock()
	if _, session := h.do("GET", "/api/sessions/session-1", ""); session["external"] != false {
		t.Fatalf("session = %#v", session)
	}
}
