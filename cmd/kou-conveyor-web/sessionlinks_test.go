package main

import (
	"encoding/json/v2"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
)

// sessionsFolder makes the sessions folder of a workspace in root.
func sessionsFolder(t *testing.T, root, name string) string {
	t.Helper()
	dir := filepath.Join(root, name, ".harness", "sessions")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	return dir
}

// linksIn are the links in dir, name → what each links to.
func linksIn(t *testing.T, dir string) map[string]string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	links := map[string]string{}
	for _, entry := range entries {
		if entry.Type()&os.ModeSymlink != 0 {
			target, err := os.Readlink(filepath.Join(dir, entry.Name()))
			if err != nil {
				t.Fatal(err)
			}
			links[entry.Name()] = target
		}
	}
	return links
}

func mustSync(t *testing.T, dir string, targets ...linkTarget) {
	t.Helper()
	if _, err := syncLinks(dir, targets); err != nil {
		t.Fatal(err)
	}
}

func equalLinks(t *testing.T, got, want map[string]string) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("links = %q, want %q", got, want)
	}
	for name, target := range want {
		if got[name] != target {
			t.Fatalf("links = %q, want %q", got, want)
		}
	}
}

// A workspace gets its link as it is listed and loses it as it is not.
func TestSessionLinksFollowTheWorkspaces(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a, b := sessionsFolder(t, root, "a"), sessionsFolder(t, root, "b")
	mustSync(t, links, linkTarget{"a-1", a})
	equalLinks(t, linksIn(t, links), map[string]string{"a-1": a})
	mustSync(t, links, linkTarget{"a-1", a}, linkTarget{"b-2", b})
	equalLinks(t, linksIn(t, links), map[string]string{"a-1": a, "b-2": b})
	mustSync(t, links, linkTarget{"b-2", b})
	equalLinks(t, linksIn(t, links), map[string]string{"b-2": b})
	// A workspace that moved: its link follows it.
	moved := sessionsFolder(t, root, "b-moved")
	mustSync(t, links, linkTarget{"b-2", moved})
	equalLinks(t, linksIn(t, links), map[string]string{"b-2": moved})
}

// What the server did not make is left alone: a folder, a file, a link of
// the user's under a workspace's name or another, and a link the user
// changed since the server made it.
func TestSessionLinksLeaveTheUsersAlone(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a, b, elsewhere := sessionsFolder(t, root, "a"), sessionsFolder(t, root, "b"), sessionsFolder(t, root, "elsewhere")
	if err := os.MkdirAll(filepath.Join(links, "a-1", "mine"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(links, "notes.txt"), []byte("mine"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(elsewhere, filepath.Join(links, "b-2")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(elsewhere, filepath.Join(links, "my-link")); err != nil {
		t.Fatal(err)
	}
	mustSync(t, links, linkTarget{"a-1", a}, linkTarget{"b-2", b})
	if info, err := os.Lstat(filepath.Join(links, "a-1", "mine")); err != nil || !info.IsDir() {
		t.Fatal("the user's folder was touched")
	}
	if data, err := os.ReadFile(filepath.Join(links, "notes.txt")); err != nil || string(data) != "mine" {
		t.Fatal("the user's file was touched")
	}
	equalLinks(t, linksIn(t, links), map[string]string{"b-2": elsewhere, "my-link": elsewhere})
	mustSync(t, links)
	equalLinks(t, linksIn(t, links), map[string]string{"b-2": elsewhere, "my-link": elsewhere})

	// The server's link, which the user points elsewhere, is the user's.
	c := sessionsFolder(t, root, "c")
	mustSync(t, links, linkTarget{"c-3", c})
	if err := os.Remove(filepath.Join(links, "c-3")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(elsewhere, filepath.Join(links, "c-3")); err != nil {
		t.Fatal(err)
	}
	mustSync(t, links)
	if got := linksIn(t, links)["c-3"]; got != elsewhere {
		t.Fatalf("the link the user changed is gone or changed: %q", got)
	}
}

// Links made before, as the server makes them, are taken over: kept while
// their workspace is listed, removed once it is not.
func TestSessionLinksTakeOverThoseMadeBefore(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a := sessionsFolder(t, root, "a")
	if err := os.MkdirAll(links, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(a, filepath.Join(links, "a-1")); err != nil {
		t.Fatal(err)
	}
	before, _ := os.Lstat(filepath.Join(links, "a-1"))
	mustSync(t, links, linkTarget{"a-1", a})
	after, _ := os.Lstat(filepath.Join(links, "a-1"))
	if !os.SameFile(before, after) {
		t.Fatal("the link made before was made again")
	}
	mustSync(t, links)
	equalLinks(t, linksIn(t, links), map[string]string{})
}

// A link whose folder is gone is removed; it comes back with the folder.
func TestSessionLinksDropBrokenLinks(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a := sessionsFolder(t, root, "a")
	mustSync(t, links, linkTarget{"a-1", a})
	if err := os.RemoveAll(filepath.Join(root, "a")); err != nil {
		t.Fatal(err)
	}
	linked, err := syncLinks(links, []linkTarget{{"a-1", a}})
	if err != nil || len(linked) != 0 {
		t.Fatalf("linked %v, %v", linked, err)
	}
	equalLinks(t, linksIn(t, links), map[string]string{})
	sessionsFolder(t, root, "a")
	mustSync(t, links, linkTarget{"a-1", a})
	equalLinks(t, linksIn(t, links), map[string]string{"a-1": a})
}

// A folder reached two ways is linked once.
func TestSessionLinksLinkAFolderOnce(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a := sessionsFolder(t, root, "a")
	if err := os.Symlink(filepath.Join(root, "a"), filepath.Join(root, "via")); err != nil {
		t.Fatal(err)
	}
	variants := []linkTarget{{"a-1", a}, {"via-2", filepath.Join(root, "via", ".harness", "sessions")}}
	// On a file system that ignores case, so is a folder named in another
	// case.
	upper := filepath.Join(root, "A", ".harness", "sessions")
	if info, err := os.Stat(upper); err == nil && info.IsDir() {
		variants = append(variants, linkTarget{"a-3", upper})
	}
	linked, err := syncLinks(links, variants)
	if err != nil || len(linked) != 1 || linked[0].Name != "a-1" {
		t.Fatalf("linked %v, %v", linked, err)
	}
	equalLinks(t, linksIn(t, links), map[string]string{"a-1": a})
}

// Names that would leave the folder, or take the place of its own files,
// are not linked.
func TestSessionLinksRefuseOddNames(t *testing.T) {
	root, links := t.TempDir(), filepath.Join(t.TempDir(), "sessions")
	a := sessionsFolder(t, root, "a")
	linked, err := syncLinks(links, []linkTarget{{"../a", a}, {".managed.json", a}, {"index.json", a}, {"", a}, {"x/y", a}})
	if err != nil || len(linked) != 0 {
		t.Fatalf("linked %v, %v", linked, err)
	}
	if _, err := os.Lstat(filepath.Join(filepath.Dir(links), "a")); err == nil {
		t.Fatal("a link was made outside the folder")
	}
}

func readIndex(t *testing.T, dir string) sessionIndex {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(dir, sessionIndexFile))
	if err != nil {
		t.Fatal(err)
	}
	var index sessionIndex
	if err := json.Unmarshal(data, &index); err != nil {
		t.Fatalf("index.json: %v\n%s", err, data)
	}
	return index
}

// The index lists the sessions of every workspace with what they are doing,
// and links each workspace's folder; it is made again only when that
// changes.
func TestSessionIndexListsEveryWorkspace(t *testing.T) {
	h := newHarness(t)
	links := &sessionLinks{dir: filepath.Join(t.TempDir(), "sessions")}
	started := h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	runID := started["run_id"].(string)
	waitFor(t, func() bool {
		_, err := os.Stat(filepath.Join(h.server.opt.SessionDir, "session-1.session.jsonl"))
		return err == nil
	})
	if err := links.refresh(h.server); err != nil {
		t.Fatal(err)
	}
	index := readIndex(t, links.dir)
	startup := h.server.workspaces.startup()
	if len(index.Workspaces) != 1 || index.Workspaces[0].ID != startup.ID || index.Workspaces[0].Link != startup.ID {
		t.Fatalf("workspaces = %+v", index.Workspaces)
	}
	if got := linksIn(t, links.dir)[startup.ID]; got != h.server.opt.SessionDir {
		t.Fatalf("link = %q, want %q", got, h.server.opt.SessionDir)
	}
	session, ok := index.Sessions["session-1"]
	if !ok || session.State != "running" || session.RunID != runID || session.Workspace != startup.ID ||
		session.Title != "wait for it" || !strings.HasSuffix(session.File, "session-1.session.jsonl") {
		t.Fatalf("sessions = %+v", index.Sessions)
	}
	stamped := index.UpdatedAt
	time.Sleep(10 * time.Millisecond)
	if err := links.refresh(h.server); err != nil {
		t.Fatal(err)
	}
	if again := readIndex(t, links.dir); !again.UpdatedAt.Equal(stamped) {
		t.Fatal("the index was made again though nothing changed")
	}
	h.do("POST", "/api/runs/"+runID+"/cancel", "")
	waitFor(t, func() bool {
		if err := links.refresh(h.server); err != nil {
			t.Fatal(err)
		}
		return readIndex(t, links.dir).Sessions["session-1"].State == "idle"
	})
}

// The index can be read whole at any time, while it is made again.
func TestSessionIndexIsAlwaysWhole(t *testing.T) {
	h := newHarness(t)
	links := &sessionLinks{dir: filepath.Join(t.TempDir(), "sessions")}
	if err := links.refresh(h.server); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	stop := make(chan struct{})
	wg.Add(1)
	go func() {
		defer wg.Done()
		for i := 0; ; i++ {
			select {
			case <-stop:
				return
			default:
			}
			links.last = nil // made again each time
			if err := links.refresh(h.server); err != nil {
				t.Error(err)
				return
			}
		}
	}()
	for range 200 {
		readIndex(t, links.dir)
	}
	close(stop)
	wg.Wait()
	entries, _ := os.ReadDir(links.dir)
	names := []string{}
	for _, entry := range entries {
		names = append(names, entry.Name())
	}
	if slices.ContainsFunc(names, func(name string) bool { return strings.HasPrefix(name, ".index.json") }) {
		t.Fatalf("temporary files are left: %q", names)
	}
}

// The other working trees of a workspace's git repository that have
// sessions are linked and indexed too.
func TestSessionFoldersTakeInWorktrees(t *testing.T) {
	h := newHarness(t)
	startup := h.server.workspaces.startup()
	tree, bare := t.TempDir(), t.TempDir()
	for name, gitdir := range map[string]string{"with": filepath.Join(tree, ".git"), "without": filepath.Join(bare, ".git")} {
		dir := filepath.Join(startup.Path, ".git", "worktrees", name)
		if err := os.MkdirAll(dir, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "gitdir"), []byte(gitdir+"\n"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	sessions := sessionsFolder(t, filepath.Dir(tree), filepath.Base(tree))
	folders := h.server.sessionFolders()
	if len(folders) != 2 || folders[1].Sessions != sessions || folders[1].WorktreeOf != startup.ID || folders[1].ID != workspaceID(tree) {
		t.Fatalf("folders = %+v", folders)
	}
}

func waitFor(t *testing.T, done func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for !done() {
		if time.Now().After(deadline) {
			t.Fatal("timed out")
		}
		time.Sleep(20 * time.Millisecond)
	}
}
