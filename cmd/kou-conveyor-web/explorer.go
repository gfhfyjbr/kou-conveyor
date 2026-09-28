package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
)

// The workspace's files, for the Files tab of the sidebar: its folders one
// at a time, with what git ignores and what it sees changed; a file's text
// with its highlighting (cmd/internal/highlight), which a large file gets
// in two steps — its start at once, the rest from ?tokens=1 — and pictures.
// Paths are the workspace's, and stay in it: files are opened through a
// root, which a link out of the workspace does not leave.

const (
	// maxTreeEntries bounds the entries of a folder listed.
	maxTreeEntries = 5000
	// maxViewed bounds the text of a file shown.
	maxViewed = 8 << 20
	// maxPicture bounds a picture shown.
	maxPicture = 32 << 20
	// highlightBudget is how long highlighting may hold a file's answer;
	// what it leaves follows with ?tokens=1.
	highlightBudget = 120 * time.Millisecond
	// gitStatusFresh is how long git's word on what changed is kept.
	gitStatusFresh = 2 * time.Second
)

// pictures are the files shown as images, by extension.
var pictures = map[string]string{
	".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
	".webp": "image/webp", ".bmp": "image/bmp", ".ico": "image/x-icon", ".avif": "image/avif",
}

// explorer keeps, per workspace, what the tree marks: the files git takes
// and what it sees changed.
type explorer struct {
	highlights *highlight.Cache

	mu    sync.Mutex
	known map[string]*knownFiles // by workspace ID
	git   map[string]*gitStatus  // by workspace ID
}

// knownFiles are the files of a workspace git takes, or a walk finds, as a
// set: those not in it are ignored.
type knownFiles struct {
	list []string // the listing the set was made of
	set  map[string]struct{}
}

type gitStatus struct {
	mu    sync.Mutex
	at    time.Time
	files map[string]string // path → M, A, D, R, U or ?
	dirs  map[string]bool   // folders that hold a change
}

func newExplorer() *explorer {
	return &explorer{highlights: highlight.NewCache(12_000_000), known: make(map[string]*knownFiles), git: make(map[string]*gitStatus)}
}

// workspaceRelative cleans a path of the workspace, "." for its root, and
// says whether it stays in the workspace.
func workspaceRelative(raw string) (string, bool) {
	cleaned := strings.Trim(path.Clean("/"+strings.ReplaceAll(raw, "\\", "/")), "/")
	if cleaned == "" {
		return ".", true
	}
	return cleaned, fs.ValidPath(cleaned)
}

// knownSet is the set of the workspace's files and folders (folders with a
// slash at the end) that are not ignored; nil when there is none yet.
func (s *server) knownSet(ctx context.Context, ws *workspace) map[string]struct{} {
	ctx, cancel := context.WithTimeout(ctx, 1500*time.Millisecond)
	defer cancel()
	list, err := ws.files().Entries(ctx)
	if err != nil || len(list) == 0 {
		return nil
	}
	x := s.explorer
	x.mu.Lock()
	defer x.mu.Unlock()
	if known := x.known[ws.ID]; known != nil && len(known.list) == len(list) && &known.list[0] == &list[0] {
		return known.set
	}
	set := make(map[string]struct{}, len(list))
	for _, entry := range list {
		set[entry] = struct{}{}
	}
	x.known[ws.ID] = &knownFiles{list: list, set: set}
	return set
}

// changes is what git sees changed in the workspace, kept for a moment.
func (s *server) changes(ctx context.Context, ws *workspace) *gitStatus {
	x := s.explorer
	x.mu.Lock()
	status := x.git[ws.ID]
	if status == nil {
		status = &gitStatus{}
		x.git[ws.ID] = status
	}
	x.mu.Unlock()
	status.mu.Lock()
	defer status.mu.Unlock()
	if time.Since(status.at) < gitStatusFresh {
		return status
	}
	ctx, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "git", "-C", ws.Path, "status", "--porcelain=v1", "-z",
		"--untracked-files=all", "--ignore-submodules=dirty").Output()
	status.at = time.Now()
	status.files, status.dirs = map[string]string{}, map[string]bool{}
	if err != nil {
		return status
	}
	fields := bytes.Split(out, []byte{0})
	for i := 0; i < len(fields); i++ {
		field := string(fields[i])
		if len(field) < 4 {
			continue
		}
		x, y, name := field[0], field[1], field[3:]
		letter := string(y)
		switch {
		case x == '?':
			letter = "?"
		case x == 'U' || y == 'U' || x == 'A' && y == 'A' || x == 'D' && y == 'D':
			letter = "U"
		case y == ' ':
			letter = string(x)
		}
		if x == 'R' || x == 'C' {
			i++ // the path it came from follows
		}
		status.files[name] = letter
		for dir := path.Dir(name); dir != "." && dir != "/"; dir = path.Dir(dir) {
			status.dirs[dir] = true
		}
	}
	return status
}

type treeEntry struct {
	Name    string `json:"name"`
	Dir     bool   `json:"dir,omitzero"`
	Link    bool   `json:"link,omitzero"`
	Broken  bool   `json:"broken,omitzero"`
	Size    int64  `json:"size,omitzero"`
	Ignored bool   `json:"ignored,omitzero"`
	Git     string `json:"git,omitzero"`
	Changed bool   `json:"changed,omitzero"`
}

// handleTree lists a folder of the workspace, ?path= from its root: its
// folders first, then its files, by name.
func (s *server) handleTree(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	relative, ok := workspaceRelative(r.URL.Query().Get("path"))
	if !ok {
		writeError(w, http.StatusBadRequest, "a path outside the workspace")
		return
	}
	root, err := os.OpenRoot(ws.Path)
	if err != nil {
		writeError(w, http.StatusConflict, "the workspace's folder is missing: "+display(ws.Path))
		return
	}
	defer root.Close()
	dir, err := root.Open(relative)
	if err != nil {
		writeError(w, http.StatusNotFound, "no such folder: "+relative)
		return
	}
	entries, err := dir.ReadDir(maxTreeEntries + 1)
	dir.Close()
	if err != nil && !errors.Is(err, io.EOF) {
		writeError(w, http.StatusBadRequest, "not a folder: "+relative)
		return
	}
	truncated := len(entries) > maxTreeEntries
	entries = entries[:min(len(entries), maxTreeEntries)]
	known := s.knownSet(r.Context(), ws)
	changes := s.changes(r.Context(), ws)
	changes.mu.Lock()
	defer changes.mu.Unlock()
	prefix := ""
	if relative != "." {
		prefix = relative + "/"
	}
	out := make([]treeEntry, 0, len(entries))
	for _, entry := range entries {
		name := entry.Name()
		if name == ".git" || name == ".DS_Store" {
			continue
		}
		item := treeEntry{Name: name}
		full := prefix + name
		if entry.Type()&fs.ModeSymlink != 0 {
			item.Link = true
			// A link is followed within the workspace only.
			if info, err := root.Stat(full); err == nil {
				item.Dir = info.IsDir()
				if !item.Dir {
					item.Size = info.Size()
				}
			} else {
				item.Broken = true
			}
		} else if entry.IsDir() {
			item.Dir = true
		} else if info, err := entry.Info(); err == nil {
			item.Size = info.Size()
		}
		if known != nil {
			key := full
			if item.Dir {
				key += "/"
			}
			_, found := known[key]
			item.Ignored = !found
		}
		if item.Dir {
			item.Changed = changes.dirs[full]
		} else {
			item.Git = changes.files[full]
		}
		out = append(out, item)
	}
	slices.SortFunc(out, func(a, b treeEntry) int {
		if a.Dir != b.Dir {
			if a.Dir {
				return -1
			}
			return 1
		}
		return naturalCompare(strings.ToLower(a.Name), strings.ToLower(b.Name))
	})
	writeJSON(w, http.StatusOK, map[string]any{"path": relative, "entries": out, "truncated": truncated})
}

// naturalCompare orders names with the numbers in them by their value:
// file2 before file10.
func naturalCompare(a, b string) int {
	for a != "" && b != "" {
		if isDigit(a[0]) && isDigit(b[0]) {
			i, j := digits(a), digits(b)
			x, y := strings.TrimLeft(a[:i], "0"), strings.TrimLeft(b[:j], "0")
			if len(x) != len(y) {
				return len(x) - len(y)
			}
			if c := strings.Compare(x, y); c != 0 {
				return c
			}
			a, b = a[i:], b[j:]
			continue
		}
		if a[0] != b[0] {
			return int(a[0]) - int(b[0])
		}
		a, b = a[1:], b[1:]
	}
	return len(a) - len(b)
}

func isDigit(c byte) bool { return c >= '0' && c <= '9' }

func digits(s string) int {
	i := 0
	for i < len(s) && isDigit(s[i]) {
		i++
	}
	return i
}

// fileVersion names the state of a file: it changes when the file does.
func fileVersion(info fs.FileInfo) string {
	return fmt.Sprintf("%x-%x", info.Size(), info.ModTime().UnixNano())
}

// handleFile answers ?path= with the file: its text and highlighting, or
// that it is a picture, binary or too large. ?tokens=1&version= answers
// with the highlighting of the whole text, once it is done.
func (s *server) handleFile(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	query := r.URL.Query()
	relative, ok := workspaceRelative(query.Get("path"))
	if !ok || relative == "." {
		writeError(w, http.StatusBadRequest, "a path outside the workspace")
		return
	}
	root, err := os.OpenRoot(ws.Path)
	if err != nil {
		writeError(w, http.StatusConflict, "the workspace's folder is missing: "+display(ws.Path))
		return
	}
	defer root.Close()
	file, err := root.Open(relative)
	if err != nil {
		writeError(w, http.StatusNotFound, "no such file: "+relative)
		return
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil || info.IsDir() {
		writeError(w, http.StatusBadRequest, "not a file: "+relative)
		return
	}
	version := fileVersion(info)
	view := map[string]any{"path": relative, "size": info.Size(), "modified": info.ModTime(), "version": version}
	if media, ok := pictures[strings.ToLower(path.Ext(relative))]; ok {
		view["image"] = media
		writeJSON(w, http.StatusOK, view)
		return
	}
	if info.Size() > maxViewed {
		view["too_large"] = true
		writeJSON(w, http.StatusOK, view)
		return
	}
	data, err := io.ReadAll(io.LimitReader(file, maxViewed+1))
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if binary(data) {
		view["binary"] = true
		writeJSON(w, http.StatusOK, view)
		return
	}
	text := string(data)
	if !utf8.ValidString(text) {
		text = strings.ToValidUTF8(text, "\uFFFD")
	}
	key := ws.ID + "\x00" + relative + "\x00" + version
	if query.Get("tokens") != "" {
		if query.Get("version") != version {
			writeError(w, http.StatusConflict, "the file changed")
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 2*time.Minute)
		defer cancel()
		result, err := s.explorer.highlights.Wait(ctx, key, relative, text)
		if err != nil {
			writeError(w, http.StatusServiceUnavailable, err.Error())
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"version": version, "language": result.Language, "runs": result.Runs, "complete": true})
		return
	}
	result := s.explorer.highlights.Get(key, relative, text, highlightBudget)
	view["text"] = text
	view["language"] = result.Language
	view["runs"] = result.Runs
	view["complete"] = result.Complete
	view["classes"] = highlight.Classes
	writeJSON(w, http.StatusOK, view)
}

// binary reports whether data is not text: it has NULs, or is not UTF-8,
// at its start.
func binary(data []byte) bool {
	head := data[:min(len(data), 8<<10)]
	if bytes.IndexByte(head, 0) >= 0 {
		return true
	}
	// The last rune of the start may be cut in two.
	for cut := 0; cut < utf8.UTFMax && len(head) > 0; cut++ {
		if utf8.Valid(head) {
			return false
		}
		head = head[:len(head)-1]
	}
	return !utf8.Valid(head)
}

// handleRawFile serves a picture of the workspace for the file viewer.
func (s *server) handleRawFile(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	relative, ok := workspaceRelative(r.URL.Query().Get("path"))
	media, picture := pictures[strings.ToLower(path.Ext(relative))]
	if !ok || relative == "." || !picture {
		writeError(w, http.StatusBadRequest, "only pictures are served")
		return
	}
	root, err := os.OpenRoot(ws.Path)
	if err != nil {
		writeError(w, http.StatusConflict, "the workspace's folder is missing")
		return
	}
	defer root.Close()
	file, err := root.Open(relative)
	if err != nil {
		writeError(w, http.StatusNotFound, "no such file: "+relative)
		return
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil || info.IsDir() || info.Size() > maxPicture {
		writeError(w, http.StatusBadRequest, "not a picture to show: "+relative)
		return
	}
	w.Header().Set("Content-Type", media)
	w.Header().Set("Cache-Control", "no-cache")
	http.ServeContent(w, r, path.Base(relative), info.ModTime(), file)
}
