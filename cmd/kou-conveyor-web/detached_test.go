//go:build unix

package main

import (
	"bytes"
	"context"
	"errors"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

// apart is a workspace, its sessions and the server's settings, which one
// server whose runners run apart from it shares with the next one: the one
// that takes its runs up.
type apart struct {
	workspace, sessions, settings string
}

func newApart(t *testing.T) *apart {
	t.Helper()
	workspace := t.TempDir()
	a := &apart{workspace: workspace, sessions: filepath.Join(workspace, "sessions"), settings: filepath.Join(t.TempDir(), "settings.json")}
	// The runners that go on would outlive the test. Registered first, this
	// runs after the servers are gone.
	t.Cleanup(func() {
		dirs, _ := filepath.Glob(filepath.Join(a.sessions, runsDir, "*"))
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		for _, dir := range dirs {
			if err := cockpit.StopDetached(ctx, dir, "the test ended"); err != nil && !errors.Is(err, cockpit.ErrNotDetached) {
				t.Errorf("stop %s: %v", dir, err)
			}
		}
	})
	return a
}

// serve starts a server of the workspace whose runners run apart from it.
func (a *apart) serve(t *testing.T, configure ...func(*options)) *harness {
	t.Helper()
	return newHarnessWith(t, func(o *options) {
		o.detach = true
		o.Workspace, o.SessionDir, o.SettingsFile = a.workspace, a.sessions, a.settings
		for _, c := range configure {
			c(o)
		}
	})
}

// openGate lets the runs that hold at the gate go on.
func (a *apart) openGate(t *testing.T) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(a.workspace, "gate"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
}

// goAway shuts the server down as an interrupt does.
func (h *harness) goAway() {
	h.Helper()
	h.cancel()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if !h.server.close(ctx) {
		h.Fatal("the server did not shut down in time")
	}
}

// adoptedRun is the ID of the run the server took up for a session as it
// started, which may have ended since.
func (h *harness) adoptedRun(session string) string {
	h.Helper()
	h.server.mu.Lock()
	defer h.server.mu.Unlock()
	for id, current := range h.server.runs {
		if current.sessionID == session {
			return id
		}
	}
	h.Fatalf("no run of session %s was taken up", session)
	return ""
}

// A run goes on when its server goes, and the server that starts next takes
// it up and settles it.
func TestRunsGoOnWithoutTheServer(t *testing.T) {
	a := newApart(t)
	first := a.serve(t)
	runID := first.start(`{"prompt":"hold at the gate","session_id":"session-1"}`)["run_id"].(string)
	dir := filepath.Join(a.sessions, runsDir, runID)
	if _, err := cockpit.ReadDetachedRun(dir); err != nil {
		t.Fatalf("the runner does not run apart from the server: %v", err)
	}
	if first.server.busy() {
		t.Fatal("a run apart from the server holds a new build back")
	}
	first.goAway()
	if !cockpit.SessionBusy(a.sessions, "session-1") {
		t.Fatal("the runner went with the server")
	}

	second := a.serve(t)
	if second.server.adopted != 1 {
		t.Fatalf("adopted = %d", second.server.adopted)
	}
	adopted := second.adoptedRun("session-1")
	if adopted == runID {
		t.Fatal("the run kept its ID: browsers would read the new log from the old offsets")
	}
	if res, _ := second.do("GET", "/api/runs/"+runID+"/events", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("the run before: %d", res.StatusCode)
	}
	// Taken up, the session's prompt is where it was.
	if res, body := second.do("POST", "/api/runs", `{"prompt":"again","session_id":"session-1"}`); res.StatusCode != http.StatusConflict || body["run_id"] != adopted {
		t.Fatalf("a second run: %d %v", res.StatusCode, body)
	}
	a.openGate(t)
	events, _ := second.stream(adopted, "")
	if done := events.last(); done.Type != "done" || done.Error != "" || done.Stopped {
		t.Fatalf("done = %+v", done)
	}
	if got := entryTexts(events); got != "hold at the gate|echo: hold at the gate" {
		t.Fatalf("entries = %s", got)
	}
	if _, err := os.Stat(dir); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("the run's directory is left: %v", err)
	}
	if cockpit.SessionBusy(a.sessions, "session-1") {
		t.Fatal("the session is busy after the run")
	}
	// The session has the prompt once: the server that took the run up
	// told it from the session as it was before the run.
	_, session := second.do("GET", "/api/sessions/session-1", "")
	if entries := session["entries"].([]any); len(entries) != 2 {
		t.Fatalf("entries = %v", entries)
	}
}

// A run stopped while no server is there ends as stopped once one is: its
// queue waits for the user.
func TestRunsStoppedWhileTheServerIsAway(t *testing.T) {
	a := newApart(t)
	first := a.serve(t)
	first.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	if res, _ := first.do("POST", "/api/sessions/session-1/queue", `{"text":"second"}`); res.StatusCode != http.StatusCreated {
		t.Fatalf("enqueue: %d", res.StatusCode)
	}
	// The agent is at work: it recorded the prompt.
	first.waitFor("session-1", func(body map[string]any) bool { return len(body["entries"].([]any)) > 0 })
	// A second Ctrl+C on the server's terminal, pressed as it shuts down,
	// stops the runs that would go on.
	signals := make(chan os.Signal, 1)
	signals <- os.Interrupt
	if code := shutDown(first.server, &http.Server{}, first.cancel, signals, true); code != 0 {
		t.Fatalf("shutDown = %d", code)
	}
	if cockpit.SessionBusy(a.sessions, "session-1") {
		t.Fatal("the runner goes on")
	}
	session, err := os.ReadFile(cockpit.SessionPath(a.sessions, "session-1"))
	if err != nil || !bytes.Contains(session, []byte("the user stopped the server and its agents")) {
		t.Fatalf("the session does not say why the run stopped: %v\n%s", err, session)
	}

	second := a.serve(t)
	events, _ := second.stream(second.adoptedRun("session-1"), "")
	done := events.last()
	if done.Type != "done" || !done.Stopped || done.Next != nil || eventQueue(done.Queue) != "second (paused)" {
		t.Fatalf("done = %+v, queue %q", done, eventQueue(done.Queue))
	}
}

// Without a second interrupt, or shut down by a service manager, the server
// leaves its runs be.
func TestShutdownLeavesRunsApartBe(t *testing.T) {
	a := newApart(t)
	h := a.serve(t)
	h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	signals := make(chan os.Signal, 1)
	if code := shutDown(h.server, &http.Server{}, h.cancel, signals, false); code != 0 {
		t.Fatalf("shutDown = %d", code)
	}
	if !cockpit.SessionBusy(a.sessions, "session-1") {
		t.Fatal("the runner went with the server")
	}
}

// What waits in a queue goes over to the server that starts next, and runs
// once the run it waits for ends.
func TestQueuesGoOverToTheNextServer(t *testing.T) {
	a := newApart(t)
	first := a.serve(t)
	first.start(`{"prompt":"hold at the gate","session_id":"session-1"}`)
	if res, _ := first.do("POST", "/api/sessions/session-1/queue", `{"text":"second"}`); res.StatusCode != http.StatusCreated {
		t.Fatalf("enqueue: %d", res.StatusCode)
	}
	first.goAway()
	queues := filepath.Join(filepath.Dir(a.settings), "queues.json")
	if _, err := os.Stat(queues); err != nil {
		t.Fatalf("the queues are not kept: %v", err)
	}

	second := a.serve(t)
	if _, err := os.Stat(queues); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("the queues taken up are kept still: %v", err)
	}
	body := second.waitFor("session-1", func(body map[string]any) bool { return body["run"] != nil })
	if got := queueOf(t, body["queue"]); got != "second" {
		t.Fatalf("queue = %s", got)
	}
	a.openGate(t)
	events, _ := second.stream(body["run"].(map[string]any)["id"].(string), "")
	done := events.last()
	if done.Next == nil {
		t.Fatalf("done = %+v", done)
	}
	next, _ := second.stream(done.Next.ID, "")
	if got := entryTexts(next); got != "second|echo: second" {
		t.Fatalf("the queued prompt's run: %s", got)
	}
}

// A run that ends while its server hands over to a new build starts no
// prompt of its queue: the new build does, as soon as it starts.
func TestQueueOfARunEndedInTheHandoverRunsOnTheNextServer(t *testing.T) {
	a := newApart(t)
	first := a.serve(t)
	runID := first.start(`{"prompt":"hold at the gate","session_id":"session-1"}`)["run_id"].(string)
	first.do("POST", "/api/sessions/session-1/queue", `{"text":"second"}`)
	if !first.server.quiesce() {
		t.Fatal("the server does not hand over")
	}
	a.openGate(t)
	events, _ := first.stream(runID, "")
	if done := events.last(); done.Type != "done" || done.Next != nil || eventQueue(done.Queue) != "second" {
		t.Fatalf("done = %+v, queue %q", done, eventQueue(done.Queue))
	}
	first.goAway()
	second := a.serve(t)
	if second.server.adopted != 0 {
		t.Fatalf("adopted = %d", second.server.adopted)
	}
	body := second.waitFor("session-1", func(body map[string]any) bool {
		entries, _ := body["entries"].([]any)
		return body["run"] == nil && len(entries) == 4
	})
	var texts []string
	for _, e := range body["entries"].([]any) {
		texts = append(texts, e.(map[string]any)["text"].(string))
	}
	if got := queueOf(t, body["queue"]); got != "" || len(texts) != 4 || texts[2] != "second" || texts[3] != "echo: second" {
		t.Fatalf("entries %q, queue %s", texts, got)
	}
}

// A server that takes a new build up waits for no run apart from it, and
// hands over: it takes no new prompts meanwhile.
func TestRunsApartDoNotHoldANewBuildBack(t *testing.T) {
	attached := newHarness(t)
	attached.start(`{"prompt":"wait for it"}`)
	if !attached.server.busy() || attached.server.quiesce() {
		t.Fatal("a run attached to the server does not hold a new build back")
	}

	a := newApart(t)
	h := a.serve(t)
	h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	if !h.server.quiesce() {
		t.Fatal("a run apart from the server holds a new build back")
	}
	for _, body := range []string{`{"text":"late"}`, `{"text":"late","force":true}`} {
		if res, _ := h.do("POST", "/api/sessions/session-1/queue", body); res.StatusCode != http.StatusServiceUnavailable {
			t.Fatalf("a prompt while handing over: %d", res.StatusCode)
		}
	}
	if res, _ := h.do("POST", "/api/runs", `{"prompt":"late"}`); res.StatusCode != http.StatusServiceUnavailable {
		t.Fatalf("a run while handing over: %d", res.StatusCode)
	}
	// The new build did not start after all: the server goes on.
	h.server.reopen()
	if res, _ := h.do("POST", "/api/sessions/session-1/queue", `{"text":"late"}`); res.StatusCode != http.StatusCreated {
		t.Fatalf("a prompt after: %d", res.StatusCode)
	}
}

// A run whose runner was rebuilt since it started says so, and starts again
// with the new build when asked to, from where it got to.
func TestRestartTakesTheNewRunnerUp(t *testing.T) {
	exe := cockpittest.Runner(t)
	runner := filepath.Join(t.TempDir(), "kou-conveyor-runner")
	build := func(name string) {
		next := runner + ".new"
		script := "#!/bin/sh\n# " + name + "\nexec '" + exe + "' \"$@\"\n"
		if err := os.WriteFile(next, []byte(script), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.Rename(next, runner); err != nil {
			t.Fatal(err)
		}
	}
	build("build 1")
	a := newApart(t)
	h := a.serve(t, func(o *options) { o.Runner = runner })
	runID := h.start(`{"prompt":"wait for it","session_id":"session-1"}`)["run_id"].(string)
	// The agent is at work: it recorded the prompt.
	body := h.waitFor("session-1", func(body map[string]any) bool {
		entries, _ := body["entries"].([]any)
		return body["run"] != nil && len(entries) > 0
	})
	if body["run"].(map[string]any)["outdated"] != nil {
		t.Fatalf("a run of the runner beside the server is outdated: %v", body["run"])
	}
	build("the second build")
	h.waitFor("session-1", func(body map[string]any) bool {
		run, _ := body["run"].(map[string]any)
		return run != nil && run["outdated"] == true
	})
	if res, _ := h.do("POST", "/api/runs/"+runID+"/restart", ""); res.StatusCode != http.StatusAccepted {
		t.Fatalf("restart: %d", res.StatusCode)
	}
	events, _ := h.stream(runID, "")
	done := events.last()
	if !done.Stopped || !done.Restarted || done.Next == nil {
		t.Fatalf("done = %+v", done)
	}
	next, _ := h.stream(done.Next.ID, "")
	if done := next.last(); done.Type != "done" || done.Error != "" || done.Stopped {
		t.Fatalf("the run that goes on: %+v", done)
	}
	if got := entryTexts(next); got != cockpit.ContinuePrompt+"|echo: "+cockpit.ContinuePrompt {
		t.Fatalf("the run that goes on: %s", got)
	}
	if res, _ := h.do("POST", "/api/runs/"+runID+"/restart", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("restart of a run that ended: %d", res.StatusCode)
	}
}
