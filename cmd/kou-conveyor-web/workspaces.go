package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// workspace is a folder the cockpit runs agents in. Each keeps its own
// sessions: <folder>/.harness/sessions, or -session-directory for the folder
// the server was started with.
type workspace struct {
	ID      string
	Path    string // what the runner works in
	key     string // Path with symlinks resolved, to recognize a folder added twice
	Startup bool   // the -workspace folder; always listed first
	opt     cockpit.Options

	changesOnce sync.Once
	changesOf   *cockpit.Changes
}

// changes records what runs change in the workspace.
func (ws *workspace) changes() *cockpit.Changes {
	ws.changesOnce.Do(func() { ws.changesOf = cockpit.NewChanges(ws.Path, ws.opt.SessionDir, ws.opt.LogDir) })
	return ws.changesOf
}

// warm takes a snapshot of a workspace that has sessions, so that the one
// a run starts from is quick. Workspaces never run in are left alone.
func (ws *workspace) warm(ctx context.Context) {
	if _, err := os.Stat(ws.opt.SessionDir); err != nil {
		return
	}
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()
	_, _ = ws.changes().Snapshot(ctx)
}

// workspaceView is what browsers see of a workspace.
type workspaceView struct {
	ID         string             `json:"id"`
	Name       string             `json:"name"`
	Path       string             `json:"path"`
	Display    string             `json:"display"` // Path with ~ for the home directory
	Startup    bool               `json:"startup,omitzero"`
	Missing    bool               `json:"missing,omitzero"`
	Running    int                `json:"running,omitzero"`
	Connection cockpit.Connection `json:"connection"`
}

// workspaces is the list browsers switch between. Folders added from a
// browser persist in a file beside the settings, so every server started
// by the user offers them.
type workspaces struct {
	mu   sync.Mutex
	list []*workspace
	file string // "" keeps added folders for this server's lifetime only
	base cockpit.Options
}

type workspacesFile struct {
	Workspaces []struct {
		Path string `json:"path"`
	} `json:"workspaces"`
}

func newWorkspaces(base cockpit.Options, file string) *workspaces {
	w := &workspaces{file: file, base: base}
	startup := w.make(base.Workspace)
	startup.Startup = true
	startup.opt.SessionDir = base.SessionDir
	w.list = []*workspace{startup}
	if file == "" {
		return w
	}
	var saved workspacesFile
	if data, err := os.ReadFile(file); err == nil {
		_ = json.Unmarshal(data, &saved)
	}
	for _, entry := range saved.Workspaces {
		// Folders that went away stay listed, marked missing, until removed.
		if path := filepath.Clean(entry.Path); filepath.IsAbs(path) {
			w.insert(w.make(path))
		}
	}
	return w
}

// make describes a folder with the server's options and its own sessions.
// The ID comes from the path as given, so it holds even after the folder is
// gone; the resolved path only recognizes one folder reached two ways.
func (w *workspaces) make(path string) *workspace {
	path = filepath.Clean(path)
	key := path
	if resolved, err := filepath.EvalSymlinks(path); err == nil {
		key = resolved
	}
	opt := w.base
	opt.Workspace = path
	opt.SessionDir = filepath.Join(path, ".harness", "sessions")
	return &workspace{ID: workspaceID(path), Path: path, key: key, opt: opt}
}

// insert adds a workspace unless its folder is listed already, and returns
// the listed one. The caller holds w.mu or owns w.
func (w *workspaces) insert(ws *workspace) (*workspace, bool) {
	for _, listed := range w.list {
		if listed.key == ws.key || listed.ID == ws.ID {
			return listed, false
		}
	}
	w.list = append(w.list, ws)
	return ws, true
}

// workspaceID is readable in URLs and stable for a path across restarts.
func workspaceID(path string) string {
	var slug strings.Builder
	for _, r := range strings.ToLower(filepath.Base(path)) {
		if slug.Len() >= 24 {
			break
		}
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			slug.WriteRune(r)
		case slug.Len() > 0 && !strings.HasSuffix(slug.String(), "-"):
			slug.WriteByte('-')
		}
	}
	name := strings.Trim(slug.String(), "-")
	if name == "" {
		name = "workspace"
	}
	sum := sha256.Sum256([]byte(path))
	return name + "-" + hex.EncodeToString(sum[:3])
}

func (w *workspaces) all() []*workspace {
	w.mu.Lock()
	defer w.mu.Unlock()
	return slices.Clone(w.list)
}

func (w *workspaces) startup() *workspace {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.list[0]
}

func (w *workspaces) find(id string) *workspace {
	w.mu.Lock()
	defer w.mu.Unlock()
	for _, ws := range w.list {
		if ws.ID == id {
			return ws
		}
	}
	return nil
}

// add lists a folder and reports whether it was new.
func (w *workspaces) add(raw string) (*workspace, bool, error) {
	path, err := resolveFolder(raw)
	if err != nil {
		return nil, false, err
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	ws, added := w.insert(w.make(path))
	if !added {
		return ws, false, nil
	}
	if err := w.save(); err != nil {
		w.list = w.list[:len(w.list)-1]
		return nil, false, err
	}
	return ws, true, nil
}

var errStartupWorkspace = errors.New("the server was started in this workspace; it stays listed")

func (w *workspaces) remove(id string) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	index := slices.IndexFunc(w.list, func(ws *workspace) bool { return ws.ID == id })
	switch {
	case index < 0:
		return fs.ErrNotExist
	case w.list[index].Startup:
		return errStartupWorkspace
	}
	removed := w.list[index]
	w.list = slices.Delete(w.list, index, index+1)
	if err := w.save(); err != nil {
		w.list = slices.Insert(w.list, index, removed)
		return err
	}
	return nil
}

// save writes the folders added from browsers. The caller holds w.mu.
func (w *workspaces) save() error {
	if w.file == "" {
		return nil
	}
	var saved workspacesFile
	for _, ws := range w.list[1:] {
		saved.Workspaces = append(saved.Workspaces, struct {
			Path string `json:"path"`
		}{ws.Path})
	}
	data, err := json.Marshal(saved, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(w.file), 0o700); err != nil {
		return fmt.Errorf("save workspaces: %w", err)
	}
	temp, err := os.CreateTemp(filepath.Dir(w.file), ".workspaces-*.json")
	if err != nil {
		return fmt.Errorf("save workspaces: %w", err)
	}
	defer os.Remove(temp.Name())
	_, err = temp.Write(append(data, '\n'))
	if closeErr := temp.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(temp.Name(), w.file)
	}
	if err != nil {
		return fmt.Errorf("save workspaces: %w", err)
	}
	return nil
}

// resolveFolder accepts an absolute path, or one under ~, to an existing
// folder. Relative paths are refused: a browser cannot know what the
// server's working directory is.
func resolveFolder(raw string) (string, error) {
	path, err := expandHome(strings.TrimSpace(raw))
	switch {
	case err != nil:
		return "", err
	case path == "":
		return "", errors.New("enter the folder's path")
	case !filepath.IsAbs(path):
		return "", fmt.Errorf("%q is not an absolute path; start it with / or ~/", raw)
	}
	path = filepath.Clean(path)
	info, err := os.Stat(path)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		return "", fmt.Errorf("there is no folder at %s", path)
	case err != nil:
		return "", err
	case !info.IsDir():
		return "", fmt.Errorf("%s is a file, not a folder", path)
	case filepath.Dir(path) == path:
		return "", errors.New("the root folder cannot be a workspace")
	}
	return path, nil
}

func expandHome(path string) (string, error) {
	if path != "~" && !strings.HasPrefix(path, "~/") {
		return path, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("cannot expand ~: %w", err)
	}
	return filepath.Join(home, strings.TrimPrefix(path[1:], "/")), nil
}

// display shortens a path under the home directory to ~/…
func display(path string) string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return path
	}
	if path == home {
		return "~"
	}
	if rest, ok := strings.CutPrefix(path, home+string(filepath.Separator)); ok {
		return "~/" + filepath.ToSlash(rest)
	}
	return path
}

// ---------------------------------------------------------------- handlers

// workspaceOf is the workspace a request's path names; routes from before
// there were several workspaces act on the first one.
func (s *server) workspaceOf(w http.ResponseWriter, r *http.Request) *workspace {
	id := r.PathValue("ws")
	if id == "" {
		return s.workspaces.startup()
	}
	if ws := s.workspaces.find(id); ws != nil {
		return ws
	}
	writeError(w, http.StatusNotFound, "workspace not found")
	return nil
}

func (s *server) viewOf(ws *workspace, settings cockpit.Settings) workspaceView {
	_, err := os.Stat(ws.Path)
	view := workspaceView{
		ID: ws.ID, Name: filepath.Base(ws.Path), Path: ws.Path, Display: display(ws.Path),
		Startup: ws.Startup, Missing: err != nil, Connection: s.connection(settings, ws),
	}
	s.mu.Lock()
	for _, current := range s.active {
		if current.ws == ws {
			view.Running++
		}
	}
	s.mu.Unlock()
	return view
}

func (s *server) handleWorkspaces(w http.ResponseWriter, r *http.Request) {
	settings, _ := s.settings()
	list := s.workspaces.all()
	views := make([]workspaceView, 0, len(list))
	for _, ws := range list {
		views = append(views, s.viewOf(ws, settings))
	}
	writeJSON(w, http.StatusOK, views)
}

func (s *server) handleAddWorkspace(w http.ResponseWriter, r *http.Request) {
	var request struct {
		Path string `json:"path"`
	}
	if !decodeJSON(w, r, &request) {
		return
	}
	ws, added, err := s.workspaces.add(request.Path)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if added {
		go ws.warm(s.ctx)
	}
	settings, _ := s.settings()
	status := http.StatusOK
	if added {
		status = http.StatusCreated
	}
	writeJSON(w, status, s.viewOf(ws, settings))
}

func (s *server) handleRemoveWorkspace(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	s.mu.Lock()
	running := false
	for _, current := range s.active {
		running = running || current.ws == ws
	}
	s.mu.Unlock()
	if running {
		writeError(w, http.StatusConflict, "a run is in progress in this workspace; stop it first")
		return
	}
	switch err := s.workspaces.remove(ws.ID); {
	case errors.Is(err, errStartupWorkspace):
		writeError(w, http.StatusConflict, err.Error())
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "workspace not found")
	case err != nil:
		writeError(w, http.StatusInternalServerError, err.Error())
	default:
		writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
	}
}

// handleFolders completes a folder path as it is typed.
func (s *server) handleFolders(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, completeFolders(r.URL.Query().Get("prefix"), 40))
}

// completeFolders lists the folders a partly typed path can continue with,
// written the way it was typed: with ~ if it started with ~.
func completeFolders(typed string, limit int) []string {
	expanded, err := expandHome(typed)
	if err != nil || !filepath.IsAbs(expanded) {
		return []string{}
	}
	// What was typed decides whether it names a folder to look inside ("~",
	// or a trailing slash) or the start of a name; expanding ~ drops the slash.
	dir, part := expanded, ""
	if typed != "~" && !strings.HasSuffix(typed, "/") {
		dir, part = filepath.Dir(expanded), filepath.Base(expanded)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return []string{}
	}
	prefix := strings.TrimSuffix(typed, part)
	if typed == "~" {
		prefix = "~/"
	}
	folders := []string{}
	for _, entry := range entries {
		name := entry.Name()
		if strings.HasPrefix(name, ".") && !strings.HasPrefix(part, ".") {
			continue
		}
		if !strings.HasPrefix(strings.ToLower(name), strings.ToLower(part)) {
			continue
		}
		if !entry.IsDir() {
			// A link to a folder is as good as the folder.
			if info, err := os.Stat(filepath.Join(dir, name)); err != nil || !info.IsDir() {
				continue
			}
		}
		folders = append(folders, prefix+name)
		if len(folders) == limit {
			break
		}
	}
	slices.Sort(folders)
	return folders
}
