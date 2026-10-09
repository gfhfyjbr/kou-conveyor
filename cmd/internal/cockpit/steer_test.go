package cockpit_test

import (
	"errors"
	"strings"
	"testing"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

func TestSteerHandsTheRunningAgentAMessage(t *testing.T) {
	job := start(t, t.Context(), t.TempDir(), uuid.New().String(), "steer")
	if !job.CanSteer() {
		t.Fatal("the runner takes no messages")
	}
	if err := job.Steer("not-a-uuid", "hi"); err == nil {
		t.Fatal("a message without a UUID was taken")
	}
	tr := cockpit.NewTranscript()
	message := uuid.New().String()
	sent := false
	for line := range job.Lines() {
		if line.Stderr {
			continue
		}
		if _, err := tr.Apply([]byte(line.Text)); err != nil {
			t.Fatal(err)
		}
		// While the command runs, the user adds something.
		if running := tr.Running(); !sent && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
			if err := job.Steer(message, "use pnpm"); err != nil {
				t.Fatal(err)
			}
			sent = true
		}
	}
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, e := range tr.Entries {
		switch {
		case e.Kind == cockpit.KindUser && e.Forced:
			got = append(got, "forced "+e.Text)
		case e.Kind == cockpit.KindTool:
			got = append(got, "tool "+e.Tool.State)
		default:
			got = append(got, e.Kind+" "+e.Text)
		}
	}
	want := "user steer | tool done | forced use pnpm | assistant steered: use pnpm"
	if strings.Join(got, " | ") != want {
		t.Fatalf("transcript = %q, want %q", strings.Join(got, " | "), want)
	}
	if tr.Entry("input:"+message) == nil {
		t.Fatal("the message was not recorded under its ID")
	}
	if job.CanSteer() || !errors.Is(job.Steer(uuid.New().String(), "late"), cockpit.ErrCannotSteer) {
		t.Fatal("a run that ended took a message")
	}
}

func TestOlderRunnersTakeNoMessages(t *testing.T) {
	t.Setenv(cockpittest.NoSteer, "1")
	cockpit.ForgetRunners()
	t.Cleanup(cockpit.ForgetRunners)
	job := start(t, t.Context(), t.TempDir(), uuid.New().String(), "hello")
	if job.CanSteer() || !errors.Is(job.Steer(uuid.New().String(), "hi"), cockpit.ErrCannotSteer) {
		t.Fatal("a runner without -steer was handed a message")
	}
	tr := cockpit.NewTranscript()
	for line := range job.Lines() {
		if !line.Stderr {
			tr.Apply([]byte(line.Text))
		}
	}
	if err := job.Err(); err != nil || len(tr.Entries) != 2 || tr.Entries[1].Text != "echo: hello" {
		t.Fatalf("err %v, entries %d", err, len(tr.Entries))
	}
}
