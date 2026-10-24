package fswatch_test

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/fswatch"
)

// watching watches roots for the test, or skips it where the system tells
// of no changes.
func watching(t *testing.T, roots ...string) *fswatch.Watcher {
	t.Helper()
	w, err := fswatch.Watch(roots...)
	if errors.Is(err, errors.ErrUnsupported) {
		t.Skip(err)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(w.Close)
	return w
}

// told waits for the watcher to tell of a change.
func told(t *testing.T, w *fswatch.Watcher, what string) {
	t.Helper()
	select {
	case <-w.Changes():
	case <-time.After(10 * time.Second):
		t.Fatalf("%s: no change told", what)
	}
}

// quiet checks the watcher tells of nothing for a while, once what it had
// to tell is told.
func quiet(t *testing.T, w *fswatch.Watcher, what string) {
	t.Helper()
	settle(w)
	select {
	case <-w.Changes():
		t.Errorf("%s: a change told", what)
	case <-time.After(time.Second):
	}
}

// settle takes what the watcher has to tell, until it has had nothing new
// for half a second.
func settle(w *fswatch.Watcher) {
	for {
		select {
		case <-w.Changes():
		case <-time.After(500 * time.Millisecond):
			return
		}
	}
}

func write(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestWatchTellsOfChangesUnderItsRoots(t *testing.T) {
	dir := t.TempDir()
	plugins, settings, skills := filepath.Join(dir, "plugins"), filepath.Join(dir, "plugins.json"), filepath.Join(dir, "skills")
	write(t, filepath.Join(plugins, "a", "plugin.json"), "{}")
	write(t, settings, "{}")
	w := watching(t, plugins, settings, skills)
	settle(w)

	write(t, filepath.Join(plugins, "a", "plugin.json"), `{"name": "a"}`)
	told(t, w, "a file written")
	settle(w)
	write(t, filepath.Join(plugins, "b", "deep", "ui.js"), "")
	told(t, w, "a file made in a new directory")
	settle(w)
	write(t, settings, `{"disabled": ["a"]}`)
	told(t, w, "a root that is a file written")
	settle(w)
	write(t, filepath.Join(skills, "x", "SKILL.md"), "---\nname: x\n---\n")
	told(t, w, "a root that was not there made")
	settle(w)
	if err := os.RemoveAll(filepath.Join(plugins, "b")); err != nil {
		t.Fatal(err)
	}
	told(t, w, "a directory removed")

	write(t, filepath.Join(dir, "elsewhere", "file"), "")
	write(t, filepath.Join(dir, "plugins.json.bak"), "")
	quiet(t, w, "files beside the roots written")
}

// The roots are followed by the paths the files have: through a link to
// them, and as the system has them (/var is /private/var).
func TestWatchFollowsWhereALinkLeads(t *testing.T) {
	dir := t.TempDir()
	write(t, filepath.Join(dir, "real", "file"), "")
	if err := os.Symlink(filepath.Join(dir, "real"), filepath.Join(dir, "link")); err != nil {
		t.Fatal(err)
	}
	w := watching(t, filepath.Join(dir, "link"))
	settle(w)
	write(t, filepath.Join(dir, "real", "file"), "changed")
	told(t, w, "a file written where the link leads")
}

// A watcher closed tells of nothing after, and may be closed again.
func TestWatcherClosed(t *testing.T) {
	dir := t.TempDir()
	w := watching(t, dir)
	settle(w)
	w.Close()
	w.Close()
	write(t, filepath.Join(dir, "file"), "")
	select {
	case <-w.Changes():
		t.Error("a closed watcher told of a change")
	case <-time.After(time.Second):
	}
}

// Changes coalesce: a burst of them is told once or a few times, and the
// watcher does not wait on a reader.
func TestWatchCoalesces(t *testing.T) {
	dir := t.TempDir()
	w := watching(t, dir)
	settle(w)
	for i := range 200 {
		write(t, filepath.Join(dir, "file"), string(rune('a'+i%26)))
	}
	told(t, w, "a burst of writes")
	n := 1
	for {
		select {
		case <-w.Changes():
			n++
			continue
		case <-time.After(500 * time.Millisecond):
		}
		break
	}
	if n > 20 {
		t.Errorf("a burst of 200 writes told %d times", n)
	}
}

func TestWatchWantsRoots(t *testing.T) {
	if _, err := fswatch.Watch(); err == nil {
		t.Error("Watch of nothing did not fail")
	}
}
