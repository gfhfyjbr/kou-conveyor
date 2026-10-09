// Package worktree makes the git worktrees agents work in apart from the
// user's checkout: .harness/worktrees/<name> of the workspace, on a branch
// of its own, kou/<name>. A session's sandbox has one (agentrunner), and so
// can a terminal of a canvas.
package worktree

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
)

// Directory is where a workspace keeps its worktrees.
const Directory = ".harness/worktrees"

// BranchPrefix starts the branches of the worktrees.
const BranchPrefix = "kou/"

var validName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`)

// ValidName reports whether name can name a worktree: letters, digits, dots,
// dashes and underscores, starting with a letter or a digit.
func ValidName(name string) bool { return validName.MatchString(name) && !strings.Contains(name, "..") }

// Slug turns a title into a worktree's name: lowercase letters, digits and
// dashes, at most 40 of them; "" when nothing is left.
func Slug(title string) string {
	var b strings.Builder
	dash := false
	for _, r := range strings.ToLower(title) {
		switch {
		case r >= 'a' && r <= 'z' || r >= '0' && r <= '9':
			b.WriteRune(r)
			dash = false
		case b.Len() > 0 && !dash:
			b.WriteByte('-')
			dash = true
		}
		if b.Len() >= 40 {
			break
		}
	}
	return strings.Trim(b.String(), "-")
}

// Path is where the worktree of a name is.
func Path(workspace, name string) string {
	return filepath.Join(workspace, filepath.FromSlash(Directory), name)
}

// Ensure makes the worktree of name, on the branch kou/<name> made from
// base (HEAD when empty), or finds it again; it returns the worktree's path
// and its branch. branch, when not empty, names the branch instead.
func Ensure(ctx context.Context, workspace, name, branch, base string) (string, string, error) {
	if !ValidName(name) {
		return "", "", fmt.Errorf("%q cannot name a worktree: letters, digits, dots, dashes and underscores", name)
	}
	if _, err := os.Stat(filepath.Join(workspace, ".git")); err != nil {
		return "", "", fmt.Errorf("a worktree needs a git repository; %s has no .git", workspace)
	}
	if branch == "" {
		branch = BranchPrefix + name
	}
	if base == "" {
		base = "HEAD"
	}
	path := Path(workspace, name)
	if _, err := os.Stat(filepath.Join(path, ".git")); err == nil {
		return path, branch, nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return "", "", fmt.Errorf("create the worktrees directory: %w", err)
	}
	git := gitIn(ctx, workspace)
	// A stale registration of the path, from a worktree removed by hand.
	git("worktree", "prune")
	if _, err := git("rev-parse", "--verify", "--quiet", "refs/heads/"+branch); err == nil {
		if _, err := git("worktree", "add", path, branch); err != nil {
			return "", "", err
		}
	} else if _, err := git("worktree", "add", "-b", branch, path, base); err != nil {
		return "", "", err
	}
	return path, branch, nil
}

// ErrDirty is what Remove says of a worktree with changes not committed.
var ErrDirty = errors.New("the worktree has changes that are not committed")

// Dirty reports whether the worktree at path has changes not committed:
// what git status --porcelain lists.
func Dirty(ctx context.Context, path string) (bool, error) {
	out, err := gitIn(ctx, path)("status", "--porcelain")
	if err != nil {
		return false, err
	}
	return strings.TrimSpace(out) != "", nil
}

// Remove takes the worktree at path away, unless it has changes not
// committed (ErrDirty); its branch stays, with what was committed on it.
func Remove(ctx context.Context, workspace, path string) error {
	dirty, err := Dirty(ctx, path)
	if err != nil {
		return err
	}
	if dirty {
		return ErrDirty
	}
	_, err = gitIn(ctx, workspace)("worktree", "remove", path)
	return err
}

func gitIn(ctx context.Context, dir string) func(arguments ...string) (string, error) {
	return func(arguments ...string) (string, error) {
		command := exec.CommandContext(ctx, "git", arguments...)
		command.Dir = dir
		out, err := command.CombinedOutput()
		if err != nil {
			return "", fmt.Errorf("git %s: %v: %s", strings.Join(arguments, " "), err, strings.TrimSpace(string(out)))
		}
		return strings.TrimSpace(string(out)), nil
	}
}
