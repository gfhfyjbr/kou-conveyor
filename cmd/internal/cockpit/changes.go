package cockpit

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode"
	"unicode/utf8"
)

// Changes records how runs change their workspace, so the cockpits can show
// each prompt's changes file by file. Commands change files however they
// like, so changes are found by taking snapshots of the workspace: one when
// a prompt's run starts and more as its tool calls finish. Snapshots go to a
// git repository of the cockpits' own beside the sessions, with an index of
// its own: they never touch the workspace's repository, and work in folders
// that have none. What .gitignore leaves out, they leave out too.
//
// Several cockpits may snapshot one workspace at once; git's index lock keeps
// them apart. Runs in one workspace at the same time share its changes.
//
// Snapshots copy what they record, so they leave out what would take the
// most room and show the least: dependencies, caches, archives, media,
// databases, logs, and files larger than maxFileSize that were not recorded
// before. A home directory or a file system's root is not recorded at all.
type Changes struct {
	workspace string
	dir       string   // <sessions>/.changes: the repository, its index and the records
	exclude   []string // patterns for info/exclude, from the work tree's root
	lock      chan struct{}
	once      sync.Once
	initErr   error
}

// emptyTree is git's tree with nothing in it.
const emptyTree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"

// ErrNoGit reports a system without git, which changes are recorded with.
var ErrNoGit = errors.New("changes are recorded with git, which is not on PATH")

// ErrWideWorkspace reports a workspace too wide to record: a snapshot would
// copy everything in it.
var ErrWideWorkspace = errors.New("changes are not recorded in a home directory or a file system's root")

const (
	// maxFileSize is the size of the largest file a snapshot takes in. A
	// larger one, data or output that no .gitignore names, would be copied
	// whole with each change.
	maxFileSize = 5 << 20
	// staleLock is how old an index lock is when no snapshot holds it any
	// more: every git a snapshot runs is done, or stopped, well before.
	staleLock = 3 * time.Minute
	// largeMarker starts the part of info/exclude that lists the files too
	// large to record; it changes with the workspace.
	largeMarker = "# Too large to record:"
)

// NewChanges keeps the changes of a workspace whose sessions are in
// sessionDir. logDir is where the runner writes its logs, "" for its default
// in the workspace; logs are not changes.
func NewChanges(workspace, sessionDir, logDir string) *Changes {
	c := &Changes{workspace: workspace, dir: filepath.Join(sessionDir, ".changes"), lock: make(chan struct{}, 1)}
	c.exclude = []string{
		"/.harness/",
		// Dependencies and caches, even where no .gitignore names them.
		"node_modules/", ".venv/", "venv/", "__pycache__/", "*.pyc", ".mypy_cache/", ".pytest_cache/",
		".tox/", ".gradle/", ".next/", ".nuxt/", ".turbo/", ".terraform/", ".DS_Store",
		// Logs, and what no diff shows: archives, disk images, media,
		// databases, data sets, model weights and compiled objects.
		"*.log",
		"*.zip", "*.tar", "*.tgz", "*.gz", "*.bz2", "*.xz", "*.zst", "*.7z", "*.rar", "*.jar", "*.war",
		"*.iso", "*.dmg", "*.img", "*.vmdk", "*.qcow2",
		"*.mp4", "*.mov", "*.avi", "*.mkv", "*.webm", "*.mp3", "*.wav", "*.flac", "*.ogg", "*.m4a", "*.aac",
		"*.db", "*.db-journal", "*.db-wal", "*.db-shm", "*.sqlite", "*.sqlite3", "*.sqlite-journal", "*.sqlite-wal", "*.sqlite-shm",
		"*.parquet", "*.arrow", "*.feather", "*.h5", "*.hdf5", "*.npy", "*.npz", "*.pkl", "*.pickle",
		"*.safetensors", "*.gguf", "*.ckpt", "*.pt", "*.pth", "*.onnx", "*.tflite",
		"*.o", "*.obj", "*.a", "*.so", "*.dylib", "*.dll", "*.exe", "*.class", "*.pyo",
	}
	if logDir == "" {
		logDir = filepath.Join(workspace, "logs")
	}
	for _, dir := range []string{sessionDir, logDir} {
		if rel, err := filepath.Rel(workspace, dir); err == nil && rel != "." && !strings.HasPrefix(rel, "..") {
			pattern := "/" + filepath.ToSlash(rel) + "/"
			if dir == logDir {
				pattern += "[0-9]*-[0-9]*.jsonl" // the runner's logs, not whatever else is there
			}
			c.exclude = append(c.exclude, pattern)
		}
	}
	return c
}

func (c *Changes) repo() string  { return filepath.Join(c.dir, "repo.git") }
func (c *Changes) index() string { return filepath.Join(c.dir, "index") }

// ensure creates the repository the first time it is needed.
func (c *Changes) ensure(ctx context.Context) error {
	c.once.Do(func() {
		if c.initErr = c.recordable(); c.initErr != nil {
			return
		}
		if err := os.MkdirAll(c.dir, 0o700); err != nil {
			c.initErr = err
			return
		}
		// The workspace's own repository leaves all of this alone.
		_ = os.WriteFile(filepath.Join(c.dir, ".gitignore"), []byte("*\n"), 0o600)
		head := filepath.Join(c.repo(), "HEAD")
		if _, err := os.Stat(head); err != nil {
			cmd := exec.CommandContext(ctx, "git", "init", "--bare", "--quiet", c.repo())
			cmd.Env = gitEnv(nil)
			stopGently(cmd)
			if out, err := cmd.CombinedOutput(); err != nil {
				// Another cockpit may be creating it: it is there once
				// HEAD is.
				for range 100 {
					if _, statErr := os.Stat(head); statErr == nil {
						err = nil
						break
					}
					time.Sleep(20 * time.Millisecond)
				}
				if err != nil {
					c.initErr = fmt.Errorf("create the changes repository: %v: %s", err, bytes.TrimSpace(out))
					return
				}
			}
		}
		c.initErr = c.writeExclude(c.excludedLarge())
	})
	return c.initErr
}

// recordable reports why changes cannot be recorded here, or nil.
func (c *Changes) recordable() error {
	if _, err := exec.LookPath("git"); err != nil {
		return ErrNoGit
	}
	workspace := resolved(c.workspace)
	if filepath.Dir(workspace) == workspace {
		return ErrWideWorkspace
	}
	if home, err := os.UserHomeDir(); err == nil && home != "" && resolved(home) == workspace {
		return ErrWideWorkspace
	}
	return nil
}

// resolved is a path with its symbolic links resolved, where they can be.
func resolved(path string) string {
	if real, err := filepath.EvalSymlinks(path); err == nil {
		return real
	}
	return filepath.Clean(path)
}

// writeExclude writes info/exclude: the fixed patterns, then the files too
// large to record.
func (c *Changes) writeExclude(large []string) error {
	lines := append([]string(nil), c.exclude...)
	if len(large) > 0 {
		lines = append(lines, largeMarker)
		for _, path := range large {
			lines = append(lines, ignorePattern(path))
		}
	}
	want := []byte(strings.Join(lines, "\n") + "\n")
	path := filepath.Join(c.repo(), "info", "exclude")
	if have, err := os.ReadFile(path); err == nil && bytes.Equal(have, want) {
		return nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	// Replaced whole: another cockpit's git may be reading it.
	return replaceFile(path, want)
}

// excludedLarge lists the files info/exclude leaves out as too large.
func (c *Changes) excludedLarge() []string {
	data, err := os.ReadFile(filepath.Join(c.repo(), "info", "exclude"))
	if err != nil {
		return nil
	}
	_, rest, found := strings.Cut(string(data), largeMarker+"\n")
	if !found {
		return nil
	}
	var paths []string
	for _, line := range strings.Split(rest, "\n") {
		if line != "" {
			paths = append(paths, unignorePattern(line))
		}
	}
	return paths
}

// excludeLarge leaves the files too large to record out of the snapshot
// about to be taken: those no snapshot has that are larger now, and those
// left out before that still are. A file recorded before goes on being
// recorded.
func (c *Changes) excludeLarge(ctx context.Context) error {
	out, err := c.git(ctx, "ls-files", "-z", "--others", "--exclude-standard")
	if err != nil {
		return err
	}
	seen := map[string]bool{}
	var large []string
	for _, path := range append(strings.Split(string(out), "\x00"), c.excludedLarge()...) {
		// A line break cannot be written in a pattern.
		if path == "" || seen[path] || strings.ContainsAny(path, "\n\r") {
			continue
		}
		seen[path] = true
		info, err := os.Lstat(filepath.Join(c.workspace, filepath.FromSlash(path)))
		if err == nil && info.Mode().IsRegular() && info.Size() > maxFileSize {
			large = append(large, path)
		}
	}
	slices.Sort(large)
	return c.writeExclude(large)
}

// ignorePattern is the exclude pattern that matches exactly one path.
func ignorePattern(path string) string {
	var b strings.Builder
	b.WriteByte('/')
	for _, r := range path {
		switch r {
		case '\\', '*', '?', '[', '!', '#':
			b.WriteByte('\\')
		}
		b.WriteRune(r)
	}
	pattern := b.String()
	// Spaces at the end count only when escaped.
	trimmed := strings.TrimRight(pattern, " ")
	return trimmed + strings.Repeat("\\ ", len(pattern)-len(trimmed))
}

// unignorePattern is the path an ignorePattern matches.
func unignorePattern(pattern string) string {
	var b strings.Builder
	escaped := false
	for _, r := range strings.TrimPrefix(pattern, "/") {
		if r == '\\' && !escaped {
			escaped = true
			continue
		}
		escaped = false
		b.WriteRune(r)
	}
	return b.String()
}

// gitEnv is the environment git runs in: none of the caller's GIT_
// variables, which could point it at another repository.
func gitEnv(extra []string) []string {
	var env []string
	for _, v := range os.Environ() {
		if !strings.HasPrefix(v, "GIT_") {
			env = append(env, v)
		}
	}
	return append(env, append([]string{"GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0", "LC_ALL=C"}, extra...)...)
}

func (c *Changes) git(ctx context.Context, args ...string) ([]byte, error) {
	full := []string{
		"-c", "core.fsmonitor=false", "-c", "core.autocrlf=false", "-c", "core.safecrlf=false",
		"-c", "core.quotepath=false", "-c", "gc.auto=0",
		"--git-dir=" + c.repo(), "--work-tree=" + c.workspace,
	}
	cmd := exec.CommandContext(ctx, "git", append(full, args...)...)
	cmd.Dir = c.workspace
	cmd.Env = gitEnv([]string{"GIT_INDEX_FILE=" + c.index()})
	stopGently(cmd)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return out, fmt.Errorf("git %s: %w: %s", args[0], err, strings.TrimSpace(stderr.String()))
	}
	return out, nil
}

// stopGently has a git that runs out of time asked to stop, not killed: it
// then removes the index lock it holds, which would stop every snapshot
// after it. One that does not stop is killed a moment later.
func stopGently(cmd *exec.Cmd) {
	cmd.Cancel = func() error { return cmd.Process.Signal(syscall.SIGTERM) }
	cmd.WaitDelay = 5 * time.Second
}

// Available reports why changes cannot be recorded here, or nil. It
// creates nothing.
func (c *Changes) Available() error { return c.recordable() }

// Sentence makes a message a sentence to show: capitalized, with a full
// stop.
func Sentence(s string) string {
	s = strings.TrimSpace(s)
	if s == "" {
		return s
	}
	r, size := utf8.DecodeRuneInString(s)
	s = string(unicode.ToUpper(r)) + s[size:]
	if !strings.HasSuffix(s, ".") {
		s += "."
	}
	return s
}

// breakStaleLock removes an index lock that a git which was killed, or
// crashed, left behind.
func (c *Changes) breakStaleLock() {
	lock := c.index() + ".lock"
	if info, err := os.Stat(lock); err == nil && time.Since(info.ModTime()) > staleLock {
		os.Remove(lock)
	}
}

// Snapshot records the workspace as it is and returns its tree.
func (c *Changes) Snapshot(ctx context.Context) (string, error) {
	if err := c.ensure(ctx); err != nil {
		return "", err
	}
	select {
	case c.lock <- struct{}{}:
	case <-ctx.Done():
		return "", ctx.Err()
	}
	defer func() { <-c.lock }()
	if err := c.excludeLarge(ctx); err != nil && ctx.Err() != nil {
		return "", ctx.Err()
	}
	for attempt := 0; ; attempt++ {
		// Unreadable paths, and nested repositories without a commit, are
		// left out; everything else is added.
		_, err := c.git(ctx, "add", "--all", "--ignore-errors", "--", ".")
		if err == nil || !locked(err) {
			var out []byte
			if out, err = c.git(ctx, "write-tree"); err == nil {
				return strings.TrimSpace(string(out)), nil
			}
		}
		if !locked(err) || attempt >= 100 {
			return "", err
		}
		c.breakStaleLock()
		// Another cockpit is taking a snapshot of this workspace.
		select {
		case <-time.After(time.Duration(20+rand.IntN(60)) * time.Millisecond):
		case <-ctx.Done():
			return "", ctx.Err()
		}
	}
}

// locked reports git failing on the index lock another git holds.
func locked(err error) bool { return err != nil && strings.Contains(err.Error(), "index.lock") }

// changeRecord is a line of a session's change records.
type changeRecord struct {
	Message string    `json:"message"`
	Tree    string    `json:"tree"`
	Before  bool      `json:"before,omitzero"` // taken as the prompt's run started
	Changed []string  `json:"changed,omitzero"`
	At      time.Time `json:"at"`
}

func (c *Changes) records(session string) string { return filepath.Join(c.dir, session+".jsonl") }

func (c *Changes) record(session string, r changeRecord) error {
	if !ValidSessionID(session) {
		return fmt.Errorf("invalid session ID %q", session)
	}
	data, err := json.Marshal(r)
	if err != nil {
		return err
	}
	file, err := os.OpenFile(c.records(session), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	_, err = file.Write(append(data, '\n'))
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	return err
}

// Exchange is what a prompt's run changed: the workspace before it and now,
// or when it ended.
type Exchange struct {
	Message string `json:"message"`
	Before  string `json:"before"`
	After   string `json:"after"`
	// Latest are the files the last snapshot that found a change found
	// changed: the ones the agent touched last.
	Latest []string `json:"latest,omitzero"`
}

// Exchange returns what the run of the prompt with the given message ID
// changed, as far as it was recorded.
func (c *Changes) Exchange(session, message string) (Exchange, bool) {
	ex := Exchange{Message: message}
	if !ValidSessionID(session) {
		return ex, false
	}
	file, err := os.Open(c.records(session))
	if err != nil {
		return ex, false
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 0, 64<<10), 4<<20)
	for scanner.Scan() {
		var r changeRecord
		if json.Unmarshal(scanner.Bytes(), &r) != nil || r.Message != message {
			continue
		}
		if r.Before {
			ex.Before, ex.After, ex.Latest = r.Tree, r.Tree, nil
			continue
		}
		ex.After = r.Tree
		if len(r.Changed) > 0 {
			ex.Latest = r.Changed
		}
	}
	return ex, ex.Before != ""
}

// FileChange is how one file changed.
type FileChange struct {
	Path    string `json:"path"`
	OldPath string `json:"old_path,omitzero"` // renamed from
	Status  string `json:"status"`            // added, modified, deleted, renamed, typechange
	Added   int    `json:"added"`
	Removed int    `json:"removed"`
	Binary  bool   `json:"binary,omitzero"`
}

// Files lists the files that differ between two snapshots, by path.
func (c *Changes) Files(ctx context.Context, before, after string) ([]FileChange, error) {
	if before == after {
		return nil, nil
	}
	if err := c.ensure(ctx); err != nil {
		return nil, err
	}
	status, err := c.git(ctx, "diff-tree", "-r", "-M", "-z", "--name-status", "--no-commit-id", before, after)
	if err != nil {
		return nil, err
	}
	numstat, err := c.git(ctx, "diff-tree", "-r", "-M", "-z", "--numstat", "--no-commit-id", before, after)
	if err != nil {
		return nil, err
	}
	var files []FileChange
	byPath := map[string]int{}
	fields := strings.Split(strings.TrimSuffix(string(status), "\x00"), "\x00")
	for i := 0; i+1 < len(fields); {
		code := fields[i]
		f := FileChange{Path: fields[i+1]}
		i += 2
		switch code[0] {
		case 'A':
			f.Status = "added"
		case 'D':
			f.Status = "deleted"
		case 'T':
			f.Status = "typechange"
		case 'R', 'C':
			if i >= len(fields) {
				continue
			}
			f.Status, f.OldPath, f.Path = "renamed", f.Path, fields[i]
			i++
		default:
			f.Status = "modified"
		}
		byPath[f.Path] = len(files)
		files = append(files, f)
	}
	// numstat: "added\tremoved\tpath\0", or "added\tremoved\t\0old\0new\0"
	// for a rename; "-" counts for binary files.
	parts := strings.Split(strings.TrimSuffix(string(numstat), "\x00"), "\x00")
	for i := 0; i < len(parts); i++ {
		counts := strings.SplitN(parts[i], "\t", 3)
		if len(counts) < 3 {
			continue
		}
		path := counts[2]
		if path == "" && i+2 < len(parts) {
			path = parts[i+2]
			i += 2
		}
		at, ok := byPath[path]
		if !ok {
			continue
		}
		if counts[0] == "-" {
			files[at].Binary = true
			continue
		}
		files[at].Added, _ = strconv.Atoi(counts[0])
		files[at].Removed, _ = strconv.Atoi(counts[1])
	}
	slices.SortFunc(files, func(a, b FileChange) int { return strings.Compare(a.Path, b.Path) })
	return files, nil
}

// Diff limits: a patch bigger than this is cut, and says so.
const (
	maxDiffBytes = 512 << 10
	maxDiffLines = 5000
)

// Diff returns the unified diff of one file between two snapshots. oldPath
// is where a renamed file came from.
func (c *Changes) Diff(ctx context.Context, before, after, path, oldPath string) (patch string, truncated bool, err error) {
	if err := c.ensure(ctx); err != nil {
		return "", false, err
	}
	// Literal pathspecs: a path is never a pattern.
	paths := []string{":(literal)" + path}
	if oldPath != "" && oldPath != path {
		paths = append(paths, ":(literal)"+oldPath)
	}
	out, err := c.git(ctx, append([]string{"diff-tree", "-p", "-M", "--no-color", "--no-ext-diff", "--no-commit-id", before, after, "--"}, paths...)...)
	if err != nil {
		return "", false, err
	}
	if len(out) > maxDiffBytes {
		out, truncated = out[:bytes.LastIndexByte(out[:maxDiffBytes], '\n')+1], true
	}
	if lines := bytes.Count(out, []byte{'\n'}); lines > maxDiffLines {
		cut := 0
		for range maxDiffLines {
			cut += bytes.IndexByte(out[cut:], '\n') + 1
		}
		out, truncated = out[:cut], true
	}
	return string(out), truncated, nil
}

// changed lists the paths that differ between two snapshots.
func (c *Changes) changed(ctx context.Context, before, after string) ([]string, error) {
	out, err := c.git(ctx, "diff-tree", "-r", "-z", "--name-only", "--no-commit-id", before, after)
	if err != nil {
		return nil, err
	}
	var paths []string
	for _, p := range strings.Split(string(out), "\x00") {
		if p != "" {
			paths = append(paths, p)
		}
	}
	return paths, nil
}

// Forget removes the change records of a deleted session.
func (c *Changes) Forget(session string) {
	if ValidSessionID(session) {
		os.Remove(c.records(session))
	}
}

// ---------------------------------------------------------------- tracker

// ChangeUpdate says a snapshot during a run found changes.
type ChangeUpdate struct {
	Message string   `json:"message"`
	Tree    string   `json:"tree"`
	Changed []string `json:"changed"`
	Final   bool     `json:"final,omitzero"` // the run ended
}

// A Tracker takes the snapshots of one prompt's run. The first is taken by
// Begin, before the runner starts; more follow when tool calls finish, now
// and then while one runs, and when the run ends.
type Tracker struct {
	c                *Changes
	session, message string
	previous         string
	wake             chan struct{}
	stop             chan bool // true: take a last snapshot
	updates          chan ChangeUpdate
}

// Begin takes the snapshot a prompt's run starts from.
func (c *Changes) Begin(ctx context.Context, session, message string) (*Tracker, error) {
	tree, err := c.Snapshot(ctx)
	if err != nil {
		return nil, err
	}
	if err := c.record(session, changeRecord{Message: message, Tree: tree, Before: true, At: time.Now().UTC()}); err != nil {
		return nil, err
	}
	t := &Tracker{
		c: c, session: session, message: message, previous: tree,
		wake: make(chan struct{}, 1), stop: make(chan bool, 1), updates: make(chan ChangeUpdate, 32),
	}
	go t.loop()
	return t, nil
}

// Updates delivers what the snapshots found, and is closed when the run's
// last snapshot is taken.
func (t *Tracker) Updates() <-chan ChangeUpdate { return t.updates }

// Poke asks for a snapshot soon: a tool call finished.
func (t *Tracker) Poke() {
	select {
	case t.wake <- struct{}{}:
	default:
	}
}

// Finish takes the last snapshot, after the run.
func (t *Tracker) Finish() { t.end(true) }

// Cancel stops without another snapshot: the run did not start.
func (t *Tracker) Cancel() { t.end(false) }

func (t *Tracker) end(snapshot bool) {
	select {
	case t.stop <- snapshot:
	default:
	}
}

func (t *Tracker) loop() {
	defer close(t.updates)
	// Long commands write as they go; the pause grows with what a snapshot
	// of the workspace costs.
	pause := 5 * time.Second
	timer := time.NewTimer(pause)
	defer timer.Stop()
	for {
		select {
		case <-t.wake:
		case <-timer.C:
		case last := <-t.stop:
			if last {
				t.take(true)
			}
			return
		}
		took := t.take(false)
		timer.Reset(max(pause, 10*took))
	}
}

func (t *Tracker) take(final bool) time.Duration {
	start := time.Now()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	tree, err := t.c.Snapshot(ctx)
	if err != nil {
		return time.Since(start)
	}
	var changed []string
	if tree != t.previous {
		changed, _ = t.c.changed(ctx, t.previous, tree)
	}
	if len(changed) > 0 {
		if t.c.record(t.session, changeRecord{Message: t.message, Tree: tree, Changed: changed, At: time.Now().UTC()}) == nil {
			t.previous = tree
		}
	}
	if len(changed) > 0 || final {
		// A reader that falls behind misses updates; the next carries the tree.
		select {
		case t.updates <- ChangeUpdate{Message: t.message, Tree: t.previous, Changed: changed, Final: final}:
		default:
		}
	}
	return time.Since(start)
}
