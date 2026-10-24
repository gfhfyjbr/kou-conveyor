//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cockpit_test

import (
	"context"
	"encoding/json/v2"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// startDetached starts a run apart from the test, in a directory of its own
// beside the sessions, as the web cockpit does.
func startDetached(t *testing.T, ctx context.Context, sessions, workspace, id, prompt string) (*cockpit.Job, string) {
	t.Helper()
	dir := filepath.Join(sessions, ".runs", uuid.New().String())
	job, err := cockpit.Start(ctx, cockpit.Options{
		Runner: cockpittest.Runner(t), Workspace: workspace, SessionDir: sessions, Heartbeat: time.Minute,
	}, cockpit.Request{SessionID: id, MessageID: uuid.New().String(), Prompt: prompt, Thinking: "high", Detached: dir})
	if err != nil {
		t.Fatal(err)
	}
	// A runner the test leaves behind goes with it. Only a runner holds the
	// session's lock, which no one else here takes.
	t.Cleanup(func() {
		if info, err := cockpit.ReadDetachedRun(dir); err == nil && cockpit.SessionBusy(sessions, id) {
			syscall.Kill(-info.PID, syscall.SIGKILL)
		}
	})
	return job, dir
}

// collect reads a job's output until the runner exits.
func collect(t *testing.T, job *cockpit.Job) []cockpit.Line {
	t.Helper()
	var lines []cockpit.Line
	timeout := time.After(15 * time.Second)
	for {
		select {
		case line, ok := <-job.Lines():
			if !ok {
				return lines
			}
			lines = append(lines, line)
		case <-timeout:
			t.Fatal("runner did not finish")
		}
	}
}

// transcript folds lines of runner output into a transcript.
func transcript(t *testing.T, lines []cockpit.Line) *cockpit.Transcript {
	t.Helper()
	tr := cockpit.NewTranscript()
	for _, line := range lines {
		if line.Stderr {
			continue
		}
		if _, err := tr.Apply([]byte(line.Text)); err != nil {
			t.Fatalf("%v: %s", err, line.Text)
		}
	}
	return tr
}

// hardStops lists the reasons the hard stops the lines record give.
func hardStops(lines []cockpit.Line) []string {
	var reasons []string
	for _, line := range lines {
		var item sessionstore.Item
		if line.Stderr || json.Unmarshal([]byte(line.Text), &item) != nil {
			continue
		}
		if input, ok := item.Data.(inbox.Input); ok && input.Kind == inbox.InputControl {
			if control, err := input.DecodeControlMessage(); err == nil && control.Mode == inbox.StopHard {
				reasons = append(reasons, control.Reason)
			}
		}
	}
	return reasons
}

// waitUntilFree waits for the runner that holds a session to exit.
func waitUntilFree(t *testing.T, sessions, id string) {
	t.Helper()
	deadline := time.Now().Add(15 * time.Second)
	for cockpit.SessionBusy(sessions, id) {
		if time.Now().After(deadline) {
			t.Fatal("the runner did not exit")
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func expectFree(t *testing.T, sessions, id string) {
	t.Helper()
	unlock, err := cockpit.LockSession(sessions, id)
	if err != nil {
		t.Fatalf("session still locked after the runner exited: %v", err)
	}
	unlock()
}

// A detached run delivers what its runner writes as an attached one does,
// though not the line its runner ends with, says how it ended, and leaves
// the session free once its runner exits.
func TestDetachedRunDeliversItsOutput(t *testing.T) {
	sessions := t.TempDir()
	job, dir := startDetached(t, t.Context(), sessions, t.TempDir(), "session-1", "use a tool")
	got, info, ok := job.Detached()
	if !ok || got != dir || info.PID <= 0 || info.SessionID != "session-1" || info.SessionDir != sessions || info.Build == "" || !info.Steers || !info.Stops {
		t.Fatalf("Detached() = %q, %+v, %v", got, info, ok)
	}
	if recorded, err := cockpit.ReadDetachedRun(dir); err != nil || recorded.PID != info.PID || recorded.Build != info.Build {
		t.Fatalf("job.json = %+v, %v", recorded, err)
	}
	if info.Outdated() {
		t.Fatal("a run of the runner as it is is outdated")
	}
	lines := collect(t, job)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	for _, line := range lines {
		if strings.Contains(line.Text, `"type":"exit"`) {
			t.Fatalf("the runner's exit line was delivered: %s", line.Text)
		}
	}
	tr := transcript(t, lines)
	call := tr.Entry("tool:call-1")
	if call == nil || call.Tool.State != cockpit.ToolDone || call.Tool.Output != "hi\n" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	if last := tr.Entries[len(tr.Entries)-1]; last.Text != "echo: use a tool" {
		t.Fatalf("last entry = %#v", last)
	}
	output, err := os.ReadFile(filepath.Join(dir, "out.jsonl"))
	if err != nil || !strings.HasSuffix(string(output), "{\"type\":\"exit\",\"code\":0}\n") {
		t.Fatalf("out.jsonl ends %q, %v", output[max(0, len(output)-80):], err)
	}
	expectFree(t, sessions, "session-1")
}

// A run that fails says how, as an attached one does.
func TestDetachedRunFails(t *testing.T) {
	sessions := t.TempDir()
	job, _ := startDetached(t, t.Context(), sessions, t.TempDir(), "session-1", "fail")
	collect(t, job)
	if err := job.Err(); err == nil || err.Error() != "exit status 1" {
		t.Fatalf("err = %v, want exit status 1", err)
	}
	expectFree(t, sessions, "session-1")
}

// Messages for the agent go to a detached runner through its input file.
func TestDetachedRunTakesMessages(t *testing.T) {
	job, dir := startDetached(t, t.Context(), t.TempDir(), t.TempDir(), uuid.New().String(), "steer")
	if !job.CanSteer() {
		t.Fatal("the runner takes no messages")
	}
	tr := cockpit.NewTranscript()
	message := uuid.New().String()
	sent := false
	timeout := time.After(15 * time.Second)
	for done := false; !done; {
		select {
		case line, ok := <-job.Lines():
			if !ok {
				done = true
				break
			}
			if line.Stderr {
				continue
			}
			if _, err := tr.Apply([]byte(line.Text)); err != nil {
				t.Fatal(err)
			}
			if running := tr.Running(); !sent && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
				if err := job.Steer(message, "use pnpm"); err != nil {
					t.Fatal(err)
				}
				sent = true
			}
		case <-timeout:
			t.Fatal("runner did not finish")
		}
	}
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	if last := tr.Entries[len(tr.Entries)-1]; last.Text != "steered: use pnpm" || tr.Entry("input:"+message) == nil {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	if job.CanSteer() || !errors.Is(job.Steer(uuid.New().String(), "late"), cockpit.ErrCannotSteer) {
		t.Fatal("a run that ended took a message")
	}
	input, err := os.ReadFile(filepath.Join(dir, "in.jsonl"))
	if err != nil || strings.Count(string(input), "\n") != 2 || !strings.Contains(string(input), message) {
		t.Fatalf("in.jsonl = %q, %v", input, err)
	}
}

// Stop tells a detached runner why, as an attached one, and interrupts it.
func TestStopStopsADetachedRun(t *testing.T) {
	sessions := t.TempDir()
	job, _ := startDetached(t, t.Context(), sessions, t.TempDir(), "session-1", "wait for me")
	select {
	case <-job.Lines():
	case <-time.After(10 * time.Second):
		t.Fatal("runner produced no output")
	}
	job.Stop("the user  stopped\n the run")
	lines := collect(t, job)
	if err := job.Err(); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
	if reasons := hardStops(lines); !slices.Equal(reasons, []string{"the user stopped the run"}) {
		t.Fatalf("recorded stops = %q", reasons)
	}
	expectFree(t, sessions, "session-1")
}

// When the context a detached run was started in ends, the front-end lets
// go of it: the runner goes on, holding the session, and the next front-end
// takes the run up from its directory and follows it from its start to its
// end.
func TestReleasedRunGoesOnForTheNextFrontEnd(t *testing.T) {
	sessions, workspace := t.TempDir(), t.TempDir()
	ctx, release := context.WithCancel(t.Context())
	job, dir := startDetached(t, ctx, sessions, workspace, "session-1", "gate")
	select {
	case <-job.Lines():
	case <-time.After(10 * time.Second):
		t.Fatal("runner produced no output")
	}
	release()
	collect(t, job)
	if err := job.Err(); !errors.Is(err, cockpit.ErrReleased) {
		t.Fatalf("err = %v, want ErrReleased", err)
	}
	_, info, _ := job.Detached()
	if err := syscall.Kill(info.PID, 0); err != nil {
		t.Fatalf("the runner went with the front-end: %v", err)
	}
	if !cockpit.SessionBusy(sessions, "session-1") {
		t.Fatal("the session is free while its runner goes on")
	}
	if _, err := cockpit.LockSession(sessions, "session-1"); !errors.Is(err, cockpit.ErrSessionBusy) {
		t.Fatalf("another run took the session: %v", err)
	}

	adopted, adoptedInfo, err := cockpit.Adopt(t.Context(), dir)
	if err != nil || adoptedInfo.PID != info.PID || !adopted.CanSteer() {
		t.Fatalf("Adopt = %+v, %v", adoptedInfo, err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "gate"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	lines := collect(t, adopted)
	if err := adopted.Err(); err != nil {
		t.Fatal(err)
	}
	tr := transcript(t, lines)
	if first, last := tr.Entries[0], tr.Entries[len(tr.Entries)-1]; first.Kind != cockpit.KindUser || first.Text != "gate" || last.Text != "echo: gate" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	expectFree(t, sessions, "session-1")
}

// A front-end stops a run it took up as it stops one it started.
func TestStopStopsAnAdoptedRun(t *testing.T) {
	sessions := t.TempDir()
	ctx, release := context.WithCancel(t.Context())
	job, dir := startDetached(t, ctx, sessions, t.TempDir(), "session-1", "wait for me")
	select {
	case <-job.Lines():
	case <-time.After(10 * time.Second):
		t.Fatal("runner produced no output")
	}
	release()
	collect(t, job)

	adopted, _, err := cockpit.Adopt(t.Context(), dir)
	if err != nil {
		t.Fatal(err)
	}
	adopted.Stop("the server restarted the run")
	lines := collect(t, adopted)
	if err := adopted.Err(); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
	if reasons := hardStops(lines); !slices.Equal(reasons, []string{"the server restarted the run"}) {
		t.Fatalf("recorded stops = %q", reasons)
	}
	expectFree(t, sessions, "session-1")
}

// A run that ended while no front-end followed it delivers all it wrote to
// the front-end that takes it up, and ends as it did.
func TestAdoptedRunEndsAsItDid(t *testing.T) {
	for _, test := range []struct{ prompt, err string }{
		{prompt: "use a tool"},
		{prompt: "fail", err: "exit status 1"},
	} {
		t.Run(test.prompt, func(t *testing.T) {
			sessions := t.TempDir()
			ctx, release := context.WithCancel(t.Context())
			job, dir := startDetached(t, ctx, sessions, t.TempDir(), "session-1", test.prompt)
			release()
			collect(t, job)
			waitUntilFree(t, sessions, "session-1")

			adopted, _, err := cockpit.Adopt(t.Context(), dir)
			if err != nil {
				t.Fatal(err)
			}
			lines := collect(t, adopted)
			err = adopted.Err()
			if test.err == "" && err != nil || test.err != "" && (err == nil || err.Error() != test.err) {
				t.Fatalf("err = %v, want %q", err, test.err)
			}
			if test.err == "" {
				if tr := transcript(t, lines); tr.Entries[len(tr.Entries)-1].Text != "echo: use a tool" {
					t.Fatalf("entries = %#v", tr.Entries)
				}
			}
		})
	}
}

// A directory no run started in is no run to take up.
func TestAdoptNeedsARun(t *testing.T) {
	if _, _, err := cockpit.Adopt(t.Context(), t.TempDir()); !errors.Is(err, cockpit.ErrNotDetached) {
		t.Fatalf("err = %v, want ErrNotDetached", err)
	}
}

// A runner built before runners ran apart runs attached, as before.
func TestOlderRunnersRunAttached(t *testing.T) {
	t.Setenv(cockpittest.NoDetach, "1")
	cockpit.ForgetRunners()
	t.Cleanup(cockpit.ForgetRunners)
	job, dir := startDetached(t, t.Context(), t.TempDir(), t.TempDir(), "session-1", "hello")
	if _, _, ok := job.Detached(); ok {
		t.Fatal("a runner without -detached ran detached")
	}
	lines := collect(t, job)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	if tr := transcript(t, lines); tr.Entries[len(tr.Entries)-1].Text != "echo: hello" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
	if _, err := cockpit.ReadDetachedRun(dir); !errors.Is(err, cockpit.ErrNotDetached) {
		t.Fatalf("ReadDetachedRun = %v, want ErrNotDetached", err)
	}
}

// A runner that takes no messages reads its request from the input file to
// its end.
func TestDetachedRunWithoutMessages(t *testing.T) {
	t.Setenv(cockpittest.NoSteer, "1")
	cockpit.ForgetRunners()
	t.Cleanup(cockpit.ForgetRunners)
	job, _ := startDetached(t, t.Context(), t.TempDir(), t.TempDir(), "session-1", "hello")
	if _, info, ok := job.Detached(); !ok || info.Steers || job.CanSteer() {
		t.Fatalf("Detached() = %+v, %v", info, ok)
	}
	lines := collect(t, job)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	if tr := transcript(t, lines); tr.Entries[len(tr.Entries)-1].Text != "echo: hello" {
		t.Fatalf("entries = %#v", tr.Entries)
	}
}

// A run is outdated once its runner's executable is another build.
func TestDetachedRunOutdated(t *testing.T) {
	runner := filepath.Join(t.TempDir(), "runner")
	if err := os.WriteFile(runner, []byte("build 1"), 0o700); err != nil {
		t.Fatal(err)
	}
	run := cockpit.DetachedRun{Runner: runner, Build: cockpit.RunnerBuild(runner)}
	if run.Build == "" || run.Outdated() {
		t.Fatalf("build %q, outdated %v", run.Build, run.Outdated())
	}
	// A rebuild replaces the executable.
	next := runner + ".new"
	if err := os.WriteFile(next, []byte("build 2"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(next, runner); err != nil {
		t.Fatal(err)
	}
	if !run.Outdated() {
		t.Fatal("a run of the build before is not outdated")
	}
	if (cockpit.DetachedRun{Runner: runner}).Outdated() {
		t.Fatal("a run of an unknown build is outdated")
	}
}

// One front-end follows a run at a time: no other takes it up while one
// follows it, and one may once it let go. A run that ended is anyone's to
// read.
func TestOneFrontEndFollowsARun(t *testing.T) {
	sessions, workspace := t.TempDir(), t.TempDir()
	ctx, release := context.WithCancel(t.Context())
	job, dir := startDetached(t, ctx, sessions, workspace, "session-1", "gate")
	if _, _, err := cockpit.Adopt(t.Context(), dir); !errors.Is(err, cockpit.ErrFollowed) {
		t.Fatalf("Adopt of a run its starter follows: %v, want ErrFollowed", err)
	}
	release()
	collect(t, job)
	adopted, _, err := cockpit.Adopt(t.Context(), dir)
	if err != nil {
		t.Fatalf("Adopt of a run let go of: %v", err)
	}
	if _, _, err := cockpit.Adopt(t.Context(), dir); !errors.Is(err, cockpit.ErrFollowed) {
		t.Fatalf("Adopt of a run taken up: %v, want ErrFollowed", err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "gate"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	collect(t, adopted)
	if err := adopted.Err(); err != nil {
		t.Fatal(err)
	}
	again, _, err := cockpit.Adopt(t.Context(), dir)
	if err != nil {
		t.Fatalf("Adopt of a run that ended: %v", err)
	}
	if tr := transcript(t, collect(t, again)); again.Err() != nil || tr.Entries[len(tr.Entries)-1].Text != "echo: gate" {
		t.Fatalf("err = %v, entries = %#v", again.Err(), tr.Entries)
	}
}

// A run no front-end follows, as one a front-end let go of as it shut down,
// is stopped from its directory as Stop stops one that is followed: the
// runner records why, ends as stopped, and leaves the session free.
func TestStopDetachedStopsARunNoOneFollows(t *testing.T) {
	sessions := t.TempDir()
	ctx, release := context.WithCancel(t.Context())
	job, dir := startDetached(t, ctx, sessions, t.TempDir(), "session-1", "wait for me")
	select {
	case <-job.Lines():
	case <-time.After(10 * time.Second):
		t.Fatal("runner produced no output")
	}
	release()
	collect(t, job)
	if err := cockpit.StopDetached(t.Context(), dir, "the server  shut down\n"); err != nil {
		t.Fatal(err)
	}
	expectFree(t, sessions, "session-1")
	output, err := os.ReadFile(filepath.Join(dir, "out.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	var lines []cockpit.Line
	for line := range strings.Lines(string(output)) {
		lines = append(lines, cockpit.Line{Text: strings.TrimSuffix(line, "\n")})
	}
	if reasons := hardStops(lines); !slices.Equal(reasons, []string{"the server shut down"}) {
		t.Fatalf("recorded stops = %q", reasons)
	}
	if last := lines[len(lines)-1].Text; last != `{"type":"exit","code":130}` {
		t.Fatalf("out.jsonl ends with %s", last)
	}
	// A run that is over is left as it is; a directory without one is none.
	if err := cockpit.StopDetached(t.Context(), dir, "again"); err != nil {
		t.Fatalf("StopDetached of a run that ended: %v", err)
	}
	if input, err := os.ReadFile(filepath.Join(dir, "in.jsonl")); err != nil || strings.Contains(string(input), "again") {
		t.Fatalf("in.jsonl = %q, %v", input, err)
	}
	if err := cockpit.StopDetached(t.Context(), t.TempDir(), "nothing"); !errors.Is(err, cockpit.ErrNotDetached) {
		t.Fatalf("StopDetached of no run: %v, want ErrNotDetached", err)
	}
}

// The front-end that takes up a run reads the session as it was when the
// run started, which the run's output goes on from.
func TestLoadSessionPrefixReadsTheSessionARunStartedFrom(t *testing.T) {
	sessions, workspace := t.TempDir(), t.TempDir()
	if tr, err := cockpit.LoadSessionPrefix(sessions, "session-1", 0); err != nil || len(tr.Entries) != 0 {
		t.Fatalf("a session the run begins: %v", err)
	}
	first, _ := startDetached(t, t.Context(), sessions, workspace, "session-1", "hello")
	collect(t, first)
	before, err := cockpit.LoadSession(sessions, "session-1")
	if err != nil {
		t.Fatal(err)
	}
	second, _ := startDetached(t, t.Context(), sessions, workspace, "session-1", "use a tool")
	_, info, _ := second.Detached()
	collect(t, second)
	if err := second.Err(); err != nil {
		t.Fatal(err)
	}
	prefix, err := cockpit.LoadSessionPrefix(sessions, "session-1", info.SessionSize)
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(prefix.Entries)
	want, _ := json.Marshal(before.Entries)
	if info.SessionSize != before.Size || prefix.Size != before.Size || string(got) != string(want) {
		t.Fatalf("the first %d bytes: %d bytes, %s; want %d bytes, %s", info.SessionSize, prefix.Size, got, before.Size, want)
	}
	if after, err := cockpit.LoadSession(sessions, "session-1"); err != nil || len(after.Entries) <= len(prefix.Entries) {
		t.Fatalf("the session after the run: %v", err)
	}
}
