package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"net"
	"net/http"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Finished runs stay addressable for a while so a reconnecting browser can
// still read their tail and final state.
const runRetention = 10 * time.Minute

type server struct {
	opt        options
	ctx        context.Context // server lifetime; runners are interrupted when it ends
	hosts      map[string]bool
	anyHost    bool
	workspaces *workspaces

	settingsMu sync.Mutex // serializes settings writes

	// gateway is the accounts gateway (accounts.go), nil when it is off;
	// gatewayErr says why it could not be set up.
	gateway    *accounts.Host
	gatewayErr string
	// gatewayGone is closed once the gateway stopped and its publication
	// was withdrawn.
	gatewayGone chan struct{}

	// models is the model list an endpoint gave last (models.go).
	models catalogCache

	// assets are the page and the built-in plugins (assets.go); watch
	// follows plugins for the pages open (pluginevents.go).
	assets *assets
	watch  *pluginWatch
	// rebuild builds the server anew as its Go code changes (rebuild.go),
	// nil when it does not run from a checkout or -rebuild is off; instance
	// tells pages one run of the server from another.
	rebuild  *rebuilder
	instance string
	started  time.Time // when this run of the server started

	mu     sync.Mutex
	runs   map[string]*run           // by run ID, including recently finished runs
	active map[string]*run           // by activeKey, unfinished runs only
	queues map[string]*cockpit.Queue // by activeKey; see queue.go
	closed bool
	pumps  sync.WaitGroup
}

// activeKey names a session across workspaces, which may reuse session IDs.
func activeKey(ws *workspace, sessionID string) string { return ws.ID + "/" + sessionID }

func newServer(ctx context.Context, o options, addr net.Addr) *server {
	file := ""
	if o.SettingsFile != "" {
		file = filepath.Join(filepath.Dir(o.SettingsFile), "workspaces.json")
	}
	s := &server{
		opt: o, ctx: ctx,
		hosts:      map[string]bool{"localhost": true, "127.0.0.1": true, "::1": true},
		workspaces: newWorkspaces(o.Options, file),
		runs:       make(map[string]*run),
		active:     make(map[string]*run),
		queues:     make(map[string]*cockpit.Queue),
		assets:     o.assets,
		watch:      newPluginWatch(),
	}
	if s.assets == nil {
		s.assets = compiledAssets()
	}
	s.started = time.Now()
	s.instance = fmt.Sprintf("%x", s.started.UnixNano())
	if o.rebuild {
		s.rebuild = newRebuilder(s.assets)
	}
	// A page on another site can resolve its own name to this address (DNS
	// rebinding), so only names that really denote this server are accepted.
	// Bound to every interface, any IP literal is fine too: rebinding needs a
	// name.
	if tcp, ok := addr.(*net.TCPAddr); ok {
		if tcp.IP.IsUnspecified() {
			s.anyHost = true
		} else {
			s.hosts[tcp.IP.String()] = true
		}
	}
	for _, host := range o.allowedHosts {
		s.hosts[host] = true
	}
	for _, ws := range s.workspaces.all() {
		go ws.warm(ctx)
	}
	s.startGateway(ctx)
	return s
}

func (s *server) handler() http.Handler {
	mux := http.NewServeMux()
	files := http.FileServerFS(s.assets.static)
	mux.Handle("GET /", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-cache")
		files.ServeHTTP(w, r)
	}))
	mux.HandleFunc("GET /api/config", s.handleConfig)
	mux.HandleFunc("GET /api/settings", s.handleSettings)
	mux.HandleFunc("PUT /api/settings", s.handleSaveSettings)
	mux.HandleFunc("POST /api/settings/check", s.handleCheckSettings)
	mux.HandleFunc("GET /api/workspaces", s.handleWorkspaces)
	mux.HandleFunc("POST /api/workspaces", s.handleAddWorkspace)
	mux.HandleFunc("DELETE /api/workspaces/{ws}", s.handleRemoveWorkspace)
	mux.HandleFunc("GET /api/folders", s.handleFolders)
	mux.HandleFunc("GET /api/preferences", s.handlePreferences)
	mux.HandleFunc("PUT /api/preferences", s.handleSavePreferences)
	// Accounts of the gateway (accounts.go), and signing new ones in.
	mux.HandleFunc("GET /api/accounts", s.handleAccounts)
	mux.HandleFunc("GET /api/accounts/models", s.withGateway(s.handleGatewayModels))
	mux.HandleFunc("PUT /api/connection", s.handleSaveConnection)
	mux.HandleFunc("POST /api/connection/adopt", s.withGateway(s.handleAdoptConnection))
	mux.HandleFunc("POST /api/endpoints", s.withGateway(s.handleAddEndpoint))
	mux.HandleFunc("POST /api/endpoints/probe", s.withGateway(s.handleProbeEndpoint))
	mux.HandleFunc("PUT /api/endpoints/{id}", s.withGateway(s.handleUpdateEndpoint))
	mux.HandleFunc("PATCH /api/endpoints/{id}", s.withGateway(s.handleSwitchEndpoint))
	mux.HandleFunc("DELETE /api/endpoints/{id}", s.withGateway(s.handleRemoveEndpoint))
	mux.HandleFunc("POST /api/accounts/import", s.withGateway(s.handleImportAccount))
	mux.HandleFunc("GET /api/accounts/{name}/quota", s.withGateway(s.handleAccountQuota))
	mux.HandleFunc("POST /api/accounts/{name}/refresh", s.withGateway(s.handleRefreshAccount))
	mux.HandleFunc("PATCH /api/accounts/{name}", s.withGateway(s.handleUpdateAccount))
	mux.HandleFunc("DELETE /api/accounts/{name}", s.withGateway(s.handleRemoveAccount))
	mux.HandleFunc("POST /api/sign-ins", s.withGateway(s.handleStartSignIn))
	mux.HandleFunc("GET /api/sign-ins/{state}", s.withGateway(s.handleSignIn))
	mux.HandleFunc("POST /api/sign-ins/{state}/callback", s.withGateway(s.handleFinishSignIn))
	mux.HandleFunc("DELETE /api/sign-ins/{state}", s.withGateway(s.handleCancelSignIn))
	// Sessions and runs belong to a workspace. The routes without one act on
	// the workspace the server was started in.
	for _, prefix := range []string{"/api/w/{ws}", "/api"} {
		mux.HandleFunc("GET "+prefix+"/sessions", s.handleSessions)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}", s.handleSession)
		mux.HandleFunc("PATCH "+prefix+"/sessions/{id}", s.handleUpdateSession)
		mux.HandleFunc("DELETE "+prefix+"/sessions/{id}", s.handleDeleteSession)
		mux.HandleFunc("PUT "+prefix+"/pins", s.handleOrderPins)
		mux.HandleFunc("POST "+prefix+"/sessions/{id}/branch", s.handleBranch)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/export", s.handleExport)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/changes/{message}", s.handleChanges)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/changes/{message}/diff", s.handleChangeDiff)
		mux.HandleFunc("POST "+prefix+"/runs", s.handleStart)
		mux.HandleFunc("GET "+prefix+"/models", s.handleModels)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/queue", s.handleQueue)
		mux.HandleFunc("POST "+prefix+"/sessions/{id}/queue", s.handleEnqueue)
		mux.HandleFunc("DELETE "+prefix+"/sessions/{id}/queue", s.handleClearQueue)
		mux.HandleFunc("POST "+prefix+"/sessions/{id}/queue/pause", s.handlePauseQueue)
		mux.HandleFunc("POST "+prefix+"/sessions/{id}/queue/resume", s.handleResumeQueue)
		mux.HandleFunc("PATCH "+prefix+"/sessions/{id}/queue/{item}", s.handleUpdateQueued)
		mux.HandleFunc("DELETE "+prefix+"/sessions/{id}/queue/{item}", s.handleDropQueued)
		// The images of prompts, and those the agent looked at (images.go).
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/images/{message}/{n}", s.handlePromptImage)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/queue/{item}/images/{n}", s.handleQueuedImage)
		mux.HandleFunc("GET "+prefix+"/sessions/{id}/tools/{call}/image", s.handleToolImage)
		// Plugins (plugins.go), and the stream that says when they change
		// (pluginevents.go).
		mux.HandleFunc("GET "+prefix+"/plugins", s.handlePlugins)
		mux.HandleFunc("GET "+prefix+"/plugins/events", s.handlePluginEvents)
		mux.HandleFunc("PUT "+prefix+"/plugins/trust", s.handleTrustPlugins)
		mux.HandleFunc("PUT "+prefix+"/plugins/{name}/enabled", s.handleEnablePlugin)
		mux.HandleFunc("GET "+prefix+"/plugins/{name}/files/{path...}", s.handlePluginFile)
		mux.HandleFunc("GET "+prefix+"/plugins/{name}/v/{version}/{path...}", s.handlePluginFile)
	}
	mux.HandleFunc("GET /api/runs/{id}/events", s.handleEvents)
	mux.HandleFunc("POST /api/runs/{id}/cancel", s.handleCancel)
	notFound := func(w http.ResponseWriter, r *http.Request) { writeError(w, http.StatusNotFound, "not found") }
	for _, method := range []string{"GET", "POST", "PUT", "PATCH", "DELETE"} {
		mux.HandleFunc(method+" /api/", notFound)
	}

	// Every non-GET request runs commands, so cross-site browser requests are
	// rejected outright.
	protection := http.NewCrossOriginProtection()
	protection.SetDenyHandler(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		writeError(w, http.StatusForbidden, "cross-origin request rejected")
	}))
	return logRequests(s.secure(protection.Handler(mux)))
}

func (s *server) secure(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "no-referrer")
		h.Set("Cross-Origin-Opener-Policy", "same-origin")
		// Transcripts render model and tool output; nothing may execute from it.
		// Images pasted into a prompt show from blob: URLs until it is sent.
		h.Set("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; "+
			"connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
		if !s.hostAllowed(r.Host) {
			writeError(w, http.StatusForbidden, "unexpected Host header")
			return
		}
		next.ServeHTTP(w, r)
	})
}

func (s *server) hostAllowed(hostport string) bool {
	host := hostport
	if h, _, err := net.SplitHostPort(hostport); err == nil {
		host = h
	}
	host = strings.ToLower(strings.Trim(host, "[]"))
	return s.hosts[host] || s.anyHost && net.ParseIP(host) != nil
}

func (s *server) handleConfig(w http.ResponseWriter, r *http.Request) {
	settings, err := s.settings()
	startup := s.workspaces.startup()
	effort, saved := s.effort()
	config := map[string]any{
		"workspace":      startup.Path,
		"workspace_name": filepath.Base(startup.Path),
		"workspace_id":   startup.ID,
		"session_dir":    startup.opt.SessionDir,
		// The effort runs use unless a prompt asks for another; saved is set
		// when a cockpit chose it, rather than the server's flag.
		"thinking":        effort,
		"effort_saved":    saved,
		"thinking_levels": cockpit.ThinkingLevels,
		"settings":        s.opt.SettingsFile != "",
		"accounts":        s.gatewayView(),
	}
	if err != nil {
		config["settings_error"] = err.Error()
	}
	c := s.connection(settings, startup)
	config["connection"], config["model"], config["provider"] = c, c.Model, c.Provider
	list := s.workspaces.all()
	views := make([]workspaceView, 0, len(list))
	for _, ws := range list {
		views = append(views, s.viewOf(ws, settings))
	}
	config["workspaces"] = views
	writeJSON(w, http.StatusOK, config)
}

type sessionSummary struct {
	cockpit.SessionInfo
	RunID string `json:"run_id,omitempty"`
	// Queued is how many prompts wait for the session's agent; Paused, that
	// they wait for the user.
	Queued int  `json:"queued,omitempty"`
	Paused bool `json:"queue_paused,omitempty"`
}

// queued says what waits for a session's agent; the caller holds s.mu.
func (s *server) queued(ws *workspace, id string, summary *sessionSummary) {
	if q := s.queues[activeKey(ws, id)]; q != nil && len(q.Items) != 0 {
		summary.Queued, summary.Paused = len(q.Items), q.Paused
	}
}

func (s *server) handleSessions(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	list, err := cockpit.ListSessions(ws.opt.SessionDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	out := make([]sessionSummary, 0, len(list)+1)
	s.mu.Lock()
	listed := make(map[string]bool, len(list))
	for _, info := range list {
		listed[info.ID] = true
		summary := sessionSummary{SessionInfo: info}
		if current := s.attachable(ws, info.ID); current != nil {
			summary.RunID = current.id
			// A session rewound to before its first prompt has no title until
			// the runner persists the new one.
			if summary.Title == "" {
				summary.Title = current.title
			}
		}
		s.queued(ws, info.ID, &summary)
		out = append(out, summary)
	}
	// A brand-new session has no file until its runner creates one.
	var unlisted []sessionSummary
	for _, current := range s.active {
		if current.ws == ws && !listed[current.sessionID] && s.attachable(ws, current.sessionID) != nil {
			summary := sessionSummary{
				SessionInfo: cockpit.SessionInfo{ID: current.sessionID, Title: current.title, UpdatedAt: current.started},
				RunID:       current.id,
			}
			s.queued(ws, current.sessionID, &summary)
			unlisted = append(unlisted, summary)
		}
	}
	s.mu.Unlock()
	slices.SortFunc(unlisted, func(a, b sessionSummary) int { return b.UpdatedAt.Compare(a.UpdatedAt) })
	writeJSON(w, http.StatusOK, append(unlisted, out...))
}

// attachable returns the session's run once browsers can stream it: a run is
// reserved in active before its runner starts, and only then registered.
// The caller holds s.mu.
func (s *server) attachable(ws *workspace, sessionID string) *run {
	current := s.active[activeKey(ws, sessionID)]
	if current == nil || s.runs[current.id] != current {
		return nil
	}
	return current
}

type runSummary struct {
	ID       string    `json:"id"`
	Started  time.Time `json:"started_at"`
	Stopping bool      `json:"stopping"`
	Compact  bool      `json:"compact,omitempty"` // the run compacts the conversation
}

func (s *server) handleSession(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	if !cockpit.ValidSessionID(id) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	dir := ws.opt.SessionDir
	// Read the file before looking at runs: whatever a run appends after this
	// point reaches the browser through the run's event stream.
	tr, err := cockpit.LoadSession(dir, id)
	s.mu.Lock()
	current, reserved := s.attachable(ws, id), s.active[activeKey(ws, id)]
	var summary *runSummary
	if current != nil {
		summary = &runSummary{ID: current.id, Started: current.started, Stopping: current.isStopping(), Compact: current.compact}
	}
	queue := s.queueSnapshot(activeKey(ws, id))
	s.mu.Unlock()
	switch {
	case errors.Is(err, fs.ErrNotExist) && reserved != nil:
		tr = cockpit.NewTranscript()
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "session not found")
		return
	case err != nil:
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	// A terminal or another server may be running the session; browsers then
	// follow the file instead of a stream.
	external := reserved == nil && cockpit.SessionBusy(dir, id)
	meta := cockpit.LoadMeta(dir, id)
	writeJSON(w, http.StatusOK, map[string]any{
		"id": id, "workspace": ws.ID, "title": titleOf(dir, id, tr, reserved), "entries": tr.Entries, "usage": tr.Usage,
		"run": summary, "size": tr.Size, "external": external,
		"pinned": meta.Pinned, "queue": queue,
		// A title of the user's choosing; otherwise the title is the first
		// prompt and changes with it.
		"renamed":     meta.Title != "",
		"interrupted": reserved == nil && !external && tr.Interrupted(),
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	if err := json.MarshalWrite(w, v); err != nil {
		log.Printf("encode response: %v", err)
	}
}

func writeError(w http.ResponseWriter, status int, message string) {
	writeJSON(w, status, map[string]string{"error": message})
}

// logRequests logs what changes state, event streams and failures; the
// pages' routine polling and static assets are not worth the noise.
func logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		recorder := &statusRecorder{ResponseWriter: w, status: http.StatusOK}
		next.ServeHTTP(recorder, r)
		if r.Method == http.MethodGet && recorder.status < 400 && !strings.HasSuffix(r.URL.Path, "/events") {
			return
		}
		fmt.Printf("%s %-6s %-44s %d %s\n", start.Format("15:04:05"), r.Method, r.URL.Path, recorder.status,
			time.Since(start).Round(time.Millisecond))
	})
}

type statusRecorder struct {
	http.ResponseWriter
	status int
}

func (r *statusRecorder) WriteHeader(status int) {
	r.status = status
	r.ResponseWriter.WriteHeader(status)
}

// Unwrap lets http.ResponseController reach Flush for event streams.
func (r *statusRecorder) Unwrap() http.ResponseWriter { return r.ResponseWriter }
