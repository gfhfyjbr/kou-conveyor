package cockpit

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

// Detached runs. A runner started with Request.Detached runs apart from the
// front-end that started it: its stdin, stdout and stderr are files in the
// run's directory, it holds the session's lock itself, and it leads a
// session of its own. It goes on when the front-end exits, is restarted or
// crashes; a front-end follows it by reading its files, steers and stops it
// by appending to its input, and takes up a run that an earlier front-end
// left with Adopt.
//
// The run's directory holds:
//   - in.jsonl: the request, then the messages for the agent and the stop
//     line, which the runner reads as they are appended (-detached);
//   - out.jsonl: what the runner writes on stdout, {"type":"exit","code":N}
//     last;
//   - err.log: what it writes on stderr;
//   - job.json: the process, and where the session stood (DetachedRun);
//   - follow.lock: locked by the front-end that follows the run, so that no
//     two follow it.
const (
	runInput  = "in.jsonl"
	runOutput = "out.jsonl"
	runErrors = "err.log"
	runInfo   = "job.json"
	runFollow = "follow.lock"
)

// followPoll is how often a detached run's files are read for what the
// runner wrote since.
const followPoll = 50 * time.Millisecond

// gonePoll is how often a detached run's follower looks whether the runner
// is still there, once its output is read.
const gonePoll = 500 * time.Millisecond

// exitWait is how long a runner that said how it ended may take to exit.
const exitWait = 10 * time.Second

// waited are the PIDs of the runners this process started, which it waits
// for itself (startDetached): detachedGone leaves them to that wait, and so
// does it a process that has one's PID later.
var waited sync.Map

// ErrReleased is how the Job of a detached run ends when the context it was
// started or adopted in ends: the front-end let go of the run, which goes on
// for a front-end to take up (Adopt).
var ErrReleased = errors.New("the front-end let go of the run, which goes on")

// ErrNotDetached reports a directory without a detached run in it: its
// runner never started.
var ErrNotDetached = errors.New("no detached run in the directory")

// ErrFollowed reports a detached run that another front-end follows, in
// this process or another.
var ErrFollowed = errors.New("another front-end follows the run")

// DetachedRun is what a detached run's directory records of it (job.json).
type DetachedRun struct {
	PID        int    `json:"pid"`
	SessionID  string `json:"session_id"`
	SessionDir string `json:"session_dir"`
	// SessionSize is how long the session's file was when the runner
	// started: the run's output follows the session as it was then
	// (LoadSessionPrefix).
	SessionSize int64  `json:"session_size"`
	Workspace   string `json:"workspace"`
	Runner      string `json:"runner"`
	// Build is the runner's build when it started (RunnerBuild).
	Build string `json:"build,omitempty"`
	// Steers says the runner takes messages while it runs, Stops that it
	// takes the line that stops it, Links that messages link files.
	Steers  bool      `json:"steers,omitzero"`
	Stops   bool      `json:"stops,omitzero"`
	Links   bool      `json:"links,omitzero"`
	Started time.Time `json:"started"`
}

// Outdated reports a run whose runner was replaced by another build since
// it started: a rebuild that changed the runner. It goes on as it was built.
func (d DetachedRun) Outdated() bool {
	if d.Build == "" {
		return false
	}
	build := RunnerBuild(d.Runner)
	return build != "" && build != d.Build
}

// ReadDetachedRun reads what a detached run's directory records of it.
func ReadDetachedRun(dir string) (DetachedRun, error) {
	data, err := os.ReadFile(filepath.Join(dir, runInfo))
	if errors.Is(err, fs.ErrNotExist) {
		return DetachedRun{}, ErrNotDetached
	}
	if err != nil {
		return DetachedRun{}, err
	}
	var info DetachedRun
	if err := json.Unmarshal(data, &info); err != nil {
		return DetachedRun{}, fmt.Errorf("%s: %w", filepath.Join(dir, runInfo), err)
	}
	if info.PID <= 0 || !ValidSessionID(info.SessionID) || info.SessionDir == "" {
		return DetachedRun{}, fmt.Errorf("%s: no process or session", filepath.Join(dir, runInfo))
	}
	return info, nil
}

// runnerBuilds are the builds of the runners read, by path, with the file
// each was read from.
var runnerBuilds = struct {
	sync.Mutex
	byPath map[string]runnerBuild
}{byPath: map[string]runnerBuild{}}

type runnerBuild struct {
	file  fs.FileInfo
	build string
}

// RunnerBuild identifies the build of the runner at path: a digest of the
// executable, which a rebuild that leaves the runner as it was does not
// change. "" when it cannot be read.
func RunnerBuild(path string) string {
	info, err := os.Stat(path)
	if err != nil {
		return ""
	}
	runnerBuilds.Lock()
	known, ok := runnerBuilds.byPath[path]
	runnerBuilds.Unlock()
	// The file read before, unchanged: an executable put in its place is
	// another file, even when its size and time are the same — the time
	// goes by ticks of the clock.
	if ok && os.SameFile(known.file, info) && known.file.Size() == info.Size() && known.file.ModTime().Equal(info.ModTime()) {
		return known.build
	}
	file, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer file.Close()
	digest := sha256.New()
	if _, err := io.Copy(digest, file); err != nil {
		return ""
	}
	build := hex.EncodeToString(digest.Sum(nil))[:16]
	runnerBuilds.Lock()
	runnerBuilds.byPath[path] = runnerBuild{file: info, build: build}
	runnerBuilds.Unlock()
	return build
}

// detached is what the Job of a detached run knows of it.
type detached struct {
	dir  string
	info DetachedRun
	// follow holds the run's follower lock until the Job ends.
	follow *os.File
	// input serializes what is appended to in.jsonl.
	input sync.Mutex
}

// append adds a line to the runner's input.
func (d *detached) append(line []byte) error {
	d.input.Lock()
	defer d.input.Unlock()
	file, err := os.OpenFile(filepath.Join(d.dir, runInput), os.O_WRONLY|os.O_APPEND, 0)
	if err != nil {
		return err
	}
	if _, err := file.Write(line); err != nil {
		file.Close()
		return err
	}
	return file.Close()
}

// Detached reports the directory of a detached run and what it records of
// the run; ok is false for an attached one.
func (j *Job) Detached() (dir string, info DetachedRun, ok bool) {
	if j.detached == nil {
		return "", DetachedRun{}, false
	}
	return j.detached.dir, j.detached.info, true
}

// detachedStart is what Start hands startDetached.
type detachedStart struct {
	request []byte
	args    []string
	env     []string
	// lock holds the session's lock, which goes to the runner.
	lock                 *os.File
	steers, stops, links bool
}

// startDetached starts the runner of Start's request apart from the
// front-end, in r.Detached. The session is locked, and s.lock is closed once
// the runner holds the lock; on an error it is left to the caller.
func startDetached(ctx context.Context, o Options, r Request, s detachedStart) (*Job, error) {
	dir := r.Detached
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("start runner: %w", err)
	}
	follow, err := lockFollow(filepath.Join(dir, runFollow))
	if err != nil {
		return nil, fmt.Errorf("start runner: %w", err)
	}
	fail := func(err error) (*Job, error) {
		follow.Close()
		return nil, fmt.Errorf("start runner: %w", err)
	}
	if err := os.WriteFile(filepath.Join(dir, runInput), append(s.request, '\n'), 0o600); err != nil {
		return fail(err)
	}
	// The runner reads and writes the files itself: they go on without the
	// front-end.
	open := func(name string, flag int) (*os.File, error) {
		return os.OpenFile(filepath.Join(dir, name), flag, 0o600)
	}
	stdin, err := open(runInput, os.O_RDONLY)
	if err != nil {
		return fail(err)
	}
	defer stdin.Close()
	stdout, err := open(runOutput, os.O_WRONLY|os.O_CREATE|os.O_TRUNC|os.O_APPEND)
	if err != nil {
		return fail(err)
	}
	defer stdout.Close()
	stderr, err := open(runErrors, os.O_WRONLY|os.O_CREATE|os.O_TRUNC|os.O_APPEND)
	if err != nil {
		return fail(err)
	}
	defer stderr.Close()
	// The run's output follows the session as it is now, after a rewind.
	var size int64
	if info, err := os.Stat(SessionPath(o.SessionDir, r.SessionID)); err == nil {
		size = info.Size()
	}
	build := RunnerBuild(o.Runner)
	cmd := spawn.Command(o.Runner, s.args...)
	cmd.Dir, cmd.Env = o.Workspace, s.env
	cmd.Stdin, cmd.Stdout, cmd.Stderr = stdin, stdout, stderr
	if s.lock != nil {
		cmd.ExtraFiles = []*os.File{s.lock} // the runner's file descriptor 3
	}
	detachProcess(cmd.Cmd)
	if err := cmd.Start(); err != nil {
		return fail(err)
	}
	waited.Store(cmd.Process.Pid, true)
	// The runner holds the lock now. Closing this copy leaves it locked;
	// unlocking would release the runner's too.
	if s.lock != nil {
		s.lock.Close()
	}
	info := DetachedRun{
		PID: cmd.Process.Pid, SessionID: r.SessionID, SessionDir: o.SessionDir, SessionSize: size,
		Workspace: o.Workspace, Runner: o.Runner, Build: build,
		Steers: s.steers, Stops: s.stops, Links: s.links, Started: time.Now().UTC(),
	}
	// Without it the run cannot be taken up later; it goes on regardless.
	_ = writeDetachedRun(dir, info)
	// The front-end that started the runner is its parent, which learns
	// how it ended.
	exited := make(chan struct{})
	var status, waitErr error
	go func() {
		state, err := cmd.Process.Wait()
		if waitErr = err; err == nil {
			if state.Exited() {
				status = exitStatus(state.ExitCode())
			} else {
				status = errors.New(state.String())
			}
		}
		close(exited)
	}()
	return followDetached(ctx, dir, info, follow, func() (bool, error, bool) {
		select {
		case <-exited:
			return true, status, waitErr == nil
		default:
			return false, nil, false
		}
	}), nil
}

func writeDetachedRun(dir string, info DetachedRun) error {
	data, err := json.Marshal(info)
	if err != nil {
		return err
	}
	temp := filepath.Join(dir, "."+runInfo+".new")
	if err := os.WriteFile(temp, append(data, '\n'), 0o600); err != nil {
		return err
	}
	return os.Rename(temp, filepath.Join(dir, runInfo))
}

// Adopt takes up a detached run that an earlier front-end started, from its
// directory: the Job delivers the run's output from its start, as Start's
// does, steers and stops it the same way, and lets go of it again when ctx
// ends. A run that ended while no front-end followed it delivers all it
// wrote, and ends as it did. A run another front-end follows is
// ErrFollowed.
func Adopt(ctx context.Context, dir string) (*Job, DetachedRun, error) {
	info, err := ReadDetachedRun(dir)
	if err != nil {
		return nil, DetachedRun{}, err
	}
	follow, err := lockFollow(filepath.Join(dir, runFollow))
	if err != nil {
		return nil, info, err
	}
	job := followDetached(ctx, dir, info, follow, func() (bool, error, bool) {
		return detachedGone(info.PID, info.SessionDir, info.SessionID)
	})
	return job, info, nil
}

// StopDetached stops a detached run that no front-end follows, one a
// front-end let go of, as Stop stops one it follows: it tells the runner
// why, interrupts it, and kills its process group when it is still there
// after stopTimeout. It returns once the runner is gone, or with ctx's error
// when ctx ends first. A run whose runner is gone already is left as it is.
func StopDetached(ctx context.Context, dir, reason string) error {
	info, err := ReadDetachedRun(dir)
	if err != nil {
		return err
	}
	running := func() bool {
		gone, _, _ := detachedGone(info.PID, info.SessionDir, info.SessionID)
		return !gone
	}
	if !running() {
		return nil
	}
	if info.Stops {
		d := &detached{dir: dir, info: info}
		if err := d.append(stopLine(reason)); err != nil {
			return err
		}
	}
	if err := interruptGroup(info.PID); err != nil && running() {
		return err
	}
	kill := time.NewTimer(stopTimeout)
	defer kill.Stop()
	tick := time.NewTicker(followPoll)
	defer tick.Stop()
	for running() {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-kill.C:
			_ = killGroup(info.PID)
		case <-tick.C:
		}
	}
	return nil
}

// stopLine is the line of a runner's input that stops it and says why.
func stopLine(reason string) []byte {
	if reason = strings.Join(strings.Fields(reason), " "); reason == "" {
		reason = "the cockpit stopped the run without saying why"
	}
	type stop struct {
		Reason string `json:"reason"`
	}
	line, err := json.Marshal(struct {
		Stop stop `json:"stop"`
	}{stop{reason}})
	if err != nil {
		panic(err) // a struct of a string always marshals
	}
	return append(line, '\n')
}

// followDetached makes the Job of a detached run, which holds the run's
// follower lock, follow, until it ends. gone says whether the runner has
// exited, and how when known.
func followDetached(ctx context.Context, dir string, info DetachedRun, follow *os.File, gone func() (bool, error, bool)) *Job {
	// Stop and Cancel stop the run; the front-end's context ending does not.
	runContext, cancelCause := context.WithCancelCause(context.Background())
	d := &detached{dir: dir, info: info, follow: follow}
	job := &Job{
		lines: make(chan Line, 64), done: make(chan struct{}), cancel: cancelCause, fed: make(chan struct{}),
		workspace: info.Workspace, links: info.Links, detached: d,
	}
	if info.Steers {
		job.steer = make(chan []byte, 16)
		go func() {
			defer close(job.fed)
			for {
				select {
				case line := <-job.steer:
					if d.append(line) != nil {
						return
					}
				case <-job.done:
					return
				}
			}
		}()
	} else {
		close(job.fed)
	}
	go job.stopDetached(runContext)
	go job.follow(ctx, runContext, cancelCause, gone)
	return job
}

// stopDetached stops the runner once Stop or Cancel is called: it appends
// the line that says why to the runner's input, when the runner takes one,
// interrupts the runner's process group, and kills the group if the runner
// is still there after stopTimeout.
func (j *Job) stopDetached(run context.Context) {
	select {
	case <-run.Done():
	case <-j.done:
		return
	}
	select {
	case <-j.done:
		return // it ended, or the front-end let go of it
	default:
	}
	d := j.detached
	if d.info.Stops {
		reason := ""
		var given stopReason
		if errors.As(context.Cause(run), &given) {
			reason = string(given)
		}
		_ = d.append(stopLine(reason))
	}
	_ = interruptGroup(d.info.PID)
	select {
	case <-j.done:
	case <-time.After(stopTimeout):
		_ = killGroup(d.info.PID)
	}
}

// follow delivers what the runner writes, as it writes it, until the runner
// has exited and all of it is delivered; or until ctx ends, when the Job
// lets go of the run (ErrReleased), which goes on.
func (j *Job) follow(ctx, run context.Context, cancel context.CancelCauseFunc, gone func() (bool, error, bool)) {
	d := j.detached
	out := &tailFile{path: filepath.Join(d.dir, runOutput)}
	errs := &tailFile{path: filepath.Join(d.dir, runErrors)}
	defer out.close()
	defer errs.close()
	code := -1 // what the exit line says
	var exitSeen time.Time
	var status error
	known, released := false, false
	emit := func(line Line) bool {
		select {
		case j.lines <- line:
			return true
		case <-ctx.Done():
			released = true
			return false
		}
	}
	lastLook := time.Time{}
	exited := false
	var readErr error
	for !released {
		// What the runner wrote since: stdout, its exit line aside, then
		// stderr.
		if err := out.read(func(line []byte) bool {
			if exit, ok := exitLine(line); ok {
				code = exit
				if exitSeen.IsZero() {
					exitSeen = time.Now()
				}
				return true
			}
			return emit(Line{Text: string(line)})
		}); err != nil && readErr == nil {
			readErr = err
		}
		if released {
			break
		}
		if err := errs.read(func(line []byte) bool { return emit(Line{Text: string(line), Stderr: true}) }); err != nil && readErr == nil {
			readErr = err
		}
		if released || exited {
			break // everything the runner wrote before it exited is read
		}
		if ctx.Err() != nil {
			released = true
			break
		}
		// Whether the runner is still there; the files are read once more
		// after it is gone.
		if code >= 0 || time.Since(lastLook) >= gonePoll {
			lastLook = time.Now()
			if exited, status, known = gone(); exited {
				continue
			}
			if code >= 0 && time.Since(exitSeen) > exitWait {
				exited = true // it said how it ended; whatever it still does is no part of the run
				continue
			}
		}
		select {
		case <-ctx.Done():
			released = true
		case <-time.After(followPoll):
		}
	}
	// Whoever reads the end of the output may take the run up at once.
	if d.follow != nil {
		d.follow.Close()
	}
	if released {
		j.err = ErrReleased
		close(j.lines)
		close(j.done)
		return
	}
	var err error
	switch {
	case code >= 0:
		err = exitStatus(code)
	case known:
		err = status
	default:
		err = errors.New("the runner exited without saying how it ended")
	}
	if readErr != nil && err == nil {
		err = fmt.Errorf("read the runner's output: %w", readErr)
	}
	// A run stopped exits with 130, as the runner does when interrupted.
	if run.Err() != nil || code == 130 {
		err = context.Canceled
	}
	j.err = err
	close(j.lines)
	close(j.done)
	cancel(nil)
}

// exitLine reads the line a detached runner ends its output with.
func exitLine(line []byte) (int, bool) {
	if !bytes.HasPrefix(line, []byte(`{"type":"exit"`)) {
		return 0, false
	}
	var exit struct {
		Type string `json:"type"`
		Code *int   `json:"code"`
	}
	if json.Unmarshal(line, &exit) != nil || exit.Type != "exit" || exit.Code == nil {
		return 0, false
	}
	return *exit.Code, true
}

// exitCode is a runner's exit status other than 0, which says it as
// exec.ExitError does.
type exitCode int

func (c exitCode) Error() string { return fmt.Sprintf("exit status %d", int(c)) }

func exitStatus(code int) error {
	if code == 0 {
		return nil
	}
	return exitCode(code)
}

// tailFile reads a file that grows, a line at a time: the end of a line
// still being written waits for the rest.
type tailFile struct {
	path    string
	file    *os.File
	partial []byte
	buf     []byte
}

// read hands each line written since the last read to each, until each
// returns false.
func (t *tailFile) read(each func(line []byte) bool) error {
	if t.file == nil {
		file, err := os.Open(t.path)
		if errors.Is(err, fs.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		t.file = file
	}
	if t.buf == nil {
		t.buf = make([]byte, 256<<10)
	}
	for {
		n, err := t.file.Read(t.buf)
		data := t.buf[:n]
		for len(data) > 0 {
			i := bytes.IndexByte(data, '\n')
			if i < 0 {
				if len(t.partial)+len(data) > maxLineBytes {
					return fmt.Errorf("runner line exceeds %d MiB", maxLineBytes>>20)
				}
				t.partial = append(t.partial, data...)
				break
			}
			line := data[:i]
			if len(t.partial) > 0 {
				line = append(t.partial, line...)
			}
			data = data[i+1:]
			// Empty lines are no output, as for an attached runner.
			if len(line) > 0 && !each(line) {
				t.partial = t.partial[:0]
				// What follows the line is read again next time.
				if _, err := t.file.Seek(-int64(len(data)), io.SeekCurrent); err != nil {
					return err
				}
				return nil
			}
			t.partial = t.partial[:0]
		}
		if errors.Is(err, io.EOF) || n == 0 {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

func (t *tailFile) close() {
	if t.file != nil {
		t.file.Close()
	}
}
