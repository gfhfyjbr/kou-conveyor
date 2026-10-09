package cockpit_test

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func TestMain(m *testing.M) {
	cockpittest.Main()
	os.Exit(m.Run())
}

func start(t *testing.T, ctx context.Context, sessions, id, prompt string) *cockpit.Job {
	t.Helper()
	job, err := cockpit.Start(ctx, cockpit.Options{
		Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions, Heartbeat: time.Minute,
	}, cockpit.Request{SessionID: id, MessageID: uuid.New().String(), Prompt: prompt, Thinking: "high"})
	if err != nil {
		t.Fatal(err)
	}
	return job
}

// drain folds a job's output into a transcript until the runner exits.
func drain(t *testing.T, job *cockpit.Job, tr *cockpit.Transcript) (stderr []string) {
	t.Helper()
	timeout := time.After(10 * time.Second)
	for {
		select {
		case line, ok := <-job.Lines():
			if !ok {
				return stderr
			}
			if line.Stderr {
				stderr = append(stderr, line.Text)
				continue
			}
			if _, err := tr.Apply([]byte(line.Text)); err != nil {
				t.Fatal(err)
			}
		case <-timeout:
			t.Fatal("runner did not finish")
		}
	}
}

func TestStartRunsAndReleasesSession(t *testing.T) {
	sessions := t.TempDir()
	tr := cockpit.NewTranscript()
	drain(t, start(t, t.Context(), sessions, "session-1", "use a tool"), tr)

	call := tr.Entry("tool:call-1")
	if call == nil || call.Tool.State != cockpit.ToolDone || call.Tool.Output != "hi\n" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	if last := tr.Entries[len(tr.Entries)-1]; last.Text != "echo: use a tool" {
		t.Fatalf("last entry = %#v", last)
	}
	persisted, err := cockpit.LoadSession(sessions, "session-1")
	if err != nil || len(persisted.Entries) != len(tr.Entries) {
		t.Fatalf("persisted = %#v, %v", persisted, err)
	}
	unlock, err := cockpit.LockSession(sessions, "session-1")
	if err != nil {
		t.Fatalf("session still locked after the runner exited: %v", err)
	}
	unlock()
}

func TestStartRefusesBusySession(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("no cross-process session locks on windows")
	}
	sessions := t.TempDir()
	unlock, err := cockpit.LockSession(sessions, "session-1")
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	_, err = cockpit.Start(t.Context(), cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: sessions},
		cockpit.Request{SessionID: "session-1", MessageID: uuid.New().String(), Prompt: "hi"})
	if !errors.Is(err, cockpit.ErrSessionBusy) {
		t.Fatalf("err = %v", err)
	}
}

func TestCancelInterruptsRunner(t *testing.T) {
	job := start(t, t.Context(), t.TempDir(), "session-1", "wait for me")
	// The runner persists the prompt before it blocks; cancel once it is live.
	select {
	case <-job.Lines():
	case <-time.After(10 * time.Second):
		t.Fatal("runner produced no output")
	}
	job.Cancel()
	for range job.Lines() {
	}
	if err := job.Err(); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
}

func TestAbandonedJobStillExits(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	job := start(t, ctx, t.TempDir(), "session-1", "wait for me")
	cancel() // nobody reads Lines any more
	select {
	case <-job.Done():
	case <-time.After(10 * time.Second):
		t.Fatal("abandoned runner kept running")
	}
}

func TestCrashIsReportedOnce(t *testing.T) {
	tr := cockpit.NewTranscript()
	tr.Submit(uuid.New().String(), "crash please", time.Now())
	job := start(t, t.Context(), t.TempDir(), "session-1", "crash please")
	stderr := drain(t, job, tr)
	err := job.Err()
	var exit *exec.ExitError
	if !errors.As(err, &exit) || exit.ExitCode() != 2 || len(stderr) != 1 {
		t.Fatalf("err = %v stderr = %q", err, stderr)
	}
	changed := tr.Finish(err, false, time.Now())
	if len(changed) != 2 || !strings.HasPrefix(changed[1].Text, "Runner exited") {
		t.Fatalf("changed = %#v", changed)
	}
}

func TestLocateRunnerPrefersExplicitPath(t *testing.T) {
	runner := cockpittest.Runner(t)
	if got, err := cockpit.LocateRunner(runner); err != nil || got != runner {
		t.Fatalf("LocateRunner = %q, %v", got, err)
	}
	if _, err := cockpit.LocateRunner(t.TempDir() + "/missing"); err == nil {
		t.Fatal("located a missing runner")
	}
}

func TestSessionBusyProbeNeverBlocksARun(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("no cross-process session locks on windows")
	}
	sessions := t.TempDir()
	if cockpit.SessionBusy(sessions, "session-1") {
		t.Fatal("a session nobody ran is busy")
	}
	unlock, err := cockpit.LockSession(sessions, "session-1")
	if err != nil {
		t.Fatal(err)
	}
	if !cockpit.SessionBusy(sessions, "session-1") {
		t.Fatal("a locked session is not busy")
	}
	unlock()
	// Probes racing with a run's start must not make it fail. Viewers probe
	// every few seconds; a millisecond is already far more often.
	stop := make(chan struct{})
	go func() {
		for {
			select {
			case <-stop:
				return
			case <-time.After(time.Millisecond):
				cockpit.SessionBusy(sessions, "session-1")
			}
		}
	}()
	defer close(stop)
	for range 50 {
		unlock, err := cockpit.LockSession(sessions, "session-1")
		if err != nil {
			t.Fatalf("a probe made a run's lock fail: %v", err)
		}
		unlock()
	}
}

func TestOutdatedRunnerIsNamed(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("uses a shell script as the runner")
	}
	// What a runner from before -providers does with the flag.
	old := filepath.Join(t.TempDir(), "kou-conveyor-runner")
	os.WriteFile(old, []byte("#!/bin/sh\necho 'flag provided but not defined: -providers' >&2\nexit 2\n"), 0o755)
	settings := filepath.Join(t.TempDir(), "settings.json")
	if err := cockpit.SaveSettings(settings, cockpit.Settings{API: cockpit.APIMessages}); err != nil {
		t.Fatal(err)
	}
	o := cockpit.Options{Runner: old, Workspace: t.TempDir(), SessionDir: t.TempDir(), SettingsFile: settings}
	_, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: uuid.New().String(), Prompt: "hi"})
	if !errors.Is(err, cockpit.ErrRunnerOutdated) || !strings.Contains(err.Error(), old) || !strings.Contains(err.Error(), `"anthropic"`) {
		t.Fatalf("err = %v", err)
	}
	// Providers every runner has still start, and a current runner is fine.
	o.SettingsFile, o.Provider = "", "openai"
	if job, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: uuid.New().String(), Prompt: "hi"}); err != nil {
		t.Fatalf("openai on an old runner: %v", err)
	} else {
		for range job.Lines() {
		}
	}
	o.Runner, o.SettingsFile, o.Provider = cockpittest.Runner(t), settings, ""
	if job, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s2", MessageID: uuid.New().String(), Prompt: "hi"}); err != nil {
		t.Fatalf("anthropic on a current runner: %v", err)
	} else {
		for range job.Lines() {
		}
	}
}
