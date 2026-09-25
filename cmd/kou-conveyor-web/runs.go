package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log"
	"mime"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// run brokers one runner process to any number of browsers. Its events form
// an append-only log: subscribers read from their own offset and wait for
// more, so a slow or reconnecting browser never loses events and never
// blocks the runner.
type run struct {
	id, sessionID, title string
	message              string // the prompt's message ID; empty for a compaction
	model                string // the model the run was started with, as far as the server can tell
	compact              bool   // the run compacts the conversation instead of running a prompt
	ws                   *workspace
	started              time.Time
	job                  *cockpit.Job
	tracker              *cockpit.Tracker // what the run changes; nil when it is not recorded

	mu       sync.Mutex
	events   [][]byte      // encoded events; event i has SSE ID i+1
	wake     chan struct{} // closed and replaced whenever the log grows
	stopping bool
	done     bool
}

// event is what browsers receive. Entry events are complete snapshots keyed by
// entry ID, so applying one twice is harmless.
type event struct {
	Type     string         `json:"type"` // entry, status, log or done
	Entry    *cockpit.Entry `json:"entry,omitempty"`
	Usage    *cockpit.Usage `json:"usage,omitempty"`
	Activity string         `json:"activity,omitempty"`
	Stopping bool           `json:"stopping,omitempty"`
	Text     string         `json:"text,omitempty"`
	Error    string         `json:"error,omitempty"`
	Stopped  bool           `json:"stopped,omitempty"`
	Size     int64          `json:"size,omitempty"` // done: bytes in the session file after the run
	// Changes: a snapshot found the workspace changed.
	Changes *cockpit.ChangeUpdate `json:"changes,omitempty"`
	// Queue: what waits for the session's agent, as it is now (queue and
	// done events).
	Queue *cockpit.Queue `json:"queue,omitempty"`
	// Next: the run of the queued prompt that took over from this one (done).
	Next *runSummary `json:"next,omitempty"`
}

func (r *run) publish(e event) {
	data, err := json.Marshal(e)
	if err != nil {
		log.Printf("run %s: encode event: %v", r.id, err)
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.done {
		return
	}
	r.events = append(r.events, data)
	if e.Type == "done" {
		r.done = true
	}
	close(r.wake)
	r.wake = make(chan struct{})
}

// since returns the events after the first n, whether the log is complete and
// a channel that is closed when it grows.
func (r *run) since(n int) ([][]byte, bool, <-chan struct{}) {
	r.mu.Lock()
	defer r.mu.Unlock()
	var events [][]byte
	if n < len(r.events) {
		// Published events are never modified, so the slice outlives the lock.
		events = r.events[n:len(r.events):len(r.events)]
	}
	return events, r.done, r.wake
}

func (r *run) isStopping() bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.stopping
}

func (r *run) stop() {
	r.mu.Lock()
	first := !r.stopping && !r.done
	r.stopping = true
	r.mu.Unlock()
	if first {
		r.publish(event{Type: "status", Stopping: true, Activity: "Stopping"})
		r.job.Cancel()
	}
}

func (s *server) lookup(id string) *run {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.runs[id]
}

type startRequest struct {
	Prompt    string `json:"prompt"`
	SessionID string `json:"session_id"`
	MessageID string `json:"message_id"`
	Model     string `json:"model"`
	Thinking  string `json:"thinking_level"`
	// Resume is set for a session the browser already shows, which must
	// still exist.
	Resume bool `json:"resume"`
	// Rewind is the message ID of a prompt of the session that this one
	// replaces: the session goes back to how it was before that prompt was
	// sent, and this prompt runs from there.
	Rewind string `json:"rewind"`
	// Compact summarizes the session's conversation instead of running a
	// prompt, which frees the context it takes; Instructions tell the summary
	// what to focus on. The session must exist.
	Compact      bool   `json:"compact"`
	Instructions string `json:"instructions"`
	// Images go with the prompt, which refers to each by its label.
	Images []cockpit.Image `json:"images"`
}

func (s *server) handleStart(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	if media, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type")); media != "application/json" {
		writeError(w, http.StatusUnsupportedMediaType, "expected application/json")
		return
	}
	var req startRequest
	if err := json.UnmarshalRead(io.LimitReader(r.Body, maxPromptBytes), &req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	req.Prompt = strings.TrimSpace(req.Prompt)
	switch {
	case req.Compact && (req.Prompt != "" || req.Rewind != ""):
		writeError(w, http.StatusBadRequest, "a compaction runs no prompt")
		return
	case req.Compact && req.SessionID == "":
		writeError(w, http.StatusBadRequest, "compact needs the session_id of the session to compact")
		return
	case req.Instructions != "" && !req.Compact:
		writeError(w, http.StatusBadRequest, "instructions need compact")
		return
	case !req.Compact && strings.TrimSpace(cockpit.Clean(req.Prompt)) == "":
		writeError(w, http.StatusBadRequest, "prompt is required")
		return
	case req.SessionID != "" && !cockpit.ValidSessionID(req.SessionID):
		writeError(w, http.StatusBadRequest, "session_id must contain only letters, digits and dashes")
		return
	case req.Thinking != "" && !cockpit.ValidThinkingLevel(req.Thinking):
		writeError(w, http.StatusBadRequest, "thinking_level must be low, medium, high, xhigh or max")
		return
	case cockpit.ValidModel(strings.TrimSpace(req.Model)) != nil:
		writeError(w, http.StatusBadRequest, "model must be an ID without spaces")
		return
	case req.Rewind != "" && req.SessionID == "":
		writeError(w, http.StatusBadRequest, "rewind needs the session_id of the prompt it replaces")
		return
	}
	if req.MessageID != "" {
		if _, err := uuid.Parse(req.MessageID); err != nil {
			writeError(w, http.StatusBadRequest, "message_id must be a UUID")
			return
		}
	} else if !req.Compact {
		req.MessageID = uuid.New().String()
	}
	if req.SessionID == "" {
		req.SessionID = uuid.New().String()
	}
	if req.Thinking == "" {
		req.Thinking, _ = s.effort()
	}
	if req.Model = strings.TrimSpace(req.Model); req.Model == "" {
		req.Model = s.opt.model
	}
	images, err := prepareImages(req.Prompt, req.Images)
	if err != nil {
		writeError(w, http.StatusBadRequest, "images: "+err.Error())
		return
	}
	req.Images = images
	current, err := s.launch(r.Context(), ws, req, nil)
	if err != nil {
		launchError(w, err)
		return
	}
	writeJSON(w, http.StatusAccepted, map[string]any{
		"run_id": current.id, "session_id": req.SessionID, "message_id": req.MessageID, "workspace": ws.ID,
		"compact": req.Compact,
	})
}

// Why a run could not start, besides the runner's reasons (startError).
var (
	errClosed = errors.New("server is shutting down")
	errGone   = errors.New("the workspace folder no longer exists")
)

// busyError reports a session that another run of this server holds.
type busyError struct{ run *run }

func (err *busyError) Error() string { return "this session is already running" }

// launch starts a run of a validated request. handover is the run that ended
// and hands the session on to this one, which it keeps reserved meanwhile:
// the next prompt of its queue. ctx bounds the first snapshot of the
// workspace.
func (s *server) launch(ctx context.Context, ws *workspace, req startRequest, handover *run) (*run, error) {
	if info, err := os.Stat(ws.Path); err != nil || !info.IsDir() {
		return nil, fmt.Errorf("%w: %s", errGone, ws.Path)
	}

	now := time.Now().UTC()
	// A prompt that names no model runs with the connection's.
	model := req.Model
	if model == "" {
		settings, _ := s.settings()
		model = s.connection(settings, ws).Model
	}
	current := &run{
		id: uuid.New().String(), sessionID: req.SessionID, title: cockpit.Headline(req.Prompt, 80),
		message: req.MessageID, compact: req.Compact, model: model, ws: ws, started: now, wake: make(chan struct{}),
	}
	key := activeKey(ws, req.SessionID)
	// Reserve the session first: two concurrent requests for one session must
	// not both get past this point.
	s.mu.Lock()
	switch busy := s.active[key]; {
	case s.closed:
		if busy == handover && handover != nil {
			delete(s.active, key)
		}
		s.mu.Unlock()
		return nil, errClosed
	case busy != nil && busy != handover:
		s.mu.Unlock()
		return nil, &busyError{run: busy}
	}
	s.active[key] = current
	s.pumps.Add(1)
	s.mu.Unlock()
	release := func() {
		s.mu.Lock()
		if s.active[key] == current {
			delete(s.active, key)
		}
		s.mu.Unlock()
		s.pumps.Done()
	}

	// The run's transcript starts from the persisted session, so every entry
	// it publishes is complete even when it continues older tool calls. A
	// rewinding run starts from the session as the rewind leaves it, which
	// Start makes the file under the session's lock.
	var tr *cockpit.Transcript
	var err error
	if req.Rewind != "" {
		tr, err = cockpit.LoadSessionBefore(ws.opt.SessionDir, req.SessionID, req.Rewind)
	} else {
		tr, err = cockpit.LoadSession(ws.opt.SessionDir, req.SessionID)
		if errors.Is(err, fs.ErrNotExist) && !req.Resume && !req.Compact {
			tr, err = cockpit.NewTranscript(), nil
		}
	}
	if errors.Is(err, fs.ErrNotExist) {
		err = cockpit.ErrSessionGone
	}
	if err != nil {
		release()
		return nil, err
	}
	var prompt *cockpit.Entry
	var tracker *cockpit.Tracker
	if req.Compact {
		// A compaction changes no files, and its only entry is the notice
		// with the summary.
		tr.Begin(current.id, "Compacting context")
	} else {
		prompt = tr.SubmitModel(req.MessageID, req.Prompt, model, now)
		if len(req.Images) != 0 {
			tr.WithImages(prompt, req.Images)
		}
		// The workspace as the run finds it, to show what the run changes. A
		// workspace that takes too long to snapshot runs without.
		snapshot, cancel := context.WithTimeout(ctx, 3*time.Second)
		tracker, err = ws.changes().Begin(snapshot, req.SessionID, req.MessageID)
		cancel()
		if err != nil {
			log.Printf("run %s: changes are not recorded: %v", current.id, err)
			tracker = nil
		}
	}
	job, err := cockpit.Start(s.ctx, ws.opt, cockpit.Request{
		SessionID: req.SessionID, MessageID: req.MessageID, Prompt: req.Prompt, Model: req.Model, Thinking: req.Thinking,
		Resume: req.Resume, Rewind: req.Rewind, Compact: req.Compact, Instructions: req.Instructions,
		Images: req.Images,
	})
	if err != nil {
		if tracker != nil {
			tracker.Cancel()
		}
		release()
		return nil, err
	}
	// The queue's handlers find the run by its session before it is
	// registered: they read job under s.mu.
	s.mu.Lock()
	current.job, current.tracker = job, tracker
	queue := s.queueSnapshot(key)
	s.runs[current.id] = current
	s.mu.Unlock()
	if prompt != nil {
		current.publish(event{Type: "entry", Entry: prompt})
	}
	current.publish(event{Type: "status", Activity: tr.Activity, Usage: &tr.Usage})
	// Those who follow the run learn what waits for it.
	if len(queue.Items) != 0 {
		current.publish(event{Type: "queue", Queue: &queue})
	}
	go s.pump(current, tr)
	return current, nil
}

// launchError reports why a run could not start.
func launchError(w http.ResponseWriter, err error) {
	var busy *busyError
	switch {
	case errors.As(err, &busy):
		writeJSON(w, http.StatusConflict, map[string]string{"error": busy.Error(), "run_id": busy.run.id})
	case errors.Is(err, errClosed):
		writeError(w, http.StatusServiceUnavailable, err.Error())
	case errors.Is(err, errGone):
		writeError(w, http.StatusConflict, "the workspace folder "+strings.TrimPrefix(err.Error(), errGone.Error()+": ")+" no longer exists")
	default:
		startError(w, err)
	}
}

// startError reports why a run could not start.
func startError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, cockpit.ErrPromptGone):
		// Another window rewound the session past the prompt, or the runner
		// never persisted it.
		writeJSON(w, http.StatusConflict, map[string]string{"error": "that prompt is no longer in the session", "code": "prompt_gone"})
	case errors.Is(err, cockpit.ErrSessionBusy):
		writeError(w, http.StatusConflict, "this session is running in another window or terminal")
	case errors.Is(err, cockpit.ErrSessionGone):
		writeError(w, http.StatusGone, "this session was deleted")
	default:
		writeError(w, http.StatusInternalServerError, err.Error())
	}
}

// pump folds runner output into the run's transcript and publishes every
// changed entry.
func (s *server) pump(current *run, tr *cockpit.Transcript) {
	defer s.pumps.Done()
	// Snapshots that find changes reach browsers as they come.
	forwarded := make(chan struct{})
	if current.tracker == nil {
		close(forwarded)
	} else {
		go func() {
			defer close(forwarded)
			for update := range current.tracker.Updates() {
				current.publish(event{Type: "changes", Changes: &update})
			}
		}()
	}
	activity, usage := tr.Activity, tr.Usage
	for line := range current.job.Lines() {
		if line.Stderr {
			current.publish(event{Type: "log", Text: cockpit.Clean(line.Text)})
			continue
		}
		changed, err := tr.Apply([]byte(line.Text))
		if err != nil {
			current.publish(event{Type: "log", Text: "unreadable runner output: " + err.Error()})
			continue
		}
		for _, e := range changed {
			current.publish(event{Type: "entry", Entry: e})
			// A finished tool call may have changed files.
			if e.Kind == cockpit.KindTool && e.Tool.Terminal() && current.tracker != nil {
				current.tracker.Poke()
			}
			// A forced prompt the runner recorded leaves the queue.
			if e.Kind == cockpit.KindUser && e.Forced {
				s.delivered(current, tr)
			}
		}
		if tr.Activity != activity || tr.Usage != usage {
			activity, usage = tr.Activity, tr.Usage
			current.publish(event{Type: "status", Activity: activity, Usage: &usage, Stopping: current.isStopping()})
		}
	}
	err := current.job.Err()
	// The last snapshot goes out before browsers hear the run is done.
	if current.tracker != nil {
		current.tracker.Finish()
	}
	<-forwarded
	stopped := errors.Is(err, context.Canceled)
	for _, e := range tr.Finish(err, stopped, time.Now().UTC()) {
		current.publish(event{Type: "entry", Entry: e})
	}
	// The session is free (the runner released its lock when it exited)
	// before browsers hear that the run is done, so they can start the next
	// one right away; or the next queued prompt has it already.
	outcome := "done"
	switch {
	case stopped:
		outcome = "stopped"
	case err != nil:
		outcome = "failed"
	}
	next, queue := s.settle(current, tr, outcome)
	done := event{Type: "done", Stopped: stopped, Usage: &tr.Usage, Queue: &queue}
	if err != nil && !stopped {
		done.Error = err.Error()
	}
	// Browsers compare this with the session list to notice later writers,
	// such as the run that takes over.
	if info, err := os.Stat(cockpit.SessionPath(current.ws.opt.SessionDir, current.sessionID)); err == nil {
		done.Size = info.Size()
	}
	if next != nil {
		if following, err := s.runNext(current, *next); err != nil {
			current.publish(event{Type: "log", Text: "the next queued prompt did not start: " + err.Error()})
			done.Queue = s.snapshotOf(current)
		} else {
			done.Next = &runSummary{ID: following.id, Started: following.started}
			done.Queue = s.snapshotOf(current)
		}
	}
	current.publish(done)
	time.AfterFunc(runRetention, func() {
		s.mu.Lock()
		delete(s.runs, current.id)
		s.mu.Unlock()
	})
}

func (s *server) handleCancel(w http.ResponseWriter, r *http.Request) {
	current := s.lookup(r.PathValue("id"))
	if current == nil {
		writeError(w, http.StatusNotFound, "run not found")
		return
	}
	current.stop()
	writeJSON(w, http.StatusAccepted, map[string]bool{"ok": true})
}

// handleEvents streams a run's event log as server-sent events. Browsers
// resume with Last-Event-ID after a dropped connection.
func (s *server) handleEvents(w http.ResponseWriter, r *http.Request) {
	current := s.lookup(r.PathValue("id"))
	if current == nil {
		writeError(w, http.StatusNotFound, "run not found")
		return
	}
	next := 0
	if last := r.Header.Get("Last-Event-ID"); last != "" {
		next, _ = strconv.Atoi(last)
	} else if after := r.URL.Query().Get("after"); after != "" {
		next, _ = strconv.Atoi(after)
	}
	next = max(0, next)
	events, done, wake := current.since(next)
	if done && len(events) == 0 {
		// Everything was delivered; 204 tells EventSource not to reconnect.
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
	heartbeat := time.NewTicker(15 * time.Second)
	defer heartbeat.Stop()
	for {
		for i, data := range events {
			fmt.Fprintf(w, "id: %d\ndata: %s\n\n", next+i+1, data)
		}
		next += len(events)
		if err := flusher.Flush(); err != nil || done {
			return
		}
		select {
		case <-wake:
		case <-heartbeat.C:
			fmt.Fprint(w, ": ping\n\n")
		case <-r.Context().Done():
			return
		}
		events, done, wake = current.since(next)
	}
}

// close refuses new runs, interrupts running ones (their context is the
// server's, which is already canceled) and waits for them to exit.
func (s *server) close(ctx context.Context) bool {
	s.mu.Lock()
	s.closed = true
	s.mu.Unlock()
	finished := make(chan struct{})
	go func() {
		s.pumps.Wait()
		close(finished)
	}()
	select {
	case <-finished:
	case <-ctx.Done():
		return false
	}
	// The gateway stops with the server's context, saves its history and
	// withdraws its publication.
	if s.gatewayGone == nil {
		return true
	}
	select {
	case <-s.gatewayGone:
		return true
	case <-ctx.Done():
		return false
	}
}
