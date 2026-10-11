package worktree

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func git(t *testing.T, dir string, arguments ...string) string {
	t.Helper()
	command := exec.Command("git", append([]string{"-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "protocol.file.allow=always"}, arguments...)...)
	command.Dir = dir
	out, err := command.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", arguments, err, out)
	}
	return strings.TrimSpace(string(out))
}

func write(t *testing.T, path, text string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func read(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

// repositories makes a workspace like joroweave: a big dependency as a
// submodule its checkout cloned shallow, from an upstream of three
// commits, and beside what git carries an ignored vendor/, with a link in
// it, and a tool's folder nobody committed.
func repositories(t *testing.T) (upstream, workspace string) {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	upstream, workspace = filepath.Join(root, "upstream"), filepath.Join(root, "workspace")
	for _, dir := range []string{upstream, workspace} {
		if err := os.Mkdir(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		git(t, dir, "init", "-q", "-b", "main")
	}
	for i := range 3 {
		write(t, filepath.Join(upstream, "f.txt"), fmt.Sprint(i))
		git(t, upstream, "add", ".")
		git(t, upstream, "commit", "-qm", fmt.Sprint("c", i))
	}
	write(t, filepath.Join(workspace, "README.md"), "joroweave\n")
	write(t, filepath.Join(workspace, ".gitignore"), "vendor/\n")
	git(t, workspace, "add", ".")
	git(t, workspace, "commit", "-qm", "start")
	git(t, workspace, "submodule", "add", "-q", "--depth", "1", "file://"+upstream, "deps/sub")
	git(t, workspace, "commit", "-qm", "the dependency")
	write(t, filepath.Join(workspace, "vendor", "lib", "a.txt"), "vendored")
	if err := os.Symlink("lib", filepath.Join(workspace, "vendor", "current")); err != nil {
		t.Fatal(err)
	}
	write(t, filepath.Join(workspace, "tools", "oracle", "run.sh"), "#!/bin/sh\n")
	return upstream, workspace
}

func TestAWorktreeGetsWhatGitLeavesOut(t *testing.T) {
	upstream, workspace := repositories(t)
	ctx := context.Background()
	shared := read(t, filepath.Join(workspace, ".git", "config"))
	options := Options{Include: []string{"vendor/"}, Link: []string{"tools/oracle"}}
	made, err := Ensure(ctx, workspace, "w1", "", "", options)
	if err != nil {
		t.Fatal(err)
	}
	if !made.Made || made.Path != Path(workspace, "w1") || made.Branch != "kou/w1" || len(made.Problems) != 0 {
		t.Fatalf("made %+v", made)
	}
	if got := fmt.Sprint(made.Filled); got != "[submodule deps/sub vendor/ (copied) tools/oracle (linked)]" {
		t.Fatalf("filled %s", got)
	}
	if got := fmt.Sprint(made.Placed); got != "[vendor tools/oracle]" {
		t.Fatalf("placed %s", got)
	}

	// The submodule, from the checkout's shallow copy, at the commit of the
	// branch; it fetches from its remote, and the configuration the
	// checkout shares is as it was.
	sub := filepath.Join(made.Path, "deps", "sub")
	if got, want := git(t, sub, "rev-parse", "HEAD"), git(t, workspace, "rev-parse", "HEAD:deps/sub"); got != want {
		t.Fatalf("the submodule is at %s, not %s", got, want)
	}
	if url := git(t, sub, "remote", "get-url", "origin"); url != "file://"+upstream {
		t.Fatalf("the submodule fetches from %s", url)
	}
	if read(t, filepath.Join(workspace, ".git", "config")) != shared {
		t.Fatal("the repository's configuration changed")
	}

	// vendor/ copied, its link a link; the tool's folder linked.
	if info, err := os.Lstat(filepath.Join(made.Path, "vendor")); err != nil || !info.IsDir() {
		t.Fatalf("vendor is %v (%v)", info, err)
	}
	if text := read(t, filepath.Join(made.Path, "vendor", "lib", "a.txt")); text != "vendored" {
		t.Fatalf("vendor/lib/a.txt reads %q", text)
	}
	if link, err := os.Readlink(filepath.Join(made.Path, "vendor", "current")); err != nil || link != "lib" {
		t.Fatalf("vendor/current links to %q (%v)", link, err)
	}
	if link, err := os.Readlink(filepath.Join(made.Path, "tools", "oracle")); err != nil || link != filepath.Join(workspace, "tools", "oracle") {
		t.Fatalf("tools/oracle links to %q (%v)", link, err)
	}

	// None of that is the agent's change.
	if dirty, err := Dirty(ctx, made.Path); err != nil || dirty {
		t.Fatalf("dirty %v (%v)", dirty, err)
	}

	// Found again, it gets nothing more, and keeps what it has.
	write(t, filepath.Join(made.Path, "vendor", "lib", "a.txt"), "patched")
	again, err := Ensure(ctx, workspace, "w1", "", "", options)
	if err != nil || again.Made || len(again.Filled) != 0 || len(again.Problems) != 0 {
		t.Fatalf("again %+v (%v)", again, err)
	}
	if text := read(t, filepath.Join(made.Path, "vendor", "lib", "a.txt")); text != "patched" {
		t.Fatalf("vendor/lib/a.txt was overwritten: %q", text)
	}

	// The agent's file is a change, and so is a commit in the submodule
	// that is on none of its remotes, even when it is not checked out.
	write(t, filepath.Join(made.Path, "notes.txt"), "mine")
	if err := Remove(ctx, workspace, made.Path); !errors.Is(err, ErrDirty) {
		t.Fatalf("removing a worktree with the agent's file: %v", err)
	}
	os.Remove(filepath.Join(made.Path, "notes.txt"))
	start := git(t, sub, "rev-parse", "HEAD")
	git(t, sub, "checkout", "-q", "-b", "fix")
	write(t, filepath.Join(sub, "f.txt"), "fixed")
	git(t, sub, "commit", "-qam", "fix")
	git(t, sub, "checkout", "-q", "--detach", start)
	if dirty, err := Dirty(ctx, made.Path); err != nil || !dirty {
		t.Fatalf("a commit only the worktree's submodule has: dirty %v (%v)", dirty, err)
	}
	git(t, sub, "branch", "-q", "-D", "fix")
	if dirty, err := Dirty(ctx, made.Path); err != nil || dirty {
		t.Fatalf("dirty %v (%v)", dirty, err)
	}

	// Removed, the worktree goes, with its submodule and what was put in;
	// its branch stays, and what the link pointed to.
	if err := Remove(ctx, workspace, made.Path); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(made.Path); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("the worktree is still there: %v", err)
	}
	git(t, workspace, "rev-parse", "--verify", "--quiet", "refs/heads/kou/w1")
	if text := read(t, filepath.Join(workspace, "tools", "oracle", "run.sh")); text != "#!/bin/sh\n" {
		t.Fatalf("the workspace's tool reads %q", text)
	}
	if text := read(t, filepath.Join(workspace, "vendor", "lib", "a.txt")); text != "vendored" {
		t.Fatalf("the workspace's vendor reads %q", text)
	}
}

func TestASubmoduleCommitTheCheckoutLacksComesFromItsRemote(t *testing.T) {
	upstream, workspace := repositories(t)
	// Upstream moves on, and a branch points the submodule there: a
	// commit the checkout's shallow copy has never fetched.
	write(t, filepath.Join(upstream, "f.txt"), "3")
	git(t, upstream, "commit", "-qam", "c3")
	newer := git(t, upstream, "rev-parse", "HEAD")
	git(t, workspace, "checkout", "-q", "-b", "newer")
	git(t, workspace, "update-index", "--cacheinfo", "160000,"+newer+",deps/sub")
	git(t, workspace, "commit", "-qm", "a newer dependency")
	git(t, workspace, "checkout", "-q", "main")
	if exec.Command("git", "-C", filepath.Join(workspace, "deps", "sub"), "cat-file", "-e", newer).Run() == nil {
		t.Fatal("the checkout's copy has the newer commit")
	}
	made, err := Ensure(context.Background(), workspace, "w2", "", "newer", Options{})
	if err != nil {
		t.Fatal(err)
	}
	if len(made.Problems) != 0 || fmt.Sprint(made.Filled) != "[submodule deps/sub]" {
		t.Fatalf("made %+v", made)
	}
	if got := git(t, filepath.Join(made.Path, "deps", "sub"), "rev-parse", "HEAD"); got != newer {
		t.Fatalf("the submodule is at %s, not %s", got, newer)
	}
	if dirty, err := Dirty(context.Background(), made.Path); err != nil || dirty {
		t.Fatalf("dirty %v (%v)", dirty, err)
	}
}

func TestWhatAWorktreeCannotGet(t *testing.T) {
	_, workspace := repositories(t)
	made, err := Ensure(context.Background(), workspace, "w3", "", "", Options{
		Include:      []string{"../outside", "/etc", ".git/hooks", ".harness", "missing", "deps/sub", "README.md"},
		NoSubmodules: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{
		"../outside: not a path inside the workspace",
		"/etc: not a path inside the workspace",
		".git/hooks: git's or the harness's own",
		".harness: git's or the harness's own",
		"missing: the workspace has none",
		"deps/sub: a submodule, which is initialized rather than copied or linked",
		"README.md: git checks it out already; name the paths in it git does not carry",
	}
	if got := strings.Join(made.Problems, "\n"); got != strings.Join(want, "\n") {
		t.Fatalf("problems:\n%s", got)
	}
	if len(made.Filled) != 0 || len(made.Placed) != 0 {
		t.Fatalf("made %+v", made)
	}
	if _, err := os.Stat(filepath.Join(made.Path, "deps", "sub", ".git")); err == nil {
		t.Fatal("the submodule was initialized")
	}

	// The options travel as JSON, in the runner's environment.
	options := Options{Include: []string{"vendor"}, Link: []string{"fonts"}, NoSubmodules: true}
	if parsed, err := ParseOptions(options.Encode()); err != nil || fmt.Sprint(parsed) != fmt.Sprint(options) {
		t.Fatalf("parsed %+v (%v) from %s", parsed, err, options.Encode())
	}
	if parsed, err := ParseOptions(""); err != nil || !parsed.IsZero() {
		t.Fatalf("parsed %+v (%v)", parsed, err)
	}
	if _, err := ParseOptions("{nope"); err == nil {
		t.Fatal("bad JSON was taken")
	}
}
