package canvas

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"
	"time"
	"unicode"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/worktree"
)

// The canvas's HTTP API. The pages' routes sit under both prefixes of the
// cockpit's, /api/w/{ws} and /api (the workspace the server started in):
//
//	GET    {p}/canvases                              the workspace's canvases
//	POST   {p}/canvases                              {id, title, template}: make one, or build a template on it
//	GET    {p}/canvases/{id}                         a snapshot of one
//	PATCH  {p}/canvases/{id}                         {title, live, settings}
//	DELETE {p}/canvases/{id}                         ?keep_terminals=1&keep_sessions=1
//	POST   {p}/canvases/{id}/ops                     a batch of operations (ops.go)
//	GET    {p}/canvases/{id}/events                  its events (hub.go)
//	POST   {p}/canvases/{id}/nodes/{node}/send       {text, submit, keys, deliver}
//	GET    {p}/canvases/{id}/nodes/{node}/read       ?what=screen|tail|output|answer&lines=
//	GET    {p}/canvases/{id}/nodes/{node}/transcript ?tail=40: an agent's latest entries
//	POST   {p}/canvases/{id}/nodes/{node}/restart    {resume}
//	POST   {p}/canvases/{id}/nodes/{node}/stop       stop what it does now
//	POST   {p}/canvases/{id}/nodes/{node}/fire       {text}: a manual source's button
//	POST   {p}/canvases/{id}/nodes/{node}/token      void its tokens
//	DELETE {p}/canvases/{id}/nodes/{node}/worktree   remove its worktree, when clean
//	GET    {p}/canvases/{id}/messages/{msg}          a message, whole
//	POST   {p}/canvases/{id}/messages/{msg}/approve  {text}: let it go, its text edited or not
//	DELETE {p}/canvases/{id}/messages/{msg}          drop it
//	POST   {p}/canvases/{id}/foreman                 {text}: ask the canvas's foreman
//	POST   {p}/canvases/{id}/template                {name, description}: save it as a template
//	GET    {p}/canvas/kinds                          the presets of nodes, with their schemas
//	GET    {p}/canvas/templates                      the templates to start from
//	GET    /api/canvas/live                          the live canvases, all workspaces'
//	POST   /api/canvas/pause                         pause them all
//
// The programs of the nodes reach their own with the token of their node
// (token.go) as a bearer:
//
//	GET  /api/canvas/self                  the node, its canvas, its access, its brief
//	GET  /api/canvas/self/launch           ?resume=1: how kou-canvas launch starts its harness
//	GET  /api/canvas/view                  ?detail=full: the canvas as the agent sees it (observe)
//	POST /api/canvas/ops                   a batch of operations (build)
//	POST /api/canvas/nodes/{node}/send     as the pages' (talk)
//	GET  /api/canvas/nodes/{node}/read     as the pages', and ?wait=idle|output|exit&timeout= (observe)
//	GET  /api/canvas/nodes/{node}/wait     ?until=idle|output|exit&timeout=&after= (observe)
//	POST /api/canvas/emit                  {port, title, text, data, status, detail, agent_session}: its node's own
//	POST /api/canvas/hooks/{hook}          a webhook source's address: its secret is the token

// maxRequest bounds a request's body.
const maxRequest = 4 << 20

// Register adds the canvas's routes to a server's; a nil engine adds none.
func (e *Engine) Register(mux *http.ServeMux) {
	if e == nil {
		return
	}
	for _, p := range []string{"/api/w/{ws}", "/api"} {
		mux.HandleFunc("GET "+p+"/canvases", e.handleList)
		mux.HandleFunc("POST "+p+"/canvases", e.handleCreate)
		mux.HandleFunc("GET "+p+"/canvases/{id}", e.handleGet)
		mux.HandleFunc("PATCH "+p+"/canvases/{id}", e.handlePatch)
		mux.HandleFunc("DELETE "+p+"/canvases/{id}", e.handleDelete)
		mux.HandleFunc("POST "+p+"/canvases/{id}/ops", e.handleOps)
		mux.HandleFunc("GET "+p+"/canvases/{id}/events", e.handleEvents)
		mux.HandleFunc("POST "+p+"/canvases/{id}/nodes/{node}/send", e.handleSend)
		mux.HandleFunc("GET "+p+"/canvases/{id}/nodes/{node}/read", e.handleRead)
		mux.HandleFunc("GET "+p+"/canvases/{id}/nodes/{node}/transcript", e.handleTranscript)
		mux.HandleFunc("POST "+p+"/canvases/{id}/nodes/{node}/restart", e.handleRestart)
		mux.HandleFunc("POST "+p+"/canvases/{id}/nodes/{node}/stop", e.handleStop)
		mux.HandleFunc("POST "+p+"/canvases/{id}/nodes/{node}/fire", e.handleFire)
		mux.HandleFunc("POST "+p+"/canvases/{id}/nodes/{node}/token", e.handleRotate)
		mux.HandleFunc("DELETE "+p+"/canvases/{id}/nodes/{node}/worktree", e.handleRemoveWorktree)
		mux.HandleFunc("GET "+p+"/canvases/{id}/messages/{msg}", e.handleMessage)
		mux.HandleFunc("POST "+p+"/canvases/{id}/messages/{msg}/approve", e.handleApprove)
		mux.HandleFunc("DELETE "+p+"/canvases/{id}/messages/{msg}", e.handleDrop)
		mux.HandleFunc("POST "+p+"/canvases/{id}/foreman", e.handleForeman)
		mux.HandleFunc("POST "+p+"/canvases/{id}/template", e.handleSaveTemplate)
		mux.HandleFunc("GET "+p+"/canvas/kinds", e.handleKinds)
		mux.HandleFunc("GET "+p+"/canvas/templates", e.handleTemplates)
	}
	mux.HandleFunc("GET /api/canvas/live", e.handleLive)
	mux.HandleFunc("POST /api/canvas/pause", e.handlePauseAll)

	mux.HandleFunc("GET /api/canvas/self", e.agentRoute("", e.handleSelf))
	mux.HandleFunc("GET /api/canvas/self/launch", e.agentRoute("", e.handleLaunch))
	mux.HandleFunc("GET /api/canvas/view", e.agentRoute(ScopeObserve, e.handleView))
	mux.HandleFunc("POST /api/canvas/ops", e.agentRoute(ScopeBuild, e.handleAgentOps))
	mux.HandleFunc("POST /api/canvas/nodes/{node}/send", e.agentRoute(ScopeTalk, e.handleAgentSend))
	mux.HandleFunc("GET /api/canvas/nodes/{node}/read", e.agentRoute(ScopeObserve, e.handleAgentRead))
	mux.HandleFunc("GET /api/canvas/nodes/{node}/wait", e.agentRoute(ScopeObserve, e.handleAgentWait))
	mux.HandleFunc("POST /api/canvas/emit", e.agentRoute("", e.handleEmit))
	mux.HandleFunc("POST /api/canvas/hooks/{hook}", e.handleHook)
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.MarshalWrite(w, v)
}

func writeError(w http.ResponseWriter, status int, message string) {
	writeJSON(w, status, map[string]string{"error": message})
}

// writeErr writes an error with its status: a conflict of a batch with the
// canvas's revision, which the page starts over from.
func writeErr(w http.ResponseWriter, err error) {
	var op *opError
	switch {
	case errors.As(err, &op):
		body := map[string]any{"error": err.Error()}
		if op.status == http.StatusConflict && op.rev > 0 {
			body["rev"] = op.rev
		}
		writeJSON(w, op.status, body)
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "no such canvas")
	default:
		writeError(w, http.StatusInternalServerError, err.Error())
	}
}

func errUnauthorized(text string) error { return &opError{status: http.StatusUnauthorized, text: text} }

// decode reads a request's JSON body into v. An empty body leaves v as it
// is, when optional.
func decode(r *http.Request, v any, optional bool) error {
	data, err := readBody(r.Body, maxRequest)
	if err != nil {
		var op *opError
		if errors.As(err, &op) {
			return err
		}
		return errInvalid("cannot read the request: " + err.Error())
	}
	if len(bytes.TrimSpace(data)) == 0 {
		if optional {
			return nil
		}
		return errInvalid("the request needs a JSON body")
	}
	if err := json.Unmarshal(data, v); err != nil {
		return errInvalid("the body is not the JSON expected: " + err.Error())
	}
	return nil
}

func truthy(value string) bool {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}

// parseTimeout reads a timeout: seconds, or a duration such as 10m.
func parseTimeout(value string, fallback time.Duration) time.Duration {
	value = strings.TrimSpace(value)
	if value == "" {
		return fallback
	}
	if seconds, err := strconv.Atoi(value); err == nil {
		return time.Duration(seconds) * time.Second
	}
	if d, err := time.ParseDuration(value); err == nil {
		return d
	}
	return fallback
}

// workspaceOf finds the workspace a page's request names.
func (e *Engine) workspaceOf(w http.ResponseWriter, r *http.Request) (Workspace, bool) {
	ws, ok := e.o.Host.Workspace(r.PathValue("ws"))
	if !ok {
		writeError(w, http.StatusNotFound, "no such workspace")
	}
	return ws, ok
}

// canvasOf finds the canvas a page's request names, loading it.
func (e *Engine) canvasOf(w http.ResponseWriter, r *http.Request) (*Canvas, bool) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return nil, false
	}
	c, err := e.get(ws, r.PathValue("id"))
	if err != nil {
		writeErr(w, err)
		return nil, false
	}
	return c, true
}

// list summarizes a workspace's canvases, the latest changed first.
func (e *Engine) list(ws Workspace) []Summary {
	loaded := map[string]*Canvas{}
	for _, c := range e.all() {
		if c.ws.ID == ws.ID {
			loaded[c.id] = c
		}
	}
	out := []Summary{}
	for _, id := range listIDs(ws.Path) {
		if c := loaded[id]; c != nil {
			out = append(out, c.summary())
			continue
		}
		doc, readOnly, err := loadDoc(ws.Path, id)
		if err != nil {
			continue
		}
		s := summaryOf(doc)
		s.ReadOnly = readOnly
		out = append(out, s)
	}
	slices.SortFunc(out, func(a, b Summary) int { return b.UpdatedAt.Compare(a.UpdatedAt) })
	return out
}

// Canvases summarizes a workspace's canvases; none when the engine is off.
func (e *Engine) Canvases(wsID string) []Summary {
	if e == nil {
		return nil
	}
	ws, ok := e.o.Host.Workspace(wsID)
	if !ok {
		return nil
	}
	return e.list(ws)
}

func (e *Engine) handleList(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"canvases": e.list(ws)})
}

func (e *Engine) handleCreate(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	var req struct {
		ID       string `json:"id"`
		Title    string `json:"title"`
		Template string `json:"template"`
	}
	if err := decode(r, &req, true); err != nil {
		writeErr(w, err)
		return
	}
	if req.ID == "" {
		req.ID = uuid.New().String()
	}
	var t Template
	if req.Template != "" {
		found, ok := e.findTemplate(ws, req.Template)
		if !ok {
			writeError(w, http.StatusNotFound, "no template "+req.Template)
			return
		}
		t = found
		if req.Title == "" && len(t.Nodes) > 0 {
			req.Title = t.Title
		}
	}
	c, err := e.create(ws, req.ID, req.Title)
	if err != nil {
		writeErr(w, err)
		return
	}
	var result *Result
	if len(t.Nodes) > 0 {
		// A template built on a canvas that has nodes goes beside them.
		dx, dy := 0, 0
		c.mu.Lock()
		if len(c.doc.Nodes) > 0 {
			right := c.doc.Nodes[0].X + c.doc.Nodes[0].W
			top := c.doc.Nodes[0].Y
			for _, n := range c.doc.Nodes {
				right, top = max(right, n.X+n.W), min(top, n.Y)
			}
			left := t.Nodes[0].X
			for _, n := range t.Nodes {
				left = min(left, n.X)
			}
			dx, dy = right+2*placeGap-left, top
		}
		c.mu.Unlock()
		res, err := c.apply(userActor, Batch{Ops: t.ops(dx, dy)})
		if err != nil {
			writeErr(w, err)
			return
		}
		result = &res
	}
	writeJSON(w, http.StatusOK, map[string]any{"canvas": c.summary(), "result": result})
}

func (e *Engine) handleGet(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	c.mu.Lock()
	data, err := json.Marshal(c.snapshotLocked("snapshot"))
	c.mu.Unlock()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.Write(data)
}

func (e *Engine) handlePatch(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	data, err := readBody(r.Body, maxRequest)
	if err != nil {
		writeErr(w, err)
		return
	}
	res, err := c.apply(userActor, Batch{Ops: []Op{{Op: "canvas.update", Set: jsontext.Value(data)}}})
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

func (e *Engine) handleDelete(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	q := r.URL.Query()
	o := DeleteOptions{KeepTerminals: truthy(q.Get("keep_terminals")), KeepSessions: truthy(q.Get("keep_sessions"))}
	if err := c.remove(o); err != nil {
		writeErr(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (e *Engine) handleOps(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	var batch Batch
	if err := decode(r, &batch, false); err != nil {
		writeErr(w, err)
		return
	}
	id := r.PathValue("id")
	c, err := e.get(ws, id)
	if errors.Is(err, fs.ErrNotExist) && batch.Create {
		c, err = e.create(ws, id, batch.Title)
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	res, err := c.apply(userActor, batch)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

func (e *Engine) handleEvents(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	c.serveEvents(w, r)
}

// outputsOf counts the outputs a node gave so far: what a send answers, to
// wait for the target's next output (read ?wait=output&after=).
func (c *Canvas) outputsOf(node string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	if st := c.states[node]; st != nil {
		return st.outputs
	}
	return 0
}

func (e *Engine) handleSend(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	var req sendRequest
	if err := decode(r, &req, false); err != nil {
		writeErr(w, err)
		return
	}
	node := r.PathValue("node")
	outputs := c.outputsOf(node)
	m, err := c.send("user", node, req)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"message": m, "node": node, "outputs": outputs})
}

func (e *Engine) handleRead(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	q := r.URL.Query()
	lines, _ := strconv.Atoi(q.Get("lines"))
	node := r.PathValue("node")
	text, err := c.read(node, q.Get("what"), min(max(lines, 0), 5000))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"node": node, "what": q.Get("what"), "text": text, "status": c.status(node)})
}

func (e *Engine) handleTranscript(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	n := c.node(r.PathValue("node"))
	switch {
	case n == nil:
		writeError(w, http.StatusNotFound, "no such node")
		return
	case n.Kind != KindAgent:
		writeError(w, http.StatusBadRequest, "«"+n.Title+"» is not an agent")
		return
	}
	tail, err := strconv.Atoi(r.URL.Query().Get("tail"))
	if err != nil || tail <= 0 {
		tail = 40
	}
	tail = min(tail, 500)
	entries := []FeedEntry{}
	session := n.Runtime.Session
	// exists: the session has a transcript; none before a run of it began.
	exists := false
	if session != "" {
		tr, err := cockpit.LoadSession(c.ws.SessionDir, session)
		switch {
		case err == nil:
			exists = true
			list := tr.Entries
			if len(list) > tail {
				list = list[len(list)-tail:]
			}
			for _, entry := range list {
				entries = append(entries, compactEntry(entry))
			}
		case !errors.Is(err, fs.ErrNotExist):
			writeErr(w, err)
			return
		}
	}
	running := session != "" && e.o.Host.RunActive(c.ws, session)
	writeJSON(w, http.StatusOK, map[string]any{"node": n.ID, "session": session, "exists": exists, "running": running, "entries": entries})
}

func (e *Engine) handleRestart(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	var req struct {
		Resume bool `json:"resume"`
	}
	if err := decode(r, &req, true); err != nil {
		writeErr(w, err)
		return
	}
	if err := c.restart(r.PathValue("node"), req.Resume); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusAccepted, map[string]any{"ok": true})
}

func (e *Engine) handleStop(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	if err := c.stopNode(r.PathValue("node")); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

func (e *Engine) handleFire(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	var req struct {
		Text string `json:"text"`
	}
	if err := decode(r, &req, true); err != nil {
		writeErr(w, err)
		return
	}
	if len(req.Text) > maxPaste {
		writeError(w, http.StatusBadRequest, "the text is too long")
		return
	}
	id := r.PathValue("node")
	c.mu.Lock()
	var src *manualSource
	if st := c.states[id]; st != nil {
		src, _ = st.inst.(*manualSource)
	}
	c.mu.Unlock()
	if src == nil {
		n := c.node(id)
		switch {
		case n == nil:
			writeError(w, http.StatusNotFound, "no such node")
		case n.Kind != KindSource || n.Preset != "manual" || n.Plugin != "":
			writeError(w, http.StatusBadRequest, "only a manual source fires at the press of a button")
		default:
			writeError(w, http.StatusConflict, "the source is not running: is the canvas paused?")
		}
		return
	}
	src.press(req.Text)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

func (e *Engine) handleRotate(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	id := r.PathValue("node")
	if err := c.rotate(id); err != nil {
		writeErr(w, err)
		return
	}
	epoch := 0
	if n := c.node(id); n != nil {
		epoch = n.Runtime.Epoch
	}
	writeJSON(w, http.StatusOK, map[string]any{"epoch": epoch})
}

func (e *Engine) handleRemoveWorktree(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	id := r.PathValue("node")
	n := c.node(id)
	switch {
	case n == nil:
		writeError(w, http.StatusNotFound, "no such node")
		return
	case n.Runtime.Worktree == "":
		writeError(w, http.StatusConflict, "«"+n.Title+"» has no worktree")
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
	defer cancel()
	path := n.Runtime.Worktree
	if _, err := os.Stat(path); err == nil {
		dirty, err := worktree.Dirty(ctx, path)
		if err != nil {
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
		if dirty {
			writeError(w, http.StatusConflict, "the worktree has changes that are not committed: commit or discard them first")
			return
		}
	}
	// Its shell works there: it ends with it.
	c.mu.Lock()
	var inst instance
	if st := c.states[id]; st != nil {
		inst, st.inst, st.terminal = st.inst, nil, nil
	}
	c.mu.Unlock()
	if inst != nil {
		inst.stop()
	}
	if n.Runtime.Terminal != "" {
		_ = e.o.Host.Terminals().Kill(n.Runtime.Terminal)
	}
	if _, err := os.Stat(path); err == nil {
		if err := worktree.Remove(ctx, c.ws.Path, path); err != nil {
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
	}
	c.setRuntime(id, func(r *Runtime) { r.Worktree, r.Branch, r.Terminal = "", "", "" })
	c.setStatus(id, StateStopped, "its worktree was removed")
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "removed": path})
}

func (e *Engine) handleMessage(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	m, err := c.message(r.PathValue("msg"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, m)
}

func (e *Engine) handleApprove(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	var req struct {
		Text *string `json:"text"`
	}
	if err := decode(r, &req, true); err != nil {
		writeErr(w, err)
		return
	}
	m, err := c.approve(r.PathValue("msg"), req.Text)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"message": m})
}

func (e *Engine) handleDrop(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	if err := c.drop(r.PathValue("msg")); err != nil {
		writeErr(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// foreman is the canvas's foreman node, if it has one.
func (c *Canvas) foreman() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, n := range c.doc.Nodes {
		if n.Kind == KindAgent && n.Preset == "foreman" {
			return n.ID
		}
	}
	return ""
}

// handleForeman asks the canvas's foreman: the first question makes it.
func (e *Engine) handleForeman(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	var req struct {
		Text  string `json:"text"`
		Title string `json:"title"`
	}
	if err := decode(r, &req, false); err != nil {
		writeErr(w, err)
		return
	}
	text := strings.TrimSpace(req.Text)
	if text == "" {
		writeError(w, http.StatusBadRequest, "ask the foreman something")
		return
	}
	id := r.PathValue("id")
	c, err := e.get(ws, id)
	if errors.Is(err, fs.ErrNotExist) {
		c, err = e.create(ws, id, req.Title)
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	if foreman := c.foreman(); foreman != "" {
		m, err := c.send("user", foreman, sendRequest{Text: text})
		if err != nil {
			writeErr(w, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"node": foreman, "message": m})
		return
	}
	res, err := c.apply(userActor, Batch{Ops: []Op{{Op: "node.add", Node: &NodeSpec{
		Kind: KindAgent, Preset: "foreman", Title: "Foreman", Prompt: text,
	}}}})
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"node": res.IDs["0"], "created": true, "result": res})
}

func (e *Engine) handleSaveTemplate(w http.ResponseWriter, r *http.Request) {
	c, ok := e.canvasOf(w, r)
	if !ok {
		return
	}
	var req struct {
		Name        string `json:"name"`
		Description string `json:"description"`
	}
	if err := decode(r, &req, false); err != nil {
		writeErr(w, err)
		return
	}
	t, err := c.saveTemplate(req.Name, strings.TrimSpace(req.Description))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, templateSummaries([]Template{t})[0])
}

// agentConfig is the schema of a kou agent node's configuration.
var agentConfig = map[string]any{
	"type": "object",
	"properties": map[string]any{
		"model":        map[string]any{"type": "string", "title": "Model", "description": "The model it runs with; the session's when empty."},
		"sandbox":      map[string]any{"type": "string", "title": "Sandbox", "enum": []any{"off", "worktree"}, "default": "off", "description": "worktree: it works in a git worktree of its own."},
		"output":       map[string]any{"type": "string", "title": "Output", "enum": []any{OutputReplies, OutputAll, OutputExplicit}, "default": OutputReplies, "description": "What goes along its edges: replies — its answers to messages from the canvas (what you ask it stays with you); all — every answer; explicit — only what it emits."},
		"instructions": map[string]any{"type": "string", "title": "Role", "description": "What it is told it does on the canvas."},
	},
}

func (e *Engine) handleKinds(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	catalog := e.catalog(ws)
	harnesses := []map[string]any{}
	for _, h := range catalog.harnesses {
		def := h.Harness
		item := map[string]any{
			"id": def.ID, "plugin": h.Plugin, "title": def.Title, "icon": def.Icon, "description": def.Description,
			"config": def.Config, "launch": def.Launch, "installed": true, "resume": len(def.Resume) > 0,
		}
		check := def.Check
		if len(check) == 0 {
			check = def.Command
		}
		if len(check) > 0 && def.Launch != "shell" {
			item["program"] = check[0]
			if _, err := exec.LookPath(check[0]); err != nil {
				item["installed"] = false
			}
		}
		harnesses = append(harnesses, item)
	}
	sources := []map[string]any{}
	for _, s := range catalog.sources {
		def := s.Source
		sources = append(sources, map[string]any{
			"id": def.ID, "plugin": s.Plugin, "title": def.Title, "icon": def.Icon, "description": def.Description,
			"config": def.Config, "outputs": def.Outputs, "mode": def.Mode, "interval": def.Interval, "builtin": def.Builtin,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"harnesses": harnesses, "sources": sources, "agent": map[string]any{"config": agentConfig}})
}

func (e *Engine) handleTemplates(w http.ResponseWriter, r *http.Request) {
	ws, ok := e.workspaceOf(w, r)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"templates": templateSummaries(e.templates(ws))})
}

// handleLive lists the live canvases, for the rail's badge.
func (e *Engine) handleLive(w http.ResponseWriter, r *http.Request) {
	items := []map[string]any{}
	for _, c := range e.all() {
		s := c.summary()
		c.mu.Lock()
		deleted := c.deleted
		c.mu.Unlock()
		if s.Live && !deleted {
			items = append(items, map[string]any{"workspace": c.ws.ID, "id": s.ID, "title": s.Title, "busy": s.Busy, "waiting": s.Waiting})
		}
	}
	slices.SortFunc(items, func(a, b map[string]any) int { return strings.Compare(a["title"].(string), b["title"].(string)) })
	writeJSON(w, http.StatusOK, map[string]any{"live": len(items), "canvases": items})
}

// handlePauseAll pauses every live canvas.
func (e *Engine) handlePauseAll(w http.ResponseWriter, r *http.Request) {
	paused := 0
	for _, c := range e.all() {
		c.mu.Lock()
		live := c.doc.Live && !c.deleted && !c.readOnly
		c.mu.Unlock()
		if !live {
			continue
		}
		if _, err := c.apply(userActor, Batch{Ops: []Op{{Op: "canvas.update", Set: jsontext.Value(`{"live":false}`)}}}); err == nil {
			paused++
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"paused": paused})
}

// agentCall is a request of a node's program, its token checked.
type agentCall struct {
	c    *Canvas
	node *Node
	// scope is what the program may do: its token's, and no more than its
	// node's access now.
	scope string
	// source is set for a source's process, which emits its node's events
	// and nothing else.
	source bool
}

func (a *agentCall) actor() Actor { return Actor{Kind: "node", ID: a.node.ID, Scope: a.scope} }

// resolve names a node a program's request names: self is its own, and a
// title one node alone has, that node.
func (a *agentCall) resolve(id string) string {
	if id == "self" {
		return a.node.ID
	}
	return a.c.nodeNamed(id)
}

// nodeNamed is the ID of the node ref names: its ID, or a title that one
// node alone has, whatever its case; ref as it is when none is.
func (c *Canvas) nodeNamed(ref string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.doc.node(ref) != nil {
		return ref
	}
	title := strings.TrimSpace(strings.Trim(strings.TrimSpace(ref), "«»\""))
	found := ""
	for _, n := range c.doc.Nodes {
		if title != "" && strings.EqualFold(strings.TrimSpace(n.Title), title) {
			if found != "" {
				return ref
			}
			found = n.ID
		}
	}
	if found == "" {
		return ref
	}
	return found
}

// agentRoute checks a program's token, and that it allows what need asks;
// "" asks only for a valid token.
func (e *Engine) agentRoute(need string, h func(http.ResponseWriter, *http.Request, *agentCall)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		a, err := e.authenticate(r)
		if err != nil {
			writeErr(w, err)
			return
		}
		if need != "" && (a.source || !allows(a.scope, need)) {
			scope := a.scope
			if a.source {
				scope = "a source's"
			}
			writeError(w, http.StatusForbidden, fmt.Sprintf("your access is %s; this needs %s", scope, need))
			return
		}
		h(w, r, a)
	}
}

// authenticate checks a program's token: signed by the server, of a node
// that is there, of its current epoch.
func (e *Engine) authenticate(r *http.Request) (*agentCall, error) {
	token, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer ")
	if !ok || strings.TrimSpace(token) == "" {
		return nil, errUnauthorized("this needs a canvas token: it is given to the programs of a canvas's nodes")
	}
	claims, err := parseToken(e.secret, token)
	if err != nil {
		return nil, errUnauthorized("the canvas token is not valid")
	}
	c, err := e.findCanvas(claims.Canvas)
	if err != nil {
		return nil, errUnauthorized("the canvas of the token is gone")
	}
	n := c.node(claims.Node)
	switch {
	case n == nil:
		return nil, errUnauthorized("the node of the token is gone")
	case n.Runtime.Epoch != claims.Epoch:
		return nil, errUnauthorized("the canvas token was replaced: restart the node's program for a new one")
	case n.Proposed:
		return nil, errForbidden("the node waits for the user's approval")
	}
	a := &agentCall{c: c, node: n}
	if claims.Scope == scopeSource {
		if n.Kind != KindSource {
			return nil, errUnauthorized("the canvas token is not valid")
		}
		a.source, a.scope = true, ScopeNone
		return a, nil
	}
	access := n.Access
	if _, ok := scopeRank[access]; !ok {
		access = ScopeNone
	}
	a.scope = claims.Scope
	if scopeRank[access] < scopeRank[a.scope] {
		a.scope = access
	}
	return a, nil
}

// findCanvas finds a canvas by its ID in any workspace, loading it.
func (e *Engine) findCanvas(id string) (*Canvas, error) {
	if !ValidID(id) {
		return nil, fs.ErrNotExist
	}
	for _, c := range e.all() {
		if c.id == id {
			return c, nil
		}
	}
	for _, ws := range e.o.Host.Workspaces() {
		if _, err := os.Stat(docPath(ws.Path, id)); err == nil {
			return e.get(ws, id)
		}
	}
	return nil, fs.ErrNotExist
}

// Self is what a node's program learns of itself.
type Self struct {
	Canvas ViewCanvas `json:"canvas"`
	Node   ViewNode   `json:"node"`
	Scope  string     `json:"scope"`
	Brief  string     `json:"brief,omitzero"`
	// Workspace is the workspace's folder; Page, the canvas's address in
	// the cockpit.
	Workspace string `json:"workspace"`
	Page      string `json:"page"`
}

func (e *Engine) handleSelf(w http.ResponseWriter, r *http.Request, a *agentCall) {
	v := a.c.view(a.node.ID, a.scope, false)
	self := Self{
		Canvas: v.Canvas, Scope: a.scope, Brief: a.c.brief(a.node.ID), Workspace: a.c.ws.Path,
		Page: strings.TrimRight(e.o.URL, "/") + "/#/w/" + a.c.ws.ID + "/c/" + a.c.id,
	}
	if a.source {
		self.Scope = "source"
	}
	if v.Me != nil {
		self.Node = *v.Me
	}
	writeJSON(w, http.StatusOK, self)
}

func (e *Engine) handleLaunch(w http.ResponseWriter, r *http.Request, a *agentCall) {
	launch, err := a.c.launchSpec(a.node.ID, truthy(r.URL.Query().Get("resume")))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, launch)
}

func (e *Engine) handleView(w http.ResponseWriter, r *http.Request, a *agentCall) {
	writeJSON(w, http.StatusOK, a.c.view(a.node.ID, a.scope, r.URL.Query().Get("detail") == "full"))
}

func (e *Engine) handleAgentOps(w http.ResponseWriter, r *http.Request, a *agentCall) {
	var batch Batch
	if err := decode(r, &batch, false); err != nil {
		writeErr(w, err)
		return
	}
	batch.Create = false
	res, err := a.c.apply(a.actor(), batch)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

func (e *Engine) handleAgentSend(w http.ResponseWriter, r *http.Request, a *agentCall) {
	var req sendRequest
	if err := decode(r, &req, false); err != nil {
		writeErr(w, err)
		return
	}
	node := a.resolve(r.PathValue("node"))
	outputs := a.c.outputsOf(node)
	m, wait, err := a.c.sendWaiting(a.node.ID, node, req)
	if err != nil {
		writeErr(w, err)
		return
	}
	result := map[string]any{"message": m, "node": node, "outputs": outputs}
	if wait != nil {
		timeout := 10 * time.Minute
		if req.TimeoutS > 0 {
			timeout = time.Duration(req.TimeoutS * float64(time.Second))
		}
		answer, reason, timedOut, err := a.c.awaitReply(r.Context(), m.ID, wait, timeout)
		if err != nil {
			return // gone: the answer comes to the node as a reply
		}
		result["answered"] = answer != ""
		if answer != "" {
			result["answer"] = answer
		}
		if reason != "" {
			result["reason"] = reason
		}
		if timedOut {
			result["timed_out"] = true
			result["reason"] = "no answer yet: it comes to you as a reply message once there is one"
		}
		result["status"] = a.c.status(node)
	}
	writeJSON(w, http.StatusOK, result)
}

func (e *Engine) handleAgentRead(w http.ResponseWriter, r *http.Request, a *agentCall) {
	q := r.URL.Query()
	node := a.resolve(r.PathValue("node"))
	result := map[string]any{"node": node, "what": q.Get("what")}
	if until := q.Get("wait"); until != "" {
		after := -1
		if q.Has("after") {
			if n, err := strconv.Atoi(q.Get("after")); err == nil {
				after = n
			}
		}
		waited, err := a.c.wait(r.Context(), node, until, after, parseTimeout(q.Get("timeout"), 10*time.Minute))
		if err != nil {
			writeErr(w, err)
			return
		}
		result["timed_out"] = waited.TimedOut
		result["outputs"] = waited.Outputs
	}
	lines, _ := strconv.Atoi(q.Get("lines"))
	text, err := a.c.read(node, q.Get("what"), min(max(lines, 0), 5000))
	if err != nil {
		writeErr(w, err)
		return
	}
	result["text"] = text
	result["status"] = a.c.status(node)
	writeJSON(w, http.StatusOK, result)
}

func (e *Engine) handleAgentWait(w http.ResponseWriter, r *http.Request, a *agentCall) {
	q := r.URL.Query()
	after := -1
	if q.Has("after") {
		if n, err := strconv.Atoi(q.Get("after")); err == nil {
			after = n
		}
	}
	until := q.Get("until")
	if until == "" {
		until = "idle"
	}
	result, err := a.c.wait(r.Context(), a.resolve(r.PathValue("node")), until, after, parseTimeout(q.Get("timeout"), 10*time.Minute))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, result)
}

// emitRequest is what a node's program says of its own node: an output, a
// state, the session of its harness.
type emitRequest struct {
	Port  string         `json:"port"`
	Title string         `json:"title"`
	Text  string         `json:"text"`
	Data  jsontext.Value `json:"data"`
	// Status is busy, idle or waiting, for a harness's hooks; ok or error
	// for a source.
	Status string `json:"status"`
	Detail string `json:"detail"`
	// AgentSession is the harness's own session, which resumes it.
	AgentSession string `json:"agent_session"`
	// Answer says that the text is the harness's answer at the end of its
	// turn, which goes where the messages it answers came from — and an
	// answer without text, that it answered nothing.
	Answer bool `json:"answer"`
}

// maxEmit bounds an output's text.
const maxEmit = 1 << 20

func (e *Engine) handleEmit(w http.ResponseWriter, r *http.Request, a *agentCall) {
	var req emitRequest
	if err := decode(r, &req, false); err != nil {
		writeErr(w, err)
		return
	}
	c, n := a.c, a.node
	state := ""
	switch req.Status {
	case "":
	case StateBusy, StateIdle, StateWaiting:
		state = req.Status
	case "ok":
		state = StateIdle
	case StateError:
		state = StateError
	default:
		writeError(w, http.StatusBadRequest, "status is busy, idle, waiting, ok or error")
		return
	}
	if state != "" && n.Kind != KindTerminal && n.Kind != KindSource {
		state = "" // an agent's state is its runs'
	}
	output := req.Text != "" || len(req.Data) != 0
	port := req.Port
	if port == "" {
		port = "out"
	}
	if output {
		if len(req.Text) > maxEmit {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("an output's text is %d KiB at most", maxEmit>>10))
			return
		}
		if _, outputs := portsOf(n, c.e.catalog(c.ws)); !slices.Contains(outputs, port) {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("«%s» has no output %q", n.Title, port))
			return
		}
	}
	if session := strings.TrimSpace(req.AgentSession); session != "" && n.Kind == KindTerminal {
		if len(session) > 200 || strings.ContainsFunc(session, unicode.IsControl) {
			writeError(w, http.StatusBadRequest, "agent_session is not a session's ID")
			return
		}
		c.setRuntime(n.ID, func(r *Runtime) { r.AgentSession = session })
	}
	detail := cleanTitle(req.Detail)
	// A harness's hooks say what it answered at the end of its turn; so
	// did they before they said it was an answer, with the state idle.
	answer := n.harness() && port == "out" && (req.Answer || output && state == StateIdle)
	c.mu.Lock()
	if st := c.states[n.ID]; st != nil && (output || answer) {
		out := Output{Port: port, Title: cleanTitle(req.Title), Text: req.Text, Data: req.Data}
		if t, ok := st.inst.(*terminalInstance); ok && port == "out" && req.Text != "" {
			t.setAnswer(req.Text)
		}
		// The output first: a state of idle has the next message
		// delivered, and what the node gives after it is that message's.
		if answer {
			c.answerLocked(n.ID, out, []*Message{st.cause}, "")
		} else {
			// What an agent puts out itself goes on the chain of what it
			// works on, which its answer at the end of the turn still
			// answers.
			c.putOutLocked(n.ID, out, st.cause, nil)
		}
	}
	if st := c.states[n.ID]; st != nil && st.cause != nil && state == StateBusy && !output && n.harness() {
		// A turn the canvas did not start — its message went in a while
		// before — is the user's: its answer stays with the user.
		if t, ok := st.inst.(*terminalInstance); ok && time.Since(time.Unix(0, t.delivered.Load())) > startedBy {
			c.unansweredLocked(n.ID, "«"+n.Title+"» was given another prompt before it answered")
		}
	}
	if state != "" {
		c.setStatusLocked(n.ID, state, detail)
	}
	c.mu.Unlock()
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// handleHook takes a request to a webhook source's address.
func (e *Engine) handleHook(w http.ResponseWriter, r *http.Request) {
	c, src := e.findHook(r.PathValue("hook"))
	if c == nil {
		writeError(w, http.StatusNotFound, "no such hook")
		return
	}
	if src == nil {
		writeError(w, http.StatusConflict, "the source is not running: its canvas is paused")
		return
	}
	body, err := readBody(r.Body, maxHook)
	if err != nil {
		writeError(w, http.StatusRequestEntityTooLarge, err.Error())
		return
	}
	src.receive(body, r.Header.Get("Content-Type"))
	writeJSON(w, http.StatusAccepted, map[string]any{"ok": true})
}
