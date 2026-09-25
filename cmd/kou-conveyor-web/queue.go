package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"io"
	"mime"
	"net/http"
	"strings"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Queues. What a browser sends while a session's agent works waits in the
// session's queue on the server (see cockpit.Queue), so every tab sees it
// and it outlives the tab that sent it. A queued prompt runs once the run
// ends on its own: the server starts it in the run's place, and the run's
// done event names it. A forced one goes to the running agent, which reads it
// after the tool calls it is making. A run that is stopped or fails pauses
// the queue until a browser resumes it. Changes reach those who follow the
// run as queue events.

// queueSnapshot copies a session's queue; the caller holds s.mu.
func (s *server) queueSnapshot(key string) cockpit.Queue {
	if q := s.queues[key]; q != nil {
		return q.Clone()
	}
	return cockpit.Queue{Items: []cockpit.QueueItem{}}
}

// queueFor returns a session's queue, making it; the caller holds s.mu.
func (s *server) queueFor(key string) *cockpit.Queue {
	q := s.queues[key]
	if q == nil {
		q = &cockpit.Queue{}
		s.queues[key] = q
	}
	return q
}

// snapshotOf is the queue of a run's session, as it is now.
func (s *server) snapshotOf(current *run) *cockpit.Queue {
	s.mu.Lock()
	defer s.mu.Unlock()
	queue := s.queueSnapshot(activeKey(current.ws, current.sessionID))
	return &queue
}

// delivered takes the forced prompts the runner recorded out of the run's
// queue, and tells those who follow the run.
func (s *server) delivered(current *run, tr *cockpit.Transcript) {
	s.mu.Lock()
	q := s.queues[activeKey(current.ws, current.sessionID)]
	if q == nil || len(q.Delivered(tr)) == 0 {
		s.mu.Unlock()
		return
	}
	queue := q.Clone()
	s.mu.Unlock()
	current.publish(event{Type: "queue", Queue: &queue})
}

// settle takes the end of a run into account in its session's queue and
// returns the prompt that runs next, if any. The session stays reserved for
// that prompt; otherwise it is free.
func (s *server) settle(current *run, tr *cockpit.Transcript, outcome string) (*cockpit.QueueItem, cockpit.Queue) {
	key := activeKey(current.ws, current.sessionID)
	s.mu.Lock()
	defer s.mu.Unlock()
	var next *cockpit.QueueItem
	if q := s.queues[key]; q != nil {
		q.Delivered(tr)
		q.Settle(outcome)
		if !s.closed {
			if item, ok := q.Next(); ok {
				next = &item
			}
		}
	}
	if next == nil && s.active[key] == current {
		delete(s.active, key)
	}
	return next, s.queueSnapshot(key)
}

// runNext runs the next queued prompt in the place of the run that ended. A
// prompt that cannot start goes back first in line, and the queue pauses.
func (s *server) runNext(current *run, item cockpit.QueueItem) (*run, error) {
	thinking, _ := s.effort()
	following, err := s.launch(s.ctx, current.ws, startRequest{
		Prompt: item.Text, SessionID: current.sessionID, MessageID: item.ID, Model: s.modelOf(item), Thinking: thinking,
		Images: item.Images,
	}, current)
	if err != nil {
		key := activeKey(current.ws, current.sessionID)
		s.mu.Lock()
		if s.active[key] == current {
			delete(s.active, key)
		}
		q := s.queueFor(key)
		q.Insert(0, item)
		q.Paused = true
		s.mu.Unlock()
		return nil, err
	}
	return following, nil
}

// force hands a queued prompt to the running agent, which reads it after the
// tool calls it is making. The caller holds s.mu. The error says why the
// prompt stays queued instead.
func (s *server) force(current *run, q *cockpit.Queue, id string) error {
	switch {
	case current.compact:
		return errors.New("the conversation is being compacted — the message runs after the summary")
	case current.job == nil:
		return errors.New("the run is starting — force it again in a moment, or it runs when the agent finishes")
	case current.isStopping():
		return errors.New("the run is stopping — the message waits in the queue")
	}
	select {
	case <-current.job.Done():
		return errors.New("the run is ending — the message runs next")
	default:
	}
	if !current.job.CanSteer() {
		return errors.New("this runner takes no messages while it runs (make build updates it) — the message runs when the agent finishes")
	}
	item, ok := q.Force(id)
	if !ok {
		return errors.New("that message is no longer in the queue")
	}
	if err := current.job.Steer(item.ID, item.Text, item.Images...); err != nil {
		q.Unforce(id)
		return err
	}
	return nil
}

// queueRequest is a prompt for a queue, or a change to a queued one.
type queueRequest struct {
	Text     *string `json:"text"`
	Force    bool    `json:"force"`
	Position *int    `json:"position"`
	// Model is the model the prompt runs with, "" for the connection's.
	Model *string `json:"model"`
	// Images go with the prompt, which refers to each by its label.
	Images []cockpit.Image `json:"images"`
}

// queueRoute resolves the workspace and session of a queue request, and
// decodes its body into req when there is one.
func (s *server) queueRoute(w http.ResponseWriter, r *http.Request, req *queueRequest) (*workspace, string, bool) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return nil, "", false
	}
	id := r.PathValue("id")
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return nil, "", false
	}
	if req != nil {
		if media, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type")); media != "application/json" {
			writeError(w, http.StatusUnsupportedMediaType, "expected application/json")
			return nil, "", false
		}
		// Prompts may be as long as the ones runs start with.
		if err := json.UnmarshalRead(io.LimitReader(r.Body, maxPromptBytes), req, json.RejectUnknownMembers(true)); err != nil {
			writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
			return nil, "", false
		}
		if req.Text != nil {
			*req.Text = strings.TrimSpace(*req.Text)
			if strings.TrimSpace(cockpit.Clean(*req.Text)) == "" {
				writeError(w, http.StatusBadRequest, "text is required")
				return nil, "", false
			}
		}
		if len(req.Images) != 0 {
			text := ""
			if req.Text != nil {
				text = *req.Text
			}
			images, err := prepareImages(text, req.Images)
			if err != nil {
				writeError(w, http.StatusBadRequest, "images: "+err.Error())
				return nil, "", false
			}
			req.Images = images
		}
		if req.Model != nil {
			*req.Model = strings.TrimSpace(*req.Model)
			if err := cockpit.ValidModel(*req.Model); err != nil {
				writeError(w, http.StatusBadRequest, err.Error())
				return nil, "", false
			}
		}
	}
	return ws, id, true
}

// runNow starts a run of a prompt that has nothing to wait for, under the
// message ID it was given.
func (s *server) runNow(ctx context.Context, ws *workspace, id string, item cockpit.QueueItem) (*run, error) {
	thinking, _ := s.effort()
	return s.launch(ctx, ws, startRequest{
		Prompt: item.Text, SessionID: id, MessageID: item.ID, Model: s.modelOf(item), Thinking: thinking,
		Images: item.Images,
	}, nil)
}

// modelOf is the model a queued prompt runs with: the one it was queued
// with, else the server's -model, else the connection's.
func (s *server) modelOf(item cockpit.QueueItem) string {
	if item.Model != "" {
		return item.Model
	}
	return s.opt.model
}

// answerRun tells the browser that a prompt runs now, and in which run.
func (s *server) answerRun(w http.ResponseWriter, started *run, item cockpit.QueueItem) {
	writeJSON(w, http.StatusAccepted, map[string]any{
		"run":   runSummary{ID: started.id, Started: started.started},
		"item":  item,
		"queue": s.snapshotOf(started),
	})
}

// answerQueue tells those who follow the session's run, and the browser
// that asked, what the queue is now. The caller holds s.mu, which this
// releases.
func (s *server) answerQueue(w http.ResponseWriter, key string, status int, extra map[string]any) {
	queue := s.queueSnapshot(key)
	current := s.active[key]
	s.mu.Unlock()
	if current != nil {
		current.publish(event{Type: "queue", Queue: &queue})
	}
	body := map[string]any{"queue": queue}
	for name, value := range extra {
		body[name] = value
	}
	writeJSON(w, status, body)
}

func (s *server) handleQueue(w http.ResponseWriter, r *http.Request) {
	ws, id, ok := s.queueRoute(w, r, nil)
	if !ok {
		return
	}
	s.mu.Lock()
	queue := s.queueSnapshot(activeKey(ws, id))
	s.mu.Unlock()
	writeJSON(w, http.StatusOK, map[string]any{"queue": queue})
}

// handleEnqueue adds a prompt to a session's queue: it waits for the run
// going on to end, or, forced, goes to the running agent. With nothing to
// wait for, it runs at once.
func (s *server) handleEnqueue(w http.ResponseWriter, r *http.Request) {
	var req queueRequest
	ws, id, ok := s.queueRoute(w, r, &req)
	if !ok {
		return
	}
	if req.Text == nil {
		writeError(w, http.StatusBadRequest, "text is required")
		return
	}
	key := activeKey(ws, id)
	s.mu.Lock()
	q := s.queueFor(key)
	current := s.active[key]
	model := ""
	if req.Model != nil {
		model = *req.Model
	}
	if current == nil && (req.Force || !q.Paused && len(q.Items) == 0) {
		s.mu.Unlock()
		item := cockpit.QueueItem{ID: uuid.New().String(), Text: *req.Text, Model: model, Images: req.Images}
		started, err := s.runNow(r.Context(), ws, id, item)
		var busy *busyError
		if !errors.As(err, &busy) {
			if err != nil {
				launchError(w, err)
			} else {
				s.answerRun(w, started, item)
			}
			return
		}
		// A run started meanwhile: the prompt waits for it after all.
		s.mu.Lock()
		current = s.active[key]
	}
	item := q.Add(*req.Text, model)
	q.SetImages(item.ID, req.Images)
	note := ""
	if req.Force {
		if err := s.force(current, q, item.ID); err != nil {
			note = err.Error()
		} else if model != "" && current.model != "" && model != current.model {
			// The running agent reads it, with the model it runs with.
			note = "forced in: the running agent reads it with " + current.model + "; " + model + " takes the prompts that run next"
		}
	}
	item, _, _ = q.Get(item.ID)
	s.answerQueue(w, key, http.StatusCreated, map[string]any{"item": item, "note": note})
}

// handleUpdateQueued edits, moves or forces a queued prompt. Between runs,
// forcing one runs it.
func (s *server) handleUpdateQueued(w http.ResponseWriter, r *http.Request) {
	var req queueRequest
	ws, id, ok := s.queueRoute(w, r, &req)
	if !ok {
		return
	}
	key, itemID := activeKey(ws, id), r.PathValue("item")
	s.mu.Lock()
	q := s.queueFor(key)
	item, _, found := q.Get(itemID)
	if !found {
		s.mu.Unlock()
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "that message is no longer in the queue", "code": "queue_gone"})
		return
	}
	if (req.Text != nil || req.Position != nil || req.Model != nil) && item.Forced {
		s.mu.Unlock()
		writeError(w, http.StatusConflict, "the agent has this message already")
		return
	}
	if req.Text != nil {
		q.Edit(itemID, *req.Text)
		// The images its text no longer names leave with their labels.
		q.SetImages(itemID, item.Images)
	}
	if req.Position != nil {
		q.Move(itemID, *req.Position)
	}
	if req.Model != nil {
		q.SetModel(itemID, *req.Model)
	}
	note := ""
	if req.Force && !item.Forced {
		current := s.active[key]
		if current == nil {
			item, index, _ := q.Get(itemID)
			q.Remove(itemID)
			s.mu.Unlock()
			started, err := s.runNow(r.Context(), ws, id, item)
			if err != nil {
				s.mu.Lock()
				q.Insert(index, item)
				s.mu.Unlock()
				launchError(w, err)
				return
			}
			s.answerRun(w, started, item)
			return
		}
		if err := s.force(current, q, itemID); err != nil {
			note = err.Error()
		}
	}
	s.answerQueue(w, key, http.StatusOK, map[string]any{"note": note})
}

// handleDropQueued takes a queued prompt out of the queue. A forced one is
// the agent's already.
func (s *server) handleDropQueued(w http.ResponseWriter, r *http.Request) {
	ws, id, ok := s.queueRoute(w, r, nil)
	if !ok {
		return
	}
	key, itemID := activeKey(ws, id), r.PathValue("item")
	s.mu.Lock()
	q := s.queueFor(key)
	item, _, found := q.Get(itemID)
	switch {
	case !found:
		s.mu.Unlock()
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "that message is no longer in the queue", "code": "queue_gone"})
		return
	case item.Forced:
		s.mu.Unlock()
		writeError(w, http.StatusConflict, "the agent has this message already — it cannot be taken back")
		return
	}
	q.Remove(itemID)
	s.answerQueue(w, key, http.StatusOK, nil)
}

// handleClearQueue drops the queued prompts; forced ones are the agent's.
func (s *server) handleClearQueue(w http.ResponseWriter, r *http.Request) {
	ws, id, ok := s.queueRoute(w, r, nil)
	if !ok {
		return
	}
	key := activeKey(ws, id)
	s.mu.Lock()
	q := s.queueFor(key)
	var forced []cockpit.QueueItem
	for _, item := range q.Items {
		if item.Forced {
			forced = append(forced, item)
		}
	}
	q.Clear()
	q.Items = forced
	s.answerQueue(w, key, http.StatusOK, nil)
}

// handlePauseQueue keeps the queued prompts from running when the run ends.
func (s *server) handlePauseQueue(w http.ResponseWriter, r *http.Request) {
	ws, id, ok := s.queueRoute(w, r, nil)
	if !ok {
		return
	}
	key := activeKey(ws, id)
	s.mu.Lock()
	if q := s.queueFor(key); len(q.Items) != 0 {
		q.Paused = true
	}
	s.answerQueue(w, key, http.StatusOK, nil)
}

// handleResumeQueue lifts the pause; between runs the next prompt runs.
func (s *server) handleResumeQueue(w http.ResponseWriter, r *http.Request) {
	ws, id, ok := s.queueRoute(w, r, nil)
	if !ok {
		return
	}
	key := activeKey(ws, id)
	s.mu.Lock()
	q := s.queueFor(key)
	q.Paused = false
	if s.active[key] == nil {
		if item, ok := q.Next(); ok {
			s.mu.Unlock()
			started, err := s.runNow(r.Context(), ws, id, item)
			if err != nil {
				// A prompt that cannot start waits first in line.
				s.mu.Lock()
				q.Insert(0, item)
				q.Paused = true
				s.mu.Unlock()
				launchError(w, err)
				return
			}
			s.answerRun(w, started, item)
			return
		}
	}
	s.answerQueue(w, key, http.StatusOK, nil)
}
