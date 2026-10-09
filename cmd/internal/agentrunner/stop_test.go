//go:build darwin || linux

package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

func TestStopLine(t *testing.T) {
	for line, want := range map[string]string{
		`{"stop":{"reason":"the user  stopped\n the run"}}`: "the user stopped the run",
		`{"stop":{}}`:              "the cockpit stopped the run without saying why",
		`{"stop":{"reason":"  "}}`: "the cockpit stopped the run without saying why",
	} {
		if reason, ok := stopLine(jsontext.Value(line)); !ok || reason != want {
			t.Errorf("stopLine(%s) = %q, %v; want %q", line, reason, ok, want)
		}
	}
	for _, line := range []string{
		`{"content":"stop"}`,
		`{"stop":{"reason":"x","by":"y"}}`,
		`{"stop":null}`,
		`{"stop":"now"}`,
		`"stop"`,
	} {
		if _, ok := stopLine(jsontext.Value(line)); ok {
			t.Errorf("stopLine(%s) took it for a stop line", line)
		}
	}
}

// submissions is an inbox that hands what is submitted to the channel, and
// waits while nobody takes it.
type submissions chan inbox.Input

func (s submissions) Submit(ctx context.Context, input inbox.Input) error {
	select {
	case s <- input:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func stopReasonOf(t *testing.T, input inbox.Input) string {
	t.Helper()
	control, err := input.DecodeControlMessage()
	if err != nil || control.Mode != inbox.StopHard {
		t.Fatalf("submitted %+v (%v), want a hard stop", input, err)
	}
	return control.Reason
}

// stopReasons are the reasons of the hard stops a runner's output records.
func stopReasons(t *testing.T, output []byte) []string {
	t.Helper()
	var reasons []string
	for _, item := range decodeLogItems(t, output) {
		if input, ok := item.Data.(inbox.Input); ok && input.Kind == inbox.InputControl {
			control, err := input.DecodeControlMessage()
			if err != nil {
				t.Fatal(err)
			}
			if control.Mode == inbox.StopHard {
				reasons = append(reasons, control.Reason)
			}
		}
	}
	return reasons
}

// The first stop counts; the run is canceled only once the grace is over.
func TestRunStopperAsksOnceAndCancelsAfterTheGrace(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	inputs := make(submissions, 2)
	stops := &runStopper{cancel: cancel, grace: 200 * time.Millisecond}
	stops.ready(ctx, inputs)
	stops.stop("first")
	stops.stop("second")
	if !stops.asked() || len(inputs) != 1 || stopReasonOf(t, <-inputs) != "first" {
		t.Fatal("the first stop was not the one asked for, alone")
	}
	if ctx.Err() != nil {
		t.Fatal("the stop canceled the run before its grace")
	}
	select {
	case <-ctx.Done():
	case <-time.After(5 * time.Second):
		t.Fatal("the run was not canceled after the grace")
	}
}

// A stop the coordinator never takes still ends the run.
func TestRunStopperCancelsWhenTheStopIsNotTaken(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	stops := &runStopper{cancel: cancel, grace: 50 * time.Millisecond}
	stops.ready(ctx, make(submissions))
	done := make(chan struct{})
	go func() {
		stops.stop("nobody listens")
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("a stop nobody takes never ended")
	}
	if ctx.Err() == nil {
		t.Fatal("the run was not canceled")
	}
}

// Before the inbox is open, a stop cancels the run at once.
func TestRunStopperBeforeTheInbox(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	(&runStopper{cancel: cancel}).stop("early")
	if ctx.Err() == nil {
		t.Fatal("the run was not canceled")
	}
}

// An interrupt waits a moment for the stop line that says why; without one,
// it stops for the interrupt.
func TestInterruptWaitsForTheStopLine(t *testing.T) {
	for _, line := range []bool{true, false} {
		ctx, cancel := context.WithCancel(t.Context())
		inputs := make(submissions, 2)
		stops := &runStopper{cancel: cancel, grace: time.Minute, lineWait: 200 * time.Millisecond}
		stops.ready(ctx, inputs)
		stops.interrupt("the runner was interrupted")
		if stops.asked() {
			t.Fatal("the interrupt did not wait for the stop line")
		}
		want := "the runner was interrupted"
		if line {
			want = "the user stopped the run"
			stops.stop(want)
		}
		select {
		case input := <-inputs:
			if got := stopReasonOf(t, input); got != want {
				t.Fatalf("line %v: stopped for %q, want %q", line, got, want)
			}
		case <-time.After(5 * time.Second):
			t.Fatalf("line %v: no stop", line)
		}
		time.Sleep(300 * time.Millisecond)
		if len(inputs) != 0 {
			t.Fatalf("line %v: stopped twice", line)
		}
		cancel()
	}
}

// A stop line stops a -steer run: the session records the stop with its
// reason and the tool call it canceled, the tool is ended, the runner exits
// 130 without waiting out the grace, and the run after it tells the model
// why the run before stopped.
func TestStopLineIsRecordedAndToldToTheNextRun(t *testing.T) {
	workspace, sessions := t.TempDir(), t.TempDir()
	pidFile := filepath.Join(t.TempDir(), "tool.pid")
	stdin, feed := io.Pipe()
	defer feed.Close()
	client := &fakeClient{}
	client.respond = func(ctx context.Context, _ llm.Request) (llm.Response, error) {
		client.mu.Lock()
		client.calls++
		first := client.calls == 1
		client.mu.Unlock()
		if !first {
			<-ctx.Done()
			return llm.Response{}, ctx.Err()
		}
		return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemToolCall,
			Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"echo $$ > ` + pidFile + `; sleep 60"}`},
		}}}, nil
	}
	go func() {
		io.WriteString(feed, `{"session_id":"stopped","messages":[{"content":"run it"}],"model":"gpt-test"}`+"\n")
		for t.Context().Err() == nil {
			if data, err := os.ReadFile(pidFile); err == nil && len(bytes.TrimSpace(data)) > 0 {
				io.WriteString(feed, `{"stop":{"reason":"the user stopped the run from the cockpit"}}`+"\n")
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
	}()
	env := func() []string { return []string{"PATH=/usr/bin:/bin"} }
	var stdout, stderr bytes.Buffer
	started := time.Now()
	code := RunMain(t.Context(), []string{"-workspace", workspace, "-session-directory", sessions, "-steer"},
		steerEnv, env, stdin, &stdout, &stderr, testConfig(client))
	if code != 130 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	if elapsed := time.Since(started); elapsed >= stopGrace {
		t.Fatalf("the stop waited out its grace: %v", elapsed)
	}
	data, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		t.Fatal(err)
	}
	if syscall.Kill(pid, 0) == nil {
		syscall.Kill(-pid, syscall.SIGKILL)
		t.Fatal("the tool outlived the stop")
	}
	if reasons := stopReasons(t, stdout.Bytes()); !slices.Equal(reasons, []string{"the user stopped the run from the cockpit"}) {
		t.Fatalf("recorded stops = %q", reasons)
	}
	canceled := false
	for _, item := range decodeLogItems(t, stdout.Bytes()) {
		if status, ok := item.Data.(sessionstore.ToolCallStatus); ok && status.CallID == "call-1" {
			for _, value := range status.Operations {
				canceled = canceled || value.Status == operation.StatusCanceled
			}
		}
	}
	if !canceled {
		t.Fatalf("the session does not record the call canceled: %s", stdout.String())
	}

	resumed := &fakeClient{}
	var first llm.Request
	resumed.respond = func(_ context.Context, request llm.Request) (llm.Response, error) {
		resumed.mu.Lock()
		defer resumed.mu.Unlock()
		resumed.calls++
		if resumed.calls == 1 {
			first = request
		}
		return llm.Response{ID: "response-2", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Done."},
		}}}, nil
	}
	stdout.Reset()
	stderr.Reset()
	code = RunMain(t.Context(), []string{"-workspace", workspace, "-session-directory", sessions},
		steerEnv, env, strings.NewReader(`{"session_id":"stopped","messages":[{"role":"user","content":"go on"}],"model":"gpt-test"}`),
		&stdout, &stderr, testConfig(resumed))
	if code != 0 {
		t.Fatalf("resumed run: exit = %d, stderr = %q", code, stderr.String())
	}
	var notes []string
	for _, item := range first.Input {
		if message, ok := item.Data.(llm.Message); ok && strings.Contains(message.Text, "The previous run was stopped") {
			notes = append(notes, message.Text)
		}
	}
	if len(notes) != 1 || !strings.HasPrefix(notes[0], "[harness] The previous run was stopped at ") ||
		!strings.Contains(notes[0], ": the user stopped the run from the cockpit. The tool calls it was making were canceled") {
		t.Fatalf("resume notes = %q", notes)
	}
}
