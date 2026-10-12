package agentrunner

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/worktree"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

func TestASandboxWorktreeGetsWhatGitDoesNotCarry(t *testing.T) {
	workspace, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for _, arguments := range [][]string{{"init", "-q", "-b", "main"}, {"commit", "-q", "--allow-empty", "-m", "start"}} {
		command := exec.Command("git", append([]string{"-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"}, arguments...)...)
		command.Dir = workspace
		if out, err := command.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", arguments, err, out)
		}
	}
	if err := os.MkdirAll(filepath.Join(workspace, "vendor"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "vendor", "a.txt"), []byte("vendored"), 0o644); err != nil {
		t.Fatal(err)
	}
	options := `{"include":["vendor"]}`
	getenv := func(name string) string {
		if name == runconfig.WorktreeEnvironment {
			return options
		}
		return ""
	}
	var report strings.Builder
	box, err := prepareSandbox(context.Background(), SandboxWorktree, workspace, "/bin/bash", t.TempDir(), session.ID("0123456789abcdef"), getenv, plugin.Found{}, &report)
	if err != nil {
		t.Fatal(err)
	}
	if box.Workspace != worktree.Path(workspace, "01234567") || box.Branch != "kou/01234567" || strings.Join(box.Placed, ",") != "vendor" {
		t.Fatalf("sandbox %+v", box)
	}
	if data, err := os.ReadFile(filepath.Join(box.Workspace, "vendor", "a.txt")); err != nil || string(data) != "vendored" {
		t.Fatalf("vendor/a.txt in the worktree: %q (%v)", data, err)
	}
	if !strings.Contains(box.Description(), "vendor came from the user's checkout, not from git: leave them out of commits.") {
		t.Fatalf("description: %s", box.Description())
	}
	if !strings.Contains(report.String(), "sandbox> worktree "+box.Workspace) || !strings.Contains(report.String(), "sandbox> the worktree got vendor (copied)") {
		t.Fatalf("report:\n%s", report.String())
	}

	// The next run finds it again; options it cannot read are said, and
	// left out.
	report.Reset()
	options = "{not json"
	again, err := prepareSandbox(context.Background(), SandboxWorktree, workspace, "/bin/bash", t.TempDir(), session.ID("0123456789abcdef"), getenv, plugin.Found{}, &report)
	if err != nil || again.Workspace != box.Workspace || strings.Join(again.Placed, ",") != "vendor" {
		t.Fatalf("again %+v (%v)", again, err)
	}
	if got := report.String(); !strings.HasPrefix(got, "sandbox> "+runconfig.WorktreeEnvironment+": ") || strings.Contains(got, "sandbox> worktree ") {
		t.Fatalf("report:\n%s", got)
	}
}
