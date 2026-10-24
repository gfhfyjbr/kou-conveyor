package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Runs apart from the server. With -detach, a runner runs apart from the
// server (cockpit.Request.Detached): in a process group of its own, reading
// and writing the files of its run's directory, <session directory>/.runs/
// <run ID>, rather than pipes of the server's. When the server goes — it
// shuts down, or takes a new build up — the runner goes on; the server that
// starts next takes the run up from its directory (adopt), and settles it
// once it ends. What waits in the queues goes from one server to the next in
// queues.json beside the settings. A runner too old to run apart runs
// attached, as before: the server waits for it to take a new build up, and
// stops it when it shuts down.

// runsDir holds the directories of a session directory's runs.
const runsDir = ".runs"

// runRecordFile is what the server writes of a run in its directory, for the
// server that takes the run up; the runner writes job.json.
const runRecordFile = "run.json"

// runRecord is what a server that takes a run up needs to know of it besides
// what the runner says (cockpit.DetachedRun).
type runRecord struct {
	Session string    `json:"session"`
	Message string    `json:"message,omitzero"` // the prompt's message ID; empty for a compaction
	Prompt  string    `json:"prompt,omitzero"`  // as the runner was given it
	Title   string    `json:"title,omitzero"`
	Model   string    `json:"model,omitzero"`
	Compact bool      `json:"compact,omitzero"`
	Started time.Time `json:"started"`
}

// runDir is the directory of a run of the workspace.
func runDir(ws *workspace, id string) string {
	return filepath.Join(ws.opt.SessionDir, runsDir, id)
}

// writeRunRecord makes a run's directory, with the run's record in it.
func writeRunRecord(dir string, record runRecord) error {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(record)
	if err != nil {
		return err
	}
	temporary := filepath.Join(dir, "."+runRecordFile+".new")
	if err := os.WriteFile(temporary, append(data, '\n'), 0o600); err != nil {
		return err
	}
	return os.Rename(temporary, filepath.Join(dir, runRecordFile))
}

func readRunRecord(dir string) (runRecord, error) {
	var record runRecord
	data, err := os.ReadFile(filepath.Join(dir, runRecordFile))
	if err != nil {
		return record, err
	}
	if err := json.Unmarshal(data, &record); err != nil {
		return record, fmt.Errorf("%s: %w", runRecordFile, err)
	}
	if !cockpit.ValidSessionID(record.Session) {
		return record, fmt.Errorf("%s: invalid session ID %q", runRecordFile, record.Session)
	}
	return record, nil
}

// adopt takes up the runs that went on apart from the server before this one
// started. They go on as runs this server started, under new IDs: the pages
// that followed them read their sessions again, which name their runs. A run
// that ended meanwhile ends here as it did. adopt returns how many runs it
// took up.
func (s *server) adopt() int {
	adopted := 0
	for _, ws := range s.workspaces.all() {
		root := filepath.Join(ws.opt.SessionDir, runsDir)
		entries, err := os.ReadDir(root)
		if err != nil {
			continue
		}
		for _, entry := range entries {
			if !entry.IsDir() || strings.HasPrefix(entry.Name(), ".") {
				continue
			}
			dir := filepath.Join(root, entry.Name())
			switch err := s.adoptRun(ws, dir); {
			case err == nil:
				adopted++
			case errors.Is(err, cockpit.ErrFollowed):
				// Another server follows the run.
			default:
				log.Printf("the run in %s is not taken up: %v", dir, err)
				forgetRun(dir)
			}
		}
	}
	return adopted
}

// adoptRun takes up the run of a directory.
func (s *server) adoptRun(ws *workspace, dir string) error {
	record, err := readRunRecord(dir)
	if err != nil {
		return err
	}
	// The run is let go of when the server goes, or here when it cannot be
	// taken up after all.
	ctx, release := context.WithCancel(s.ctx)
	job, info, err := cockpit.Adopt(ctx, dir)
	if err != nil {
		release()
		return err
	}
	if info.SessionID != record.Session {
		release()
		return fmt.Errorf("its runner runs session %s, not %s", info.SessionID, record.Session)
	}
	// The run's transcript starts from the session as the run found it: the
	// runner's output, which the job delivers from its start, goes on from
	// there.
	tr, err := cockpit.LoadSessionPrefix(ws.opt.SessionDir, record.Session, info.SessionSize)
	if err != nil {
		log.Printf("the run in %s: the session as the run found it is not read: %v", dir, err)
		tr = cockpit.NewTranscript()
	}
	current := &run{
		id: uuid.New().String(), sessionID: record.Session, title: record.Title, message: record.Message,
		compact: record.Compact, model: record.Model, ws: ws, started: record.Started, wake: make(chan struct{}),
		job: job, dir: dir,
	}
	var prompt *cockpit.Entry
	if record.Compact {
		tr.Begin(current.id, "Compacting context")
	} else if record.Message != "" {
		prompt = tr.SubmitModel(record.Message, record.Prompt, record.Model, record.Started)
		// What the run changes goes on from the last snapshot recorded.
		if tracker, err := ws.changes().Resume(record.Session, record.Message); err == nil {
			current.tracker = tracker
		} else {
			log.Printf("run %s: changes are not recorded: %v", current.id, err)
		}
	}
	key := activeKey(ws, record.Session)
	s.mu.Lock()
	if s.closed || s.active[key] != nil {
		s.mu.Unlock()
		if current.tracker != nil {
			current.tracker.Cancel()
		}
		release()
		return fmt.Errorf("session %s has a run already", record.Session)
	}
	s.active[key] = current
	s.runs[current.id] = current
	queue := s.queueSnapshot(key)
	s.pumps.Add(1)
	s.mu.Unlock()
	go func() {
		<-job.Done()
		release()
	}()
	s.canvas.RunStarted(ws.ID, record.Session)
	if prompt != nil {
		current.publish(event{Type: "entry", Entry: prompt})
	}
	current.publish(event{Type: "status", Activity: tr.Activity, Usage: &tr.Usage})
	if len(queue.Items) != 0 {
		current.publish(event{Type: "queue", Queue: &queue})
	}
	go s.pump(current, tr)
	return nil
}

// forgetRun removes the directory of a run that no server can take up,
// unless a runner may still use it: one that runs its session, which ends by
// itself, or one that is being started from it now.
func forgetRun(dir string) {
	if info, err := cockpit.ReadDetachedRun(dir); err == nil {
		if cockpit.SessionBusy(info.SessionDir, info.SessionID) {
			return
		}
	} else if stat, err := os.Stat(dir); err != nil || time.Since(stat.ModTime()) < time.Minute {
		return
	}
	if err := os.RemoveAll(dir); err != nil {
		log.Printf("remove %s: %v", dir, err)
	}
}

// detachedRuns are the runs at work whose runners go on when the server goes.
func (s *server) detachedRuns() []*run {
	s.mu.Lock()
	defer s.mu.Unlock()
	var runs []*run
	for _, current := range s.active {
		if current.job != nil && current.dir != "" {
			runs = append(runs, current)
		}
	}
	return runs
}

// stopRuns stops runs whose runners went on when the server went, from their
// directories, and waits for them to exit. Their sessions record why.
func stopRuns(ctx context.Context, runs []*run, reason string) error {
	var wg sync.WaitGroup
	errs := make([]error, len(runs))
	for i, current := range runs {
		wg.Go(func() {
			if err := cockpit.StopDetached(ctx, current.dir, reason); err != nil && !errors.Is(err, cockpit.ErrNotDetached) {
				errs[i] = fmt.Errorf("session %s: %w", current.sessionID, err)
			}
		})
	}
	wg.Wait()
	return errors.Join(errs...)
}

// goingOn says what becomes of the runs at work when the server takes a new
// build up: going is how many go on, their runners running apart from it,
// and outdated how many of those with the runner build they started with,
// which a restart of the run takes the new one up for.
func (s *server) goingOn() (going, outdated int) {
	runs := s.detachedRuns()
	for _, current := range runs {
		if current.outdated() {
			outdated++
		}
	}
	return len(runs), outdated
}

// goingOnWords says what goingOn counted.
func goingOnWords(going, outdated int) string {
	switch {
	case going == 0:
		return ""
	case outdated == 0:
		return goOn(going)
	case going == 1:
		return goOn(1) + " with the runner it started with: restart it to take the new runner up"
	case outdated == going:
		return goOn(going) + " with the runner they started with: restart them to take the new runner up"
	}
	return fmt.Sprintf("%s, %d of them with the runner they started with: restart those to take the new runner up", goOn(going), outdated)
}

// runsOf says how many runs there are.
func runsOf(n int) string {
	if n == 1 {
		return "1 agent run"
	}
	return fmt.Sprintf("%d agent runs", n)
}

// goOn says that n runs go on.
func goOn(n int) string {
	if n == 1 {
		return "1 agent run goes on"
	}
	return runsOf(n) + " go on"
}

// outdated says whether the run's runner is older than the one beside the
// server: the run started before the server took a new build up, and goes on
// with the build it started with until it is restarted (handleRestart).
func (r *run) outdated() bool {
	if r.job == nil {
		return false
	}
	_, info, ok := r.job.Detached()
	return ok && info.Outdated()
}

// restarting says whether the run is stopped to start again.
func (r *run) restarting() bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.restart
}

// handleRestart stops a run to start it again at once from where it got to,
// with the runner beside the server: the build the server took up after the
// run started. The run that goes on is the done event's next, as for a
// queued prompt.
func (s *server) handleRestart(w http.ResponseWriter, r *http.Request) {
	current := s.lookup(r.PathValue("id"))
	if current == nil {
		writeError(w, http.StatusNotFound, "run not found")
		return
	}
	if current.compact {
		writeError(w, http.StatusConflict, "a compaction does not start again: stop it, and compact again")
		return
	}
	current.mu.Lock()
	ended := current.done
	current.restart = !ended
	current.mu.Unlock()
	if ended {
		writeError(w, http.StatusNotFound, "the run ended")
		return
	}
	current.stop("the user restarted the run, to go on with the runner's new build")
	writeJSON(w, http.StatusAccepted, map[string]bool{"ok": true})
}

// settleRestart takes the end of a run that is stopped to start again into
// account in its session's queue: the forced prompts the agent never read go
// back first in line, and the session stays reserved for the run that goes
// on.
func (s *server) settleRestart(current *run, tr *cockpit.Transcript) cockpit.Queue {
	key := activeKey(current.ws, current.sessionID)
	s.mu.Lock()
	defer s.mu.Unlock()
	if q := s.queues[key]; q != nil {
		q.Delivered(tr)
		q.Settle("done")
	}
	return s.queueSnapshot(key)
}

// restartRun starts the run that goes on from a run stopped to start again:
// the agent is asked to go on where it left off, with the model it ran with.
// When it cannot start, the session is free, and its queue waits.
func (s *server) restartRun(current *run) (*run, error) {
	following, err := s.launch(s.ctx, current.ws, startRequest{
		Prompt: cockpit.ContinuePrompt, SessionID: current.sessionID, MessageID: uuid.New().String(),
		Model: current.model, Resume: true,
	}, current)
	if err != nil {
		key := activeKey(current.ws, current.sessionID)
		s.mu.Lock()
		if s.active[key] == current {
			delete(s.active, key)
		}
		if q := s.queues[key]; q != nil && len(q.Items) != 0 {
			q.Paused = true
		}
		s.mu.Unlock()
		return nil, err
	}
	return following, nil
}

// queuesFile is where what waits in the queues waits while the server
// restarts: beside the settings; "" without them.
func (s *server) queuesFile() string {
	if s.opt.SettingsFile == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(s.opt.SettingsFile), "queues.json")
}

// savedQueue is a session's queue as queues.json keeps it.
type savedQueue struct {
	Workspace string        `json:"workspace"`
	Session   string        `json:"session"`
	Paused    bool          `json:"paused,omitzero"`
	Prompts   []savedPrompt `json:"prompts"`
}

// savedPrompt is a queued prompt with its images, which the queue's own
// encoding leaves out.
type savedPrompt struct {
	Item   cockpit.QueueItem `json:"item"`
	Images []cockpit.Image   `json:"images,omitzero"`
}

// saveQueues writes what waits in the queues for the server that starts
// next, which takes it up (loadQueues). The caller has the server start no
// runs any more.
func (s *server) saveQueues() error {
	file := s.queuesFile()
	if file == "" {
		return nil
	}
	var saved []savedQueue
	s.mu.Lock()
	for key, q := range s.queues {
		i := strings.LastIndex(key, "/")
		if q == nil || len(q.Items) == 0 || i < 0 {
			continue
		}
		queue := savedQueue{Workspace: key[:i], Session: key[i+1:], Paused: q.Paused}
		for _, item := range q.Items {
			queue.Prompts = append(queue.Prompts, savedPrompt{Item: item, Images: item.Images})
		}
		saved = append(saved, queue)
	}
	s.mu.Unlock()
	if len(saved) == 0 {
		if err := os.Remove(file); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	data, err := json.Marshal(saved)
	if err != nil {
		return err
	}
	temporary := file + ".new"
	if err := os.WriteFile(temporary, data, 0o600); err != nil {
		return err
	}
	return os.Rename(temporary, file)
}

// loadQueues takes up what waited in the queues when the server before this
// one went (saveQueues). The file goes as it is read, so that no later start
// runs its prompts again.
func (s *server) loadQueues() {
	file := s.queuesFile()
	if file == "" {
		return
	}
	data, err := os.ReadFile(file)
	if err != nil {
		if !errors.Is(err, fs.ErrNotExist) {
			log.Printf("queues: %v", err)
		}
		return
	}
	if err := os.Remove(file); err != nil {
		log.Printf("queues: %v", err)
	}
	var saved []savedQueue
	if err := json.Unmarshal(data, &saved); err != nil {
		log.Printf("queues: %s: %v", file, err)
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, queue := range saved {
		ws := s.workspaces.find(queue.Workspace)
		if ws == nil || !cockpit.ValidSessionID(queue.Session) || len(queue.Prompts) == 0 {
			continue
		}
		q := s.queueFor(activeKey(ws, queue.Session))
		for _, prompt := range queue.Prompts {
			item := prompt.Item
			item.Images = prompt.Images
			q.Items = append(q.Items, item)
		}
		q.Paused = queue.Paused
	}
}

// resumeQueues runs the next prompt of each queue that waits for no run: its
// run ended while no server followed it, or while the server started none.
// The forced prompts of a run that ended go back in line, but those the
// runner recorded, which the agent read.
func (s *server) resumeQueues() {
	type waiting struct {
		ws           *workspace
		session, key string
	}
	var forced, ready []waiting
	s.mu.Lock()
	for key, q := range s.queues {
		if s.active[key] != nil || q == nil || len(q.Items) == 0 {
			continue
		}
		i := strings.LastIndex(key, "/")
		ws := s.workspaces.find(key[:max(i, 0)])
		if ws == nil {
			continue
		}
		w := waiting{ws: ws, session: key[i+1:], key: key}
		if q.Forced() != 0 {
			forced = append(forced, w)
		}
		ready = append(ready, w)
	}
	s.mu.Unlock()
	for _, w := range forced {
		tr, err := cockpit.LoadSession(w.ws.opt.SessionDir, w.session)
		s.mu.Lock()
		if q := s.queues[w.key]; q != nil && s.active[w.key] == nil {
			if err == nil {
				q.Delivered(tr)
			}
			q.Settle("done")
		}
		s.mu.Unlock()
	}
	for _, w := range ready {
		s.mu.Lock()
		var item cockpit.QueueItem
		ok := false
		if q := s.queues[w.key]; q != nil && s.active[w.key] == nil && !s.closed {
			item, ok = q.Next()
		}
		s.mu.Unlock()
		if !ok {
			continue
		}
		if _, err := s.runNow(s.ctx, w.ws, w.session, item); err != nil {
			// A prompt that cannot start waits first in line; the queue
			// pauses, unless another run took the session, which runs it next.
			var busy *busyError
			s.mu.Lock()
			q := s.queueFor(w.key)
			q.Insert(0, item)
			q.Paused = !errors.As(err, &busy)
			s.mu.Unlock()
			log.Printf("session %s: the next queued prompt did not start: %v", w.session, err)
		}
	}
}

// quiesce has the server start no runs and no queued prompts any more, so
// that it can take a new build up, unless that would cut a run short: one
// that starts or ends, which the server handles, or one whose runner runs
// attached to it. reopen undoes it, when the new build cannot be taken up.
func (s *server) quiesce() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.busyLocked() {
		return false
	}
	s.closed = true
	return true
}

// reopen undoes quiesce.
func (s *server) reopen() {
	s.mu.Lock()
	s.closed = false
	s.mu.Unlock()
	if file := s.queuesFile(); file != "" {
		if err := os.Remove(file); err != nil && !errors.Is(err, fs.ErrNotExist) {
			log.Printf("queues: %v", err)
		}
	}
	// Runs that ended meanwhile started no queued prompt.
	go s.resumeQueues()
}
