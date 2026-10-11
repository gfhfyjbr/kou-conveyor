// Package worktree makes the git worktrees agents work in apart from the
// user's checkout: .harness/worktrees/<name> of the workspace, on a branch
// of its own, kou/<name>. A session's sandbox has one (agentrunner), and so
// can a terminal of a canvas.
//
// git checks out only what it tracks. A worktree gets beside that what the
// user's checkout has and a build needs: the submodules that checkout has,
// cloned from its copies, and the untracked paths its Options name, copied
// or linked.
package worktree

import (
	"context"
	"encoding/json/v2"
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

// Options is what a worktree gets beside the files git checks out. What
// they name is put in when the worktree lacks it, each time it is made or
// found again; nothing the worktree has is overwritten.
type Options struct {
	// Include are paths of the workspace git does not carry — vendor/, a
	// tool's folder, fonts — copied in; on APFS the copies share the
	// originals' blocks until written.
	Include []string `json:"include,omitzero"`
	// Link are such paths linked to the workspace's instead: one copy,
	// shared with the user's checkout, for big things nobody writes.
	Link []string `json:"link,omitzero"`
	// NoSubmodules leaves the submodules empty, as git does. Else those the
	// user's checkout has are initialized, cloned from its copies; a commit
	// missing there comes from their remote.
	NoSubmodules bool `json:"no_submodules,omitzero"`
}

// ParseOptions reads options from their JSON, as the runner's environment
// carries them (runconfig.WorktreeEnvironment); empty is none.
func ParseOptions(text string) (Options, error) {
	var options Options
	if strings.TrimSpace(text) == "" {
		return options, nil
	}
	if err := json.Unmarshal([]byte(text), &options); err != nil {
		return Options{}, fmt.Errorf("the worktree's options: %w", err)
	}
	return options, nil
}

// Encode is the options' JSON, which ParseOptions reads.
func (options Options) Encode() string {
	data, _ := json.Marshal(options)
	return string(data)
}

// IsZero reports whether the options are the default: nothing to include
// or link, and the submodules in.
func (options Options) IsZero() bool {
	return len(options.Include) == 0 && len(options.Link) == 0 && !options.NoSubmodules
}

// Worktree is a worktree Ensure made or found again.
type Worktree struct {
	Path, Branch string
	// Made is set when Ensure made it.
	Made bool
	// Filled says what Ensure put in beside git's files this time;
	// Problems what it could not, and why.
	Filled, Problems []string
	// Placed are the paths copied or linked in, this time or before: not
	// tracked, and not for commits.
	Placed []string
}

// Ensure makes the worktree of name, on the branch kou/<name> made from
// base (HEAD when empty), or finds it again, and fills in what options
// name and it lacks. branch, when not empty, names the branch instead.
func Ensure(ctx context.Context, workspace, name, branch, base string, options Options) (Worktree, error) {
	if !ValidName(name) {
		return Worktree{}, fmt.Errorf("%q cannot name a worktree: letters, digits, dots, dashes and underscores", name)
	}
	if _, err := os.Stat(filepath.Join(workspace, ".git")); err != nil {
		return Worktree{}, fmt.Errorf("a worktree needs a git repository; %s has no .git", workspace)
	}
	if branch == "" {
		branch = BranchPrefix + name
	}
	if base == "" {
		base = "HEAD"
	}
	made := Worktree{Path: Path(workspace, name), Branch: branch}
	if _, err := os.Stat(filepath.Join(made.Path, ".git")); err != nil {
		if err := os.MkdirAll(filepath.Dir(made.Path), 0o755); err != nil {
			return Worktree{}, fmt.Errorf("create the worktrees directory: %w", err)
		}
		git := gitIn(ctx, workspace)
		// A stale registration of the path, from a worktree removed by hand.
		git("worktree", "prune")
		if _, err := git("rev-parse", "--verify", "--quiet", "refs/heads/"+branch); err == nil {
			if _, err := git("worktree", "add", made.Path, branch); err != nil {
				return Worktree{}, err
			}
		} else if _, err := git("worktree", "add", "-b", branch, made.Path, base); err != nil {
			return Worktree{}, err
		}
		made.Made = true
	}
	made.Filled, made.Problems = fill(ctx, workspace, made.Path, options, made.Made)
	made.Placed = placed(ctx, made.Path)
	return made, nil
}

type submodule struct{ name, path string }

// fill puts in the worktree at path what it lacks: the submodules, then
// the paths to include and to link. A path the worktree has from git is a
// problem in one just made; later it is the one fill put in, or the
// agent's.
func fill(ctx context.Context, workspace, path string, options Options, fresh bool) (filled, problems []string) {
	submodules := submodulesOf(ctx, path)
	if !options.NoSubmodules {
		for _, sub := range submodules {
			done, err := initSubmodule(ctx, workspace, path, sub)
			switch {
			case err != nil:
				problems = append(problems, fmt.Sprintf("submodule %s: %v", sub.path, err))
			case done:
				filled = append(filled, "submodule "+sub.path)
			}
		}
	}
	place := func(name string, link bool) {
		rel, problem := localPath(name)
		if problem != "" {
			problems = append(problems, name+": "+problem)
			return
		}
		for _, sub := range submodules {
			if filepath.FromSlash(sub.path) == rel {
				problems = append(problems, fmt.Sprintf("%s: a submodule, which is initialized rather than copied or linked", name))
				return
			}
		}
		source, target := filepath.Join(workspace, rel), filepath.Join(path, rel)
		if _, err := os.Lstat(target); err == nil {
			if fresh {
				problems = append(problems, fmt.Sprintf("%s: git checks it out already; name the paths in it git does not carry", name))
			}
			return
		}
		if _, err := os.Lstat(source); err != nil {
			problems = append(problems, fmt.Sprintf("%s: the workspace has none", name))
			return
		}
		var err error
		if err = os.MkdirAll(filepath.Dir(target), 0o755); err == nil {
			if link {
				err = os.Symlink(source, target)
			} else {
				err = copyInto(source, target)
			}
		}
		if err == nil {
			err = record(ctx, path, filepath.ToSlash(rel))
		}
		if err != nil {
			problems = append(problems, fmt.Sprintf("%s: %v", name, err))
			return
		}
		if link {
			filled = append(filled, name+" (linked)")
		} else {
			filled = append(filled, name+" (copied)")
		}
	}
	for _, name := range options.Include {
		place(name, false)
	}
	for _, name := range options.Link {
		place(name, true)
	}
	return filled, problems
}

// placedFile, in the worktree's own git directory, lists the paths fill
// copied or linked in. They are not the agent's changes, and they go with
// the worktree.
const placedFile = "kou-placed"

func record(ctx context.Context, path, rel string) error {
	dir, err := gitIn(ctx, path)("rev-parse", "--absolute-git-dir")
	if err != nil {
		return err
	}
	file, err := os.OpenFile(filepath.Join(dir, placedFile), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o644)
	if err != nil {
		return err
	}
	if _, err := file.WriteString(rel + "\n"); err != nil {
		file.Close()
		return err
	}
	return file.Close()
}

// placed is what fill copied or linked into the worktree at path.
func placed(ctx context.Context, path string) []string {
	dir, err := gitIn(ctx, path)("rev-parse", "--absolute-git-dir")
	if err != nil {
		return nil
	}
	data, _ := os.ReadFile(filepath.Join(dir, placedFile))
	return strings.Fields(string(data))
}

// localPath is a path the options name, cleaned and relative to the
// workspace, or what is wrong with it: one outside the workspace, or git's
// or the harness's own.
func localPath(name string) (string, string) {
	name = strings.TrimRight(strings.TrimSpace(name), "/")
	rel := filepath.Clean(filepath.FromSlash(name))
	if name == "" || !filepath.IsLocal(rel) {
		return "", "not a path inside the workspace"
	}
	if first, _, _ := strings.Cut(filepath.ToSlash(rel), "/"); first == ".git" || first == ".harness" {
		return "", "git's or the harness's own"
	}
	return rel, ""
}

// submodulesOf lists the submodules of the checkout at path, as its
// .gitmodules names them.
func submodulesOf(ctx context.Context, path string) []submodule {
	if _, err := os.Stat(filepath.Join(path, ".gitmodules")); err != nil {
		return nil
	}
	out, err := gitIn(ctx, path)("config", "--file", ".gitmodules", "--get-regexp", `^submodule\..*\.path$`)
	if err != nil {
		return nil
	}
	var submodules []submodule
	for line := range strings.Lines(out) {
		key, value, ok := strings.Cut(strings.TrimSpace(line), " ")
		name := strings.TrimSuffix(strings.TrimPrefix(key, "submodule."), ".path")
		if ok && name != "" && value != "" {
			submodules = append(submodules, submodule{name: name, path: value})
		}
	}
	return submodules
}

// initSubmodule initializes a submodule in the worktree at path when the
// user's checkout has it and the worktree has not: cloned from the
// checkout's copy, which is quick, takes little room, and works with a
// shallow copy, where --reference does not. A commit the copy lacks is
// fetched from the submodule's remote. A submodule cannot be a link to the
// copy: git refuses a submodule that is a symbolic link.
func initSubmodule(ctx context.Context, workspace, path string, sub submodule) (bool, error) {
	dir := filepath.Join(path, filepath.FromSlash(sub.path))
	if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
		return false, nil
	}
	source := filepath.Join(workspace, filepath.FromSlash(sub.path))
	if _, err := os.Stat(filepath.Join(source, ".git")); err != nil {
		// Not initialized in the user's checkout either.
		return false, nil
	}
	git := gitIn(ctx, path)
	// The URL is set for this command alone: the repository's
	// configuration, which the user's checkout shares, stays as it is.
	_, err := git("-c", "protocol.file.allow=always", "-c", "submodule."+sub.name+".url="+source, "submodule", "update", "--init", "--", sub.path)
	if _, cloned := os.Stat(filepath.Join(dir, ".git")); cloned == nil {
		// Its fetches go to its remote, not to the user's copy.
		if url, urlErr := git("config", "--get", "submodule."+sub.name+".url"); urlErr == nil && url != "" {
			gitIn(ctx, dir)("remote", "set-url", "origin", url)
		}
	}
	if err != nil {
		_, err = git("-c", "protocol.file.allow=always", "submodule", "update", "--init", "--", sub.path)
	}
	if err != nil {
		return false, err
	}
	// Where it started, which Dirty does not count as the agent's: the
	// commit may be on no branch the copy had.
	gitIn(ctx, dir)("update-ref", startRef, "HEAD")
	return true, nil
}

// startRef marks, in a submodule Ensure initialized, the commit it was
// checked out at.
const startRef = "refs/kou/start"

// ErrDirty is what Remove says of a worktree with changes not committed.
var ErrDirty = errors.New("the worktree has changes that are not committed")

// Dirty reports whether the worktree at path has changes not committed:
// what git status lists, but for what Ensure copied or linked in; or
// commits in a submodule Ensure initialized that are on none of its
// remotes, which would go with the worktree's copy of the submodule.
func Dirty(ctx context.Context, path string) (bool, error) {
	out, err := gitIn(ctx, path)("status", "--porcelain", "-z", "--untracked-files=all")
	if err != nil {
		return false, err
	}
	ours := placed(ctx, path)
	for _, entry := range strings.Split(out, "\x00") {
		if entry == "" {
			continue
		}
		name, untracked := strings.CutPrefix(entry, "?? ")
		if !untracked || !under(name, ours) {
			return true, nil
		}
	}
	for _, sub := range submodulesOf(ctx, path) {
		dir := filepath.Join(path, filepath.FromSlash(sub.path))
		if _, err := os.Stat(filepath.Join(dir, ".git")); err != nil {
			continue
		}
		count, err := gitIn(ctx, dir)("rev-list", "--count", "HEAD", "--branches", "--not", "--remotes", "--glob="+startRef)
		if err != nil || count != "0" {
			return true, err
		}
	}
	return false, nil
}

// under reports whether name is one of paths, or inside one.
func under(name string, paths []string) bool {
	name = strings.TrimSuffix(name, "/")
	for _, p := range paths {
		if name == p || strings.HasPrefix(name, p+"/") {
			return true
		}
	}
	return false
}

// Remove takes the worktree at path away, unless it has changes not
// committed (ErrDirty); its branch stays, with what was committed on it.
// What Ensure put in goes with it: git wants --force for that, for the
// untracked copies and links, and for the submodules.
func Remove(ctx context.Context, workspace, path string) error {
	dirty, err := Dirty(ctx, path)
	if err != nil {
		return err
	}
	if dirty {
		return ErrDirty
	}
	_, err = gitIn(ctx, workspace)("worktree", "remove", "--force", path)
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
