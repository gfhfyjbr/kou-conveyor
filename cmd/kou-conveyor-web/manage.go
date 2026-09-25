package main

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"mime"
	"net/http"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// decodeJSON reads a small JSON request body into v and reports failures to
// the browser itself.
func decodeJSON(w http.ResponseWriter, r *http.Request, v any) bool {
	if media, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type")); media != "application/json" {
		writeError(w, http.StatusUnsupportedMediaType, "expected application/json")
		return false
	}
	if err := json.UnmarshalRead(io.LimitReader(r.Body, 64<<10), v); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return false
	}
	return true
}

// ---------------------------------------------------------------- settings

// settingsView is what browsers learn about the saved settings. The key never
// leaves the server; a hint is enough to recognize it.
type settingsView struct {
	Available bool        `json:"available"`
	Path      string      `json:"path,omitzero"`
	API       cockpit.API `json:"provider_type"`
	BaseURL   string      `json:"base_url"`
	Model     string      `json:"model"`
	KeySet    bool        `json:"api_key_set"`
	KeyHint   string      `json:"api_key_hint,omitzero"`
	Flag      string      `json:"provider_flag,omitzero"`
	// Error says why the saved settings could not be read; saving replaces them.
	Error     string                              `json:"error,omitzero"`
	Effective cockpit.Connection                  `json:"connection"`
	Defaults  map[cockpit.API]cockpit.APIDefaults `json:"defaults"`
	// Gateway is where the accounts gateway runs, if it does; GatewayInUse,
	// that these settings point runs at it.
	Gateway      string `json:"gateway,omitzero"`
	GatewayInUse bool   `json:"gateway_in_use,omitzero"`
}

func (s *server) settings() (cockpit.Settings, error) {
	if s.opt.SettingsFile == "" {
		return cockpit.Settings{}, nil
	}
	return cockpit.LoadSettings(s.opt.SettingsFile)
}

// connection is what a run in the workspace connects to: settings, or the
// environment with the workspace's .env file.
func (s *server) connection(settings cockpit.Settings, ws *workspace) cockpit.Connection {
	c := cockpit.ResolveConnection(s.opt.Provider, settings, cockpit.WorkspaceEnv(ws.Path))
	if s.opt.model != "" {
		c.Model = s.opt.model
	}
	return c
}

// queryWorkspace is the workspace a settings request asks about with ?ws=,
// by default the one the server was started in.
func (s *server) queryWorkspace(r *http.Request) *workspace {
	if found := s.workspaces.find(r.URL.Query().Get("ws")); found != nil {
		return found
	}
	return s.workspaces.startup()
}

func (s *server) settingsView(settings cockpit.Settings, ws *workspace) settingsView {
	view := settingsView{
		Available: s.opt.SettingsFile != "", Path: s.opt.SettingsFile,
		API: settings.API, BaseURL: settings.BaseURL, Model: settings.Model,
		KeySet: settings.APIKey != "", KeyHint: settings.KeyHint(),
		Flag: s.opt.Provider, Effective: s.connection(settings, ws),
		Defaults: map[cockpit.API]cockpit.APIDefaults{
			cockpit.APIResponses: cockpit.DefaultsFor(cockpit.APIResponses),
			cockpit.APIMessages:  cockpit.DefaultsFor(cockpit.APIMessages),
		},
	}
	if s.gateway != nil {
		view.Gateway = s.gateway.URL()
		view.GatewayInUse = settings.API != cockpit.APIEnvironment && s.gateway.Serves(settings.BaseURL)
	}
	return view
}

func (s *server) handleSettings(w http.ResponseWriter, r *http.Request) {
	settings, err := s.settings()
	view := s.settingsView(settings, s.queryWorkspace(r))
	if err != nil {
		// The form still opens, so the settings can be fixed from it.
		view.Error = err.Error()
	}
	writeJSON(w, http.StatusOK, view)
}

func (s *server) handleSaveSettings(w http.ResponseWriter, r *http.Request) {
	if s.opt.SettingsFile == "" {
		writeError(w, http.StatusConflict, "settings are unavailable: set KOU_CONVEYOR_CONFIG")
		return
	}
	var update cockpit.SettingsUpdate
	if !decodeJSON(w, r, &update) {
		return
	}
	s.settingsMu.Lock()
	defer s.settingsMu.Unlock()
	// Settings that cannot be read hold no key worth keeping; saving
	// replaces them.
	saved, _ := s.settings()
	next, err := saved.Update(update)
	if errors.Is(err, cockpit.ErrKeyRequired) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error(), "field": "api_key"})
		return
	}
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := cockpit.SaveSettings(s.opt.SettingsFile, next); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, s.settingsView(next, s.queryWorkspace(r)))
}

// handleCheckSettings checks the settings a form shows before they are
// saved. Like saving, it sends the saved key only to the endpoint it belongs to.
func (s *server) handleCheckSettings(w http.ResponseWriter, r *http.Request) {
	var update cockpit.SettingsUpdate
	if !decodeJSON(w, r, &update) {
		return
	}
	saved, _ := s.settings()
	draft, err := saved.Update(update)
	if errors.Is(err, cockpit.ErrKeyRequired) {
		writeJSON(w, http.StatusOK, cockpit.Check{Message: "Enter the API key for this endpoint"})
		return
	}
	if err != nil {
		writeJSON(w, http.StatusOK, cockpit.Check{Message: err.Error()})
		return
	}
	// The environment differs between workspaces by their .env files.
	c := cockpit.ResolveConnection("", draft, cockpit.WorkspaceEnv(s.queryWorkspace(r).Path))
	writeJSON(w, http.StatusOK, cockpit.CheckConnection(r.Context(), c))
}

// ---------------------------------------------------------------- preferences

// effort is what runs think with unless a prompt asks otherwise: the level
// last chosen in either cockpit, else the server's -thinking-level.
func (s *server) effort() (level string, saved bool) {
	if p := cockpit.LoadPreferences(cockpit.PreferencesPath(s.opt.SettingsFile)); p.Effort != "" {
		return p.Effort, true
	}
	return s.opt.thinking, false
}

func (s *server) handlePreferences(w http.ResponseWriter, r *http.Request) {
	effort, saved := s.effort()
	writeJSON(w, http.StatusOK, map[string]any{"effort": effort, "saved": saved})
}

// handleSavePreferences records the effort for both cockpits: the terminal
// one picks it up too.
func (s *server) handleSavePreferences(w http.ResponseWriter, r *http.Request) {
	var change struct {
		Effort string `json:"effort"`
	}
	if !decodeJSON(w, r, &change) {
		return
	}
	if !cockpit.ValidThinkingLevel(change.Effort) {
		writeError(w, http.StatusBadRequest, "effort must be low, medium, high, xhigh or max")
		return
	}
	path := cockpit.PreferencesPath(s.opt.SettingsFile)
	if path == "" {
		writeError(w, http.StatusConflict, "preferences are unavailable: set KOU_CONVEYOR_CONFIG")
		return
	}
	s.settingsMu.Lock()
	err := cockpit.SaveEffort(path, change.Effort)
	s.settingsMu.Unlock()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"effort": change.Effort, "saved": true})
}

// ---------------------------------------------------------------- sessions

func (s *server) sessionRunning(ws *workspace, id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.active[activeKey(ws, id)] != nil
}

func sessionError(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "session not found")
	case errors.Is(err, cockpit.ErrSessionBusy), errors.Is(err, cockpit.ErrBranchRunning):
		writeError(w, http.StatusConflict, "this session is running; stop the run first")
	default:
		writeError(w, http.StatusInternalServerError, err.Error())
	}
}

func (s *server) handleUpdateSession(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	var change struct {
		Title  *string `json:"title,omitzero"`
		Pinned *bool   `json:"pinned,omitzero"`
	}
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	if !decodeJSON(w, r, &change) {
		return
	}
	if change.Title != nil {
		if err := cockpit.RenameSession(ws.opt.SessionDir, id, *change.Title); err != nil {
			sessionError(w, err)
			return
		}
	}
	if change.Pinned != nil {
		if err := cockpit.PinSession(ws.opt.SessionDir, id, *change.Pinned); err != nil {
			sessionError(w, err)
			return
		}
	}
	meta := cockpit.LoadMeta(ws.opt.SessionDir, id)
	writeJSON(w, http.StatusOK, map[string]any{"id": id, "title": meta.Title, "pinned": meta.Pinned})
}

// handleOrderPins puts the workspace's pinned sessions in the order the
// user dragged them into: {"order": [session IDs]}.
func (s *server) handleOrderPins(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	var body struct {
		Order []string `json:"order"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	if len(body.Order) > 10000 {
		writeError(w, http.StatusBadRequest, "too many sessions")
		return
	}
	if err := cockpit.OrderPins(ws.opt.SessionDir, body.Order); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *server) handleDeleteSession(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	if s.sessionRunning(ws, id) {
		sessionError(w, cockpit.ErrSessionBusy)
		return
	}
	if err := cockpit.DeleteSession(ws.opt.SessionDir, id); err != nil {
		sessionError(w, err)
		return
	}
	s.mu.Lock()
	delete(s.queues, activeKey(ws, id)) // nothing is left to run it in
	s.mu.Unlock()
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *server) handleBranch(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	var request struct {
		MessageID string `json:"message_id"`
	}
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	if !decodeJSON(w, r, &request) {
		return
	}
	if s.sessionRunning(ws, id) {
		sessionError(w, cockpit.ErrBranchRunning)
		return
	}
	branch, err := cockpit.BranchSession(r.Context(), ws.opt.SessionDir, id, request.MessageID)
	if err != nil {
		sessionError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, branch)
}

func (s *server) handleExport(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	tr, err := cockpit.LoadSession(ws.opt.SessionDir, id)
	if err != nil {
		sessionError(w, err)
		return
	}
	title := cockpit.SessionTitle(ws.opt.SessionDir, id, tr)
	name := cockpit.ExportName(title, id)
	w.Header().Set("Content-Type", "text/markdown; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Content-Disposition", mime.FormatMediaType("attachment", map[string]string{"filename": name}))
	fmt.Fprint(w, cockpit.ExportMarkdown(title, id, tr, time.Now()))
}

// ---------------------------------------------------------------- changes

// runningMessage is the prompt the session's run is running, or "".
func (s *server) runningMessage(ws *workspace, id string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if current := s.active[activeKey(ws, id)]; current != nil {
		return current.message
	}
	return ""
}

// exchangeOf reads the path values of a changes request.
func exchangeOf(w http.ResponseWriter, r *http.Request) (id, message string, ok bool) {
	id, message = r.PathValue("id"), r.PathValue("message")
	if !cockpit.ValidSessionID(id) || message == "" || len(message) > 200 {
		writeError(w, http.StatusNotFound, "not found")
		return "", "", false
	}
	return id, message, true
}

// handleChanges lists the files a prompt's run changed.
func (s *server) handleChanges(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id, message, ok := exchangeOf(w, r)
	if !ok {
		return
	}
	live := s.runningMessage(ws, id) == message
	ex, recorded := ws.changes().Exchange(id, message)
	if !recorded {
		reason := "No changes were recorded for this prompt."
		if err := ws.changes().Available(); err != nil {
			reason = cockpit.Sentence(err.Error())
		}
		writeJSON(w, http.StatusOK, map[string]any{"message": message, "available": false, "live": live, "reason": reason})
		return
	}
	files, err := ws.changes().Files(r.Context(), ex.Before, ex.After)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if files == nil {
		files = []cockpit.FileChange{}
	}
	added, removed := 0, 0
	for _, f := range files {
		added += f.Added
		removed += f.Removed
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"message": message, "available": true, "live": live, "tree": ex.After, "latest": ex.Latest,
		"files": files, "stat": map[string]int{"files": len(files), "added": added, "removed": removed},
	})
}

// handleChangeDiff returns the diff of one file a prompt's run changed.
func (s *server) handleChangeDiff(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id, message, ok := exchangeOf(w, r)
	if !ok {
		return
	}
	path, old := r.URL.Query().Get("path"), r.URL.Query().Get("old")
	ex, recorded := ws.changes().Exchange(id, message)
	if !recorded || path == "" {
		writeError(w, http.StatusNotFound, "no such change")
		return
	}
	patch, truncated, err := ws.changes().Diff(r.Context(), ex.Before, ex.After, path, old)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"path": path, "tree": ex.After, "patch": patch, "truncated": truncated})
}

// titleOf is the display title of a session: the user's, else the reserved
// run's prompt for a session the runner has not written yet.
func titleOf(dir, id string, tr *cockpit.Transcript, reserved *run) string {
	title := cockpit.SessionTitle(dir, id, tr)
	if strings.TrimSpace(title) == "" && reserved != nil {
		title = reserved.title
	}
	return title
}
