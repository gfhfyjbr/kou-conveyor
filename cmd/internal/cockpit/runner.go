package cockpit

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// ThinkingLevels are the reasoning efforts the runner accepts, lowest first.
var ThinkingLevels = []string{"low", "medium", "high", "xhigh", "max"}

// ValidThinkingLevel reports whether the runner accepts level.
func ValidThinkingLevel(level string) bool {
	for _, known := range ThinkingLevels {
		if level == known {
			return true
		}
	}
	return false
}

// LocateRunner resolves the runner executable: an explicit path, then
// KOU_CONVEYOR_RUNNER, then a sibling of the current executable (so a build
// never mixes versions), then PATH, then ./bin. Under `go run` the sibling is
// built from the same source tree as the cockpit.
func LocateRunner(explicit string) (string, error) {
	resolve := func(path string) (string, error) {
		p, err := exec.LookPath(path)
		if err != nil {
			return "", err
		}
		return filepath.Abs(p)
	}
	if explicit != "" {
		p, err := resolve(explicit)
		if err != nil {
			return "", fmt.Errorf("runner %q: %w", explicit, err)
		}
		return p, nil
	}
	if env := os.Getenv("KOU_CONVEYOR_RUNNER"); env != "" {
		return LocateRunner(env)
	}
	if exe, err := os.Executable(); err == nil {
		sibling := filepath.Join(filepath.Dir(exe), "kou-conveyor-runner")
		if p, err := resolve(sibling); err == nil {
			return p, nil
		}
		// go run leaves the cockpit alone in a throwaway directory, and a
		// runner from PATH may be any older build. Build one from the same
		// sources there instead; go run removes it with the cockpit.
		if source := goRunSource(exe); source != "" {
			if err := buildRunner(source, sibling); err != nil {
				return "", err
			}
			return sibling, nil
		}
	}
	if p, err := resolve("kou-conveyor-runner"); err == nil {
		return p, nil
	}
	if p, err := resolve(filepath.Join(".", "bin", "kou-conveyor-runner")); err == nil {
		return p, nil
	}
	return "", errors.New("cannot find kou-conveyor-runner; run `make build` or pass -runner /path/to/kou-conveyor-runner")
}

// goRunSource returns the module a `go run` build of the cockpit came from,
// or "" for any other build. Such builds keep absolute source paths.
func goRunSource(exe string) string {
	dir := filepath.Dir(exe)
	if filepath.Base(dir) != "exe" || !strings.Contains(dir, string(filepath.Separator)+"go-build") {
		return ""
	}
	_, file, _, ok := runtime.Caller(0)
	if !ok || !filepath.IsAbs(file) {
		return ""
	}
	root := filepath.Clean(filepath.Join(filepath.Dir(file), "..", "..", ".."))
	if _, err := os.Stat(filepath.Join(root, "cmd", "kou-conveyor-runner", "main.go")); err != nil {
		return ""
	}
	return root
}

func buildRunner(root, target string) error {
	goTool, err := exec.LookPath("go")
	if err != nil {
		return fmt.Errorf("build the runner: %w", err)
	}
	fmt.Fprintf(os.Stderr, "building kou-conveyor-runner from %s…\n", root)
	cmd := exec.Command(goTool, "build", "-o", target, "./cmd/kou-conveyor-runner")
	cmd.Dir = root
	cmd.Stdout, cmd.Stderr = os.Stderr, os.Stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("build the runner from %s: %w", root, err)
	}
	return nil
}

// Every runner release has these providers; others need a runner at least as
// new as the cockpit asking for them.
var baseProviders = []string{"ollama", "openai", "openai-codex", "openrouter", "fireworks"}

// ErrRunnerOutdated reports a runner that predates a provider the cockpit
// asks for.
var ErrRunnerOutdated = errors.New("the runner is older than this cockpit")

var runnerProviders = struct {
	sync.Mutex
	byBinary map[string][]string
}{byBinary: map[string][]string{}}

// checkRunner makes sure the runner knows the provider a run will ask for,
// so an outdated runner is named as the problem instead of failing the run.
func checkRunner(runner, provider string) error {
	if provider == "" || slices.Contains(baseProviders, provider) {
		return nil
	}
	info, err := os.Stat(runner)
	if err != nil {
		return nil // Start reports a missing runner
	}
	key := fmt.Sprintf("%s\x00%d\x00%d", runner, info.Size(), info.ModTime().UnixNano())
	runnerProviders.Lock()
	providers, known := runnerProviders.byBinary[key]
	runnerProviders.Unlock()
	if !known {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		var stderr bytes.Buffer
		cmd := exec.CommandContext(ctx, runner, "-providers")
		cmd.Stderr = &stderr
		out, err := cmd.Output()
		switch {
		case err == nil:
			providers = strings.Fields(string(out))
		case strings.Contains(stderr.String(), "flag provided but not defined"):
			providers = baseProviders // from before runners could say
		default:
			return nil // cannot tell; the run reports whatever is wrong
		}
		runnerProviders.Lock()
		runnerProviders.byBinary[key] = providers
		runnerProviders.Unlock()
	}
	if slices.Contains(providers, provider) {
		return nil
	}
	return fmt.Errorf("%w: %s has no %q provider; rebuild it with `make build` or `go install ./cmd/kou-conveyor-runner`, or pass -runner",
		ErrRunnerOutdated, runner, provider)
}

// Options configure how front-ends launch the runner.
type Options struct {
	Runner     string
	Workspace  string
	SessionDir string
	LogDir     string
	// Provider selects a runner provider configured by the environment,
	// which takes precedence over saved settings.
	Provider string
	// SettingsFile holds the saved connection, read at every launch so
	// changes apply to the next run.
	SettingsFile string
	Heartbeat    time.Duration
}

// Request is one prompt for the runner.
type Request struct {
	SessionID string
	// Resume says the session must already exist. A missing one was deleted,
	// and running would start it over under the same ID without its history.
	Resume bool
	// MessageID is the UUID the runner persists the prompt under; front-ends
	// use it to match their pending entry with the persisted input.
	MessageID string
	Prompt    string
	// Model is the model the prompt runs with, "" for the connection's. A
	// session may use another for each prompt, of any provider the
	// connection reaches.
	Model    string
	Thinking string
	// Rewind is the message ID of a prompt this one replaces: under the
	// session's lock, before the runner starts, the session goes back to how
	// it was before that prompt was sent (see RewindSession). The session
	// must exist.
	Rewind string
	// Compact summarizes the session's conversation, which frees the context
	// it takes, instead of running a prompt; Instructions tell the summary
	// what to focus on. The session must exist, and MessageID may be empty.
	Compact      bool
	Instructions string
	// Images go with the prompt, which refers to each by its label.
	Images []Image
}

// Line is one line of runner output.
type Line struct {
	Text   string
	Stderr bool
}

// Job is one runner process.
type Job struct {
	lines  chan Line
	done   chan struct{}
	cancel context.CancelFunc
	err    error

	// steer takes the messages Steer hands to the runner's stdin; nil when
	// the runner cannot take any. fed is closed once stdin takes no more.
	steer chan []byte
	fed   chan struct{}
}

// ErrCannotSteer reports a runner that takes no messages while it runs: one
// built before it could, or a run that has ended.
var ErrCannotSteer = errors.New("the runner cannot take messages while it runs")

// CanSteer reports whether Steer can hand the runner messages.
func (j *Job) CanSteer() bool {
	if j.steer == nil {
		return false
	}
	// A runner that exited takes nothing, even before feed has noticed.
	select {
	case <-j.fed:
		return false
	case <-j.done:
		return false
	default:
		return true
	}
}

// Steer hands the running agent a message, which it reads once the tool
// calls it is making finish: after their results, and never in the middle of
// a response. The runner records it under messageID when it goes out, which
// is how front-ends learn that it did; one the run ends before is never
// recorded. The images its text refers to go with it. Steer does not block.
func (j *Job) Steer(messageID, text string, images ...Image) error {
	if !j.CanSteer() {
		return ErrCannotSteer
	}
	if _, err := uuid.Parse(messageID); err != nil {
		return fmt.Errorf("invalid message ID %q", messageID)
	}
	line, err := json.Marshal(struct {
		Content   string  `json:"content"`
		MessageID string  `json:"message_id"`
		Images    []Image `json:"images,omitempty"`
	}{text, messageID, Referenced(text, images)})
	if err != nil {
		return err
	}
	select {
	case j.steer <- append(line, '\n'):
		return nil
	case <-j.fed:
		return ErrCannotSteer
	default:
		return errors.New("the runner is not reading its messages")
	}
}

// feed writes the request to the runner's stdin, then the messages Steer
// hands over, until the runner stops reading.
func (j *Job) feed(stdin io.WriteCloser, request []byte) {
	defer close(j.fed)
	defer stdin.Close()
	if _, err := stdin.Write(append(request, '\n')); err != nil {
		return // the runner is gone; Wait says why
	}
	for {
		select {
		case line := <-j.steer:
			if _, err := stdin.Write(line); err != nil {
				return
			}
		case <-j.done:
			return
		}
	}
}

var runnerSteering = struct {
	sync.Mutex
	byBinary map[string]bool
}{byBinary: map[string]bool{}}

// runnerSteers reports whether the runner takes messages while it runs
// (-steer). One built before could not, and reads its request up to the end
// of stdin; its runs then take messages only after they end.
func runnerSteers(runner string) bool {
	info, err := os.Stat(runner)
	if err != nil {
		return false
	}
	key := fmt.Sprintf("%s\x00%d\x00%d", runner, info.Size(), info.ModTime().UnixNano())
	runnerSteering.Lock()
	steers, known := runnerSteering.byBinary[key]
	runnerSteering.Unlock()
	if known {
		return steers
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	var usage bytes.Buffer
	cmd := exec.CommandContext(ctx, runner, "-h")
	cmd.Stdout, cmd.Stderr = &usage, &usage
	if err := cmd.Run(); err != nil && ctx.Err() != nil {
		return false // cannot tell; try again next time
	}
	steers = bytes.Contains(usage.Bytes(), []byte("-steer"))
	runnerSteering.Lock()
	runnerSteering.byBinary[key] = steers
	runnerSteering.Unlock()
	return steers
}

// Lines delivers the runner's output in order and is closed after the runner
// has exited and all of its output has been delivered.
func (j *Job) Lines() <-chan Line { return j.lines }

// Done is closed once the runner has exited.
func (j *Job) Done() <-chan struct{} { return j.done }

// Cancel interrupts the runner. It is safe to call more than once.
func (j *Job) Cancel() { j.cancel() }

// Err reports how the runner ended once Done is closed: nil on success and
// context.Canceled when the job or its parent context was canceled.
func (j *Job) Err() error {
	<-j.done
	return j.err
}

// ErrSessionBusy reports that another run holds the session.
var ErrSessionBusy = errors.New("session is in use by another run")

// ErrSessionGone reports a session that was deleted while it was open.
var ErrSessionGone = errors.New("the session was deleted")

const (
	// A prompt's images come back in the line that records it.
	maxLineBytes = 256 << 20
	// An interrupted runner terminates its tools before exiting: SIGTERM, a
	// five-second grace period, then SIGKILL. Only a runner that overstays
	// that is killed outright.
	stopTimeout = 15 * time.Second
)

// Start launches the runner for one request. The session stays locked until
// the runner exits, so no other front-end can append to it concurrently.
// Output is delivered on Lines until ctx is done; after that it is discarded
// so an abandoned runner can always exit.
func Start(ctx context.Context, o Options, r Request) (*Job, error) {
	if !ValidSessionID(r.SessionID) {
		return nil, fmt.Errorf("invalid session ID %q", r.SessionID)
	}
	switch _, err := uuid.Parse(r.MessageID); {
	case r.Compact && (r.Prompt != "" || r.Rewind != ""):
		return nil, errors.New("a compaction runs no prompt")
	case err != nil && !(r.Compact && r.MessageID == ""):
		return nil, fmt.Errorf("invalid message ID %q", r.MessageID)
	}
	if r.Thinking != "" && !ValidThinkingLevel(r.Thinking) {
		return nil, fmt.Errorf("invalid thinking level %q", r.Thinking)
	}
	r.Model = strings.TrimSpace(r.Model)
	if err := ValidModel(r.Model); err != nil {
		return nil, err
	}
	type message struct {
		Content   string  `json:"content"`
		MessageID string  `json:"message_id"`
		Images    []Image `json:"images,omitempty"`
	}
	payload := struct {
		Messages     []message `json:"messages,omitempty"`
		SessionID    string    `json:"session_id"`
		Model        string    `json:"model,omitempty"`
		Thinking     string    `json:"thinking_level,omitempty"`
		Compact      bool      `json:"compact,omitempty"`
		Instructions string    `json:"compact_instructions,omitempty"`
	}{SessionID: r.SessionID, Model: r.Model, Thinking: r.Thinking}
	if r.Compact {
		payload.Compact, payload.Instructions = true, strings.TrimSpace(r.Instructions)
	} else {
		payload.Messages = []message{{r.Prompt, r.MessageID, Referenced(r.Prompt, r.Images)}}
	}
	// stdin keeps prompts out of process listings and argv size limits.
	request, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	env := os.Environ()
	provider := o.Provider
	if o.Provider != "" {
		env = setEnv(env, providerVariable, o.Provider)
	} else if o.SettingsFile != "" {
		settings, err := LoadSettings(o.SettingsFile)
		if err != nil {
			return nil, fmt.Errorf("%w; fix them in the connection settings", err)
		}
		// The prompt's model may be another than the settings', of another
		// provider; through the gateway, the run speaks the API it takes.
		settings = settings.ForModel(r.Model)
		// A run through the gateway goes where the gateway listens now.
		if settings, err = ResolveGateway(ctx, o.SettingsFile, settings); err != nil {
			return nil, err
		}
		env = settings.Environment(env)
		provider = apiDefaults[settings.API].Provider
	}
	if provider == "" {
		provider = WorkspaceEnv(o.Workspace)(providerVariable)
	}
	// The runner finds the user's plugins, and the workspaces trusted with
	// theirs, beside the cockpit's settings.
	if o.SettingsFile != "" {
		env = setEnv(env, plugin.ConfigEnvironment, o.SettingsFile)
	}
	if err := checkRunner(o.Runner, provider); err != nil {
		return nil, err
	}
	unlock, err := LockSession(o.SessionDir, r.SessionID)
	if err != nil {
		return nil, err
	}
	// Checked under the lock, which deleting holds too.
	if _, err := os.Stat(SessionPath(o.SessionDir, r.SessionID)); (r.Resume || r.Rewind != "" || r.Compact) && errors.Is(err, fs.ErrNotExist) {
		unlock()
		return nil, ErrSessionGone
	}
	if r.Rewind != "" {
		if err := rewindSession(o.SessionDir, r.SessionID, r.Rewind); err != nil {
			unlock()
			return nil, err
		}
	}

	runContext, cancel := context.WithCancel(ctx)
	args := []string{"-workspace", o.Workspace, "-session-directory", o.SessionDir, "-tool-heartbeat-interval", o.Heartbeat.String()}
	if o.LogDir != "" {
		args = append(args, "-log-directory", o.LogDir)
	}
	// A compaction takes no messages: what is sent during it runs after it.
	steers := !r.Compact && runnerSteers(o.Runner)
	if steers {
		args = append(args, "-steer")
	}
	cmd := exec.CommandContext(runContext, o.Runner, args...)
	cmd.Dir = o.Workspace
	cmd.Env = env
	var stdin io.WriteCloser
	if steers {
		// stdin stays open for the messages sent while the run goes on.
		if stdin, err = cmd.StdinPipe(); err != nil {
			cancel()
			unlock()
			return nil, fmt.Errorf("start runner: %w", err)
		}
	} else {
		cmd.Stdin = bytes.NewReader(request)
	}
	configureProcess(cmd)
	cmd.WaitDelay = stopTimeout

	job := &Job{lines: make(chan Line, 64), done: make(chan struct{}), cancel: cancel, fed: make(chan struct{})}
	emit := func(line Line) {
		select {
		case job.lines <- line:
		case <-ctx.Done():
		}
	}
	stdout, stderr := &lineWriter{emit: emit}, &lineWriter{emit: emit, stderr: true}
	cmd.Stdout, cmd.Stderr = stdout, stderr
	if err := cmd.Start(); err != nil {
		cancel()
		unlock()
		return nil, fmt.Errorf("start runner: %w", err)
	}
	if steers {
		job.steer = make(chan []byte, 16)
		go job.feed(stdin, request)
	} else {
		close(job.fed)
	}
	go func() {
		err := cmd.Wait() // Wait drains both writers before returning.
		stdout.flush()
		stderr.flush()
		unlock()
		if runContext.Err() != nil {
			err = context.Canceled
		}
		cancel()
		job.err = err
		close(job.lines)
		close(job.done)
	}()
	return job, nil
}

func setEnv(env []string, name, value string) []string {
	result := make([]string, 0, len(env)+1)
	for _, entry := range env {
		if !strings.HasPrefix(entry, name+"=") {
			result = append(result, entry)
		}
	}
	return append(result, name+"="+value)
}

// lineWriter splits process output into lines. os/exec serializes writes to
// each writer; stdout and stderr have separate writers.
type lineWriter struct {
	buf    []byte
	stderr bool
	emit   func(Line)
}

func (w *lineWriter) Write(p []byte) (int, error) {
	n := len(p)
	for len(p) > 0 {
		i := bytes.IndexByte(p, '\n')
		part := p
		if i >= 0 {
			part = p[:i]
		}
		if len(w.buf)+len(part) > maxLineBytes {
			return 0, fmt.Errorf("runner line exceeds %d MiB", maxLineBytes>>20)
		}
		w.buf = append(w.buf, part...)
		if i < 0 {
			break
		}
		w.flush()
		p = p[i+1:]
	}
	return n, nil
}

func (w *lineWriter) flush() {
	if len(w.buf) > 0 {
		w.emit(Line{Text: string(w.buf), Stderr: w.stderr})
		w.buf = w.buf[:0]
	}
}
