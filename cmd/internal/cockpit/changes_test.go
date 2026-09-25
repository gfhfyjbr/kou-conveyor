package cockpit_test

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func write(t *testing.T, dir, name, content string) {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}

func gitIn(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-c", "user.email=t@t", "-c", "user.name=t"}, args...)...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return string(out)
}

// run records one prompt's run that makes the given change.
func run(t *testing.T, c *cockpit.Changes, session, message string, change func()) cockpit.ChangeUpdate {
	t.Helper()
	tracker, err := c.Begin(t.Context(), session, message)
	if err != nil {
		t.Fatal(err)
	}
	change()
	tracker.Poke()
	var last cockpit.ChangeUpdate
	deadline := time.After(20 * time.Second)
	for {
		select {
		case u, ok := <-tracker.Updates():
			if !ok {
				return last
			}
			if len(u.Changed) > 0 {
				last = u
				tracker.Finish()
			}
		case <-deadline:
			t.Fatal("no snapshot came")
		}
	}
}

func TestChangesAreRecordedPerPrompt(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	ws := t.TempDir()
	sessions := filepath.Join(ws, ".harness", "sessions")
	write(t, ws, "main.go", "package main\n\nfunc main() {}\n")
	write(t, ws, "notes.txt", "keep\n")
	write(t, ws, "old.md", "# Old\nsame text\nmore text\n")
	write(t, ws, "gone.txt", "bye\n")
	write(t, ws, ".gitignore", "*.log\n")
	c := cockpit.NewChanges(ws, sessions, "")

	update := run(t, c, "session-1", "message-1", func() {
		write(t, ws, "main.go", "package main\n\nfunc main() {\n\tprintln(\"hi\")\n}\n")
		write(t, ws, "pkg/new.go", "package pkg\n")
		os.Remove(filepath.Join(ws, "gone.txt"))
		os.Rename(filepath.Join(ws, "old.md"), filepath.Join(ws, "docs", "renamed.md"))
		os.MkdirAll(filepath.Join(ws, "docs"), 0o700)
		os.Rename(filepath.Join(ws, "old.md"), filepath.Join(ws, "docs", "renamed.md"))
		write(t, ws, "image.bin", "\x00\x01\x02\x03")
		// None of these are changes to show.
		write(t, ws, "node_modules/dep/index.js", "x")
		write(t, ws, "logs/20260101-120000.jsonl", "{}")
		write(t, ws, "build.log", "noise")
		write(t, ws, ".harness/sessions/session-1.session.jsonl", "{}")
	})
	if !slices.Contains(update.Changed, "main.go") || update.Message != "message-1" {
		t.Fatalf("update = %+v", update)
	}

	ex, ok := c.Exchange("session-1", "message-1")
	if !ok || ex.Before == ex.After || !slices.Contains(ex.Latest, "main.go") {
		t.Fatalf("exchange = %+v, %v", ex, ok)
	}
	files, err := c.Files(t.Context(), ex.Before, ex.After)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]cockpit.FileChange{}
	for _, f := range files {
		got[f.Path] = f
	}
	for path, want := range map[string]cockpit.FileChange{
		"main.go":         {Path: "main.go", Status: "modified", Added: 3, Removed: 1},
		"pkg/new.go":      {Path: "pkg/new.go", Status: "added", Added: 1},
		"gone.txt":        {Path: "gone.txt", Status: "deleted", Removed: 1},
		"docs/renamed.md": {Path: "docs/renamed.md", OldPath: "old.md", Status: "renamed"},
		"image.bin":       {Path: "image.bin", Status: "added", Binary: true},
	} {
		if got[path] != want {
			t.Errorf("%s: %+v, want %+v", path, got[path], want)
		}
	}
	if len(files) != 5 {
		t.Fatalf("files = %+v", files)
	}
	patch, truncated, err := c.Diff(t.Context(), ex.Before, ex.After, "main.go", "")
	if err != nil || truncated || !strings.Contains(patch, "+\tprintln(\"hi\")") || !strings.Contains(patch, "-func main() {}") {
		t.Fatalf("patch %q, %v, %v", patch, truncated, err)
	}
	if patch, _, _ := c.Diff(t.Context(), ex.Before, ex.After, "docs/renamed.md", "old.md"); !strings.Contains(patch, "rename from old.md") {
		t.Fatalf("rename patch %q", patch)
	}

	// The next prompt has changes of its own.
	run(t, c, "session-1", "message-2", func() { write(t, ws, "notes.txt", "keep\nand more\n") })
	second, _ := c.Exchange("session-1", "message-2")
	if files, _ := c.Files(t.Context(), second.Before, second.After); len(files) != 1 || files[0].Path != "notes.txt" {
		t.Fatalf("second prompt's files = %+v", files)
	}
	if _, ok := c.Exchange("session-1", "message-3"); ok {
		t.Fatal("a prompt without a run has changes")
	}
	c.Forget("session-1")
	if _, ok := c.Exchange("session-1", "message-1"); ok {
		t.Fatal("the records outlived the session")
	}
}

func TestChangesLeaveTheWorkspaceRepositoryAlone(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	ws := t.TempDir()
	gitIn(t, ws, "init", "-q")
	write(t, ws, "a.txt", "one\n")
	gitIn(t, ws, "add", "a.txt")
	gitIn(t, ws, "commit", "-qm", "first")
	head := gitIn(t, ws, "rev-parse", "HEAD")
	// A nested repository without a commit cannot be added; the rest can.
	write(t, ws, "nested/x.txt", "x")
	gitIn(t, filepath.Join(ws, "nested"), "init", "-q")

	sessions := filepath.Join(ws, ".harness", "sessions")
	c := cockpit.NewChanges(ws, sessions, "")
	run(t, c, "s", "m", func() { write(t, ws, "a.txt", "one\ntwo\n") })
	if ex, _ := c.Exchange("s", "m"); ex.Before == ex.After {
		t.Fatal("the change went unrecorded")
	}
	if gitIn(t, ws, "rev-parse", "HEAD") != head {
		t.Fatal("HEAD moved")
	}
	status := gitIn(t, ws, "status", "--porcelain", "--untracked-files=all")
	if strings.Contains(status, ".changes") || strings.Contains(status, "A ") || !strings.Contains(status, " M a.txt") {
		t.Fatalf("the workspace's repository changed:\n%s", status)
	}
	if refs := gitIn(t, ws, "for-each-ref"); strings.Count(refs, "\n") != 1 {
		t.Fatalf("refs:\n%s", refs)
	}
}

// The terminal and the web cockpit may take snapshots of one workspace at
// the same time.
func TestSnapshotsFromTwoCockpitsAtOnce(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	ws := t.TempDir()
	for i := range 200 {
		write(t, ws, filepath.Join("dir", strings.Repeat("f", 1+i%7)+string(rune('a'+i%26))+".txt"), strings.Repeat("x", i))
	}
	sessions := filepath.Join(ws, ".harness", "sessions")
	a, b := cockpit.NewChanges(ws, sessions, ""), cockpit.NewChanges(ws, sessions, "")
	var wg sync.WaitGroup
	errs := make(chan error, 20)
	for i := range 20 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c := a
			if i%2 == 1 {
				c = b
			}
			if _, err := c.Snapshot(t.Context()); err != nil {
				errs <- err
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Fatal(err)
	}
}

// Files too large to record are left out until they are small again, so a
// data set or a download does not take a copy of itself per snapshot.
func TestLargeFilesAreLeftOut(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	ws := t.TempDir()
	write(t, ws, "small.txt", "small\n")
	sessions := filepath.Join(ws, ".harness", "sessions")
	c := cockpit.NewChanges(ws, sessions, "")
	large := strings.Repeat("0123456789abcdef", (6<<20)/16)
	update := run(t, c, "s", "m1", func() {
		write(t, ws, "data/big name [1].csv", large)
		write(t, ws, "archive.zip", "zip")
		write(t, ws, "small.txt", "small\nchanged\n")
	})
	if !slices.Equal(update.Changed, []string{"small.txt"}) {
		t.Fatalf("changed = %v", update.Changed)
	}
	exclude, _ := os.ReadFile(filepath.Join(sessions, ".changes", "repo.git", "info", "exclude"))
	if !strings.Contains(string(exclude), `/data/big name \[1].csv`) {
		t.Fatalf("exclude:\n%s", exclude)
	}

	// Once it is small it is a change like any other.
	run(t, c, "s", "m2", func() { write(t, ws, "data/big name [1].csv", "a,b\n") })
	ex, _ := c.Exchange("s", "m2")
	files, err := c.Files(t.Context(), ex.Before, ex.After)
	if err != nil || len(files) != 1 || files[0].Path != "data/big name [1].csv" || files[0].Status != "added" {
		t.Fatalf("files = %+v, %v", files, err)
	}
	if exclude, _ := os.ReadFile(filepath.Join(sessions, ".changes", "repo.git", "info", "exclude")); strings.Contains(string(exclude), "big name") {
		t.Fatalf("still excluded:\n%s", exclude)
	}
}

// A git that was killed leaves its index lock behind; snapshots break it
// once it is old, and wait for one that is not.
func TestSnapshotsBreakAStaleLock(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	ws := t.TempDir()
	write(t, ws, "a.txt", "a\n")
	sessions := filepath.Join(ws, ".harness", "sessions")
	c := cockpit.NewChanges(ws, sessions, "")
	if _, err := c.Snapshot(t.Context()); err != nil {
		t.Fatal(err)
	}
	lock := filepath.Join(sessions, ".changes", "index.lock")
	write(t, filepath.Dir(lock), "index.lock", "")

	ctx, cancel := context.WithTimeout(t.Context(), 300*time.Millisecond)
	defer cancel()
	if _, err := c.Snapshot(ctx); err == nil {
		t.Fatal("a snapshot went past a lock another git may hold")
	}
	old := time.Now().Add(-10 * time.Minute)
	if err := os.Chtimes(lock, old, old); err != nil {
		t.Fatal(err)
	}
	write(t, ws, "a.txt", "b\n")
	if _, err := c.Snapshot(t.Context()); err != nil {
		t.Fatalf("stale lock: %v", err)
	}
	if _, err := os.Stat(lock); !os.IsNotExist(err) {
		t.Fatalf("the lock is still there: %v", err)
	}
}

// A home directory is too wide to copy, and asking why creates nothing.
func TestHomeIsNotRecorded(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	sessions := filepath.Join(home, ".harness", "sessions")
	c := cockpit.NewChanges(home, sessions, "")
	if err := c.Available(); !errors.Is(err, cockpit.ErrWideWorkspace) {
		t.Fatalf("available: %v", err)
	}
	if _, err := c.Begin(t.Context(), "s", "m"); !errors.Is(err, cockpit.ErrWideWorkspace) {
		t.Fatalf("begin: %v", err)
	}
	if _, err := os.Stat(filepath.Join(sessions, ".changes")); !os.IsNotExist(err) {
		t.Fatalf("created the repository: %v", err)
	}

	ws := filepath.Join(home, "project")
	write(t, ws, "a.txt", "a\n")
	project := cockpit.NewChanges(ws, filepath.Join(ws, ".harness", "sessions"), "")
	if err := project.Available(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(ws, ".harness")); !os.IsNotExist(err) {
		t.Fatalf("available created files: %v", err)
	}
	if got := cockpit.Sentence(cockpit.ErrWideWorkspace.Error()); !strings.HasPrefix(got, "Changes are not") || !strings.HasSuffix(got, ".") {
		t.Fatalf("sentence %q", got)
	}
}
