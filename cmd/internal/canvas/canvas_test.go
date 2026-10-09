package canvas

import (
	"bufio"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// fakeHost is a server for the engine: one workspace, its shells, its
// plugins, and the sessions of agents, whose prompts it keeps.
type fakeHost struct {
	ws        Workspace
	terminals *terminal.Manager
	plugins   []plugin.Plugin

	mu      sync.Mutex
	prompts []prompt
	active  map[string]bool
	failing error
}

// prompt is what an agent's session was given, under its ID.
type prompt struct {
	session, id, text, model string
	force                    bool
}

func (h *fakeHost) Workspaces() []Workspace { return []Workspace{h.ws} }

func (h *fakeHost) Workspace(id string) (Workspace, bool) {
	if id == "" || id == h.ws.ID {
		return h.ws, true
	}
	return Workspace{}, false
}

func (h *fakeHost) Terminals() *terminal.Manager { return h.terminals }

func (h *fakeHost) Plugins(Workspace) plugin.Found {
	h.mu.Lock()
	defer h.mu.Unlock()
	return plugin.Found{Plugins: append([]plugin.Plugin(nil), h.plugins...), Trusted: true}
}

// setActive turns a plugin on or off.
func (h *fakeHost) setActive(name string, active bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for i := range h.plugins {
		if h.plugins[i].Name == name {
			h.plugins[i].Active = active
		}
	}
}

// forgetPresets has the engine read its plugins again at once.
func (e *Engine) forgetPresets() {
	e.mu.Lock()
	clear(e.presets)
	e.mu.Unlock()
}

func (h *fakeHost) Enqueue(_ context.Context, _ Workspace, session, id, text, model string, force bool) error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.failing != nil {
		return h.failing
	}
	h.prompts = append(h.prompts, prompt{session, id, text, model, force})
	return nil
}

// answer ends the run of a session's agent that answers the last prompt
// it was given, with text.
func (te *testEngine) answer(t *testing.T, session, text string) {
	t.Helper()
	prompts := te.host.promptsOf(session)
	if len(prompts) == 0 {
		t.Fatalf("the session %s was given no prompt to answer", session)
	}
	te.RunFinished("w1", session, text, "done", []string{prompts[len(prompts)-1].id})
}

func (h *fakeHost) StopRun(_ Workspace, session string) bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	was := h.active[session]
	delete(h.active, session)
	return was
}

func (h *fakeHost) RunActive(_ Workspace, session string) bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.active[session]
}

// promptsOf are the prompts a session was given, in order.
func (h *fakeHost) promptsOf(session string) []prompt {
	h.mu.Lock()
	defer h.mu.Unlock()
	var out []prompt
	for _, p := range h.prompts {
		if p.session == session {
			out = append(out, p)
		}
	}
	return out
}

func (h *fakeHost) setFailing(err error) {
	h.mu.Lock()
	h.failing = err
	h.mu.Unlock()
}

// newHost makes a workspace, and shells that start as the files of
// their integration say: nil gives them none. The shells start in a home
// of their own.
func newHost(t *testing.T, files func() (fs.FS, error)) *fakeHost {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if dir, ok := os.LookupEnv("ZDOTDIR"); ok {
		t.Setenv("ZDOTDIR", dir)
		os.Unsetenv("ZDOTDIR")
	}
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	h := &fakeHost{
		ws:        Workspace{ID: "w1", Path: dir, SessionDir: filepath.Join(dir, ".harness", "sessions")},
		terminals: terminal.NewManager(files, "test"),
		active:    map[string]bool{},
	}
	t.Cleanup(h.terminals.Close)
	return h
}

// testEngine is an engine with its routes served.
type testEngine struct {
	*Engine
	host *fakeHost
	srv  *httptest.Server
	logs *logBook
}

// logBook keeps what an engine logs.
type logBook struct {
	mu    sync.Mutex
	lines []string
}

func (l *logBook) printf(format string, args ...any) {
	l.mu.Lock()
	l.lines = append(l.lines, fmt.Sprintf(format, args...))
	l.mu.Unlock()
}

// startEngine starts an engine over host's workspace. Engines started one
// after the other with the same secret are runs of the same server.
func startEngine(t *testing.T, host *fakeHost, secret string) *testEngine {
	t.Helper()
	mux := http.NewServeMux()
	srv := httptest.NewServer(mux)
	logs := &logBook{}
	e, err := New(context.Background(), Options{
		Host: host, URL: srv.URL, SecretFile: secret,
		LaunchDir: filepath.Join(filepath.Dir(secret), "launch"), Logf: logs.printf,
	})
	if err != nil {
		t.Fatal(err)
	}
	e.Register(mux)
	e.Start()
	t.Cleanup(func() {
		srv.CloseClientConnections()
		srv.Close()
		e.Close()
	})
	return &testEngine{Engine: e, host: host, srv: srv, logs: logs}
}

// newEngine starts an engine over a new workspace whose shells have no
// integration.
func newEngine(t *testing.T) *testEngine {
	t.Helper()
	return startEngine(t, newHost(t, nil), filepath.Join(t.TempDir(), "canvas.secret"))
}

// call makes a request of the engine's routes, with a node's token when
// token is set.
func (te *testEngine) call(t *testing.T, method, path, body, token string) (int, []byte) {
	t.Helper()
	req, err := http.NewRequest(method, te.srv.URL+path, strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	if body != "" {
		req.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	res, err := te.srv.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	data, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	return res.StatusCode, data
}

// must makes a request that must answer want, and decodes the answer into
// v, unless v is nil.
func (te *testEngine) must(t *testing.T, want int, method, path, body, token string, v any) {
	t.Helper()
	status, data := te.call(t, method, path, body, token)
	if status != want {
		t.Fatalf("%s %s: %d %s, want %d", method, path, status, data, want)
	}
	if v != nil {
		if err := json.Unmarshal(data, v); err != nil {
			t.Fatalf("%s %s: %v in %s", method, path, err, data)
		}
	}
}

// ops applies a batch of operations as the user.
func (te *testEngine) ops(t *testing.T, canvas, ops string) Result {
	t.Helper()
	var res Result
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+canvas+"/ops", `{"ops":`+ops+`}`, "", &res)
	return res
}

// newCanvas makes a canvas.
func (te *testEngine) newCanvas(t *testing.T, title string) (string, *Canvas) {
	t.Helper()
	var made struct {
		Canvas Summary `json:"canvas"`
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases", `{"title":"`+title+`"}`, "", &made)
	return made.Canvas.ID, te.canvas(t, made.Canvas.ID)
}

// canvas is a canvas, loaded.
func (te *testEngine) canvas(t *testing.T, id string) *Canvas {
	t.Helper()
	c, err := te.get(te.host.ws, id)
	if err != nil {
		t.Fatal(err)
	}
	return c
}

// token is a token of a node, of scope.
func (te *testEngine) token(t *testing.T, c *Canvas, node, scope string) string {
	t.Helper()
	n := c.node(node)
	if n == nil {
		t.Fatalf("no node %s", node)
	}
	return mintToken(te.secret, claims{Canvas: c.id, Node: node, Epoch: n.Runtime.Epoch, Scope: scope})
}

// eventually waits for cond to hold, at most timeout.
func eventually(t *testing.T, timeout time.Duration, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

// sessionOf is the session of an agent's node.
func sessionOf(t *testing.T, c *Canvas, node string) string {
	t.Helper()
	session := ""
	eventually(t, 5*time.Second, "the session of "+node, func() bool {
		if n := c.node(node); n != nil {
			session = n.Runtime.Session
		}
		return session != ""
	})
	return session
}

// messagesIn lists the canvas's latest messages in a state.
func messagesIn(c *Canvas, state string) []Message {
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []Message
	for _, m := range c.messages {
		if m.State == state {
			out = append(out, *m)
		}
	}
	return out
}

// waitDelivered waits for n messages to have been delivered.
func waitDelivered(t *testing.T, c *Canvas, n int) {
	t.Helper()
	eventually(t, 5*time.Second, fmt.Sprintf("%d messages delivered", n), func() bool { return len(messagesIn(c, MessageDelivered)) >= n })
}

// published lists the events of a type the canvas's pages were sent.
func published(c *Canvas, kind string) []map[string]any {
	c.mu.Lock()
	defer c.mu.Unlock()
	var out []map[string]any
	for _, event := range c.hub.events {
		var v map[string]any
		if json.Unmarshal(event.data, &v) == nil && v["type"] == kind {
			out = append(out, v)
		}
	}
	return out
}

// noticed reports whether the pages were told of something that says text.
func noticed(c *Canvas, text string) bool {
	for _, notice := range published(c, "notice") {
		if s, _ := notice["text"].(string); strings.Contains(s, text) {
			return true
		}
	}
	return false
}

func TestCanvasesAreMadeListedChangedAndDeleted(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Board")

	var list struct {
		Canvases []Summary `json:"canvases"`
	}
	for _, path := range []string{"/api/w/w1/canvases", "/api/canvases"} {
		te.must(t, http.StatusOK, "GET", path, "", "", &list)
		if len(list.Canvases) != 1 || list.Canvases[0].ID != id || list.Canvases[0].Title != "Board" || !list.Canvases[0].Live {
			t.Fatalf("%s: %+v", path, list.Canvases)
		}
	}
	if status, _ := te.call(t, "GET", "/api/w/nope/canvases", "", ""); status != http.StatusNotFound {
		t.Fatalf("a workspace that is not there: %d", status)
	}

	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"note","title":"Readme","config":{"text":"hello"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}}
	]`)
	note, agent := res.IDs["0"], res.IDs["1"]
	if res.Rev != 1 || res.Placed[note] != (Rect{0, 0, 280, 180}) || res.Placed[agent] != (Rect{320, 0, 440, 560}) {
		t.Fatalf("result %+v", res)
	}
	session := sessionOf(t, c, agent)
	if meta := cockpit.LoadMeta(te.host.ws.SessionDir, session); meta.Canvas != id || meta.Node != agent || meta.Title != "Helper" {
		t.Fatalf("the agent's session says %+v", meta)
	}

	var snap snapshot
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id, "", "", &snap)
	if snap.Doc.Rev != 1 || len(snap.Doc.Nodes) != 2 || snap.Status[note].State != StateIdle || snap.Status[agent].State != StateIdle {
		t.Fatalf("snapshot %+v", snap)
	}
	var read struct {
		Text string `json:"text"`
	}
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+note+"/read", "", "", &read)
	if read.Text != "hello" {
		t.Fatalf("a note reads its text: %q", read.Text)
	}

	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"title":"Renamed","settings":{"routing":{"max_hops":5}}}`, "", nil)
	c.mu.Lock()
	title, settings := c.doc.Title, c.doc.Settings
	c.mu.Unlock()
	if title != "Renamed" || settings.Routing.MaxHops != 5 || settings.Routing.EdgeRatePerMinute != 30 || settings.Autonomy.Spawn != "allow" {
		t.Fatalf("patched: %q %+v", title, settings)
	}
	if status, _ := te.call(t, "PATCH", "/api/w/w1/canvases/"+id, `{"settings":{"autonomy":{"spawn":"sometimes"}}}`, ""); status != http.StatusBadRequest {
		t.Fatalf("bad settings: %d", status)
	}

	// Written soon after, whole.
	c.flush()
	doc, _, err := loadDoc(te.host.ws.Path, id)
	if err != nil || doc.Title != "Renamed" || len(doc.Nodes) != 2 || doc.Nodes[1].Runtime.Session != session {
		t.Fatalf("on disk: %+v, %v", doc, err)
	}

	if status, _ := te.call(t, "DELETE", "/api/w/w1/canvases/"+id, "", ""); status != http.StatusNoContent {
		t.Fatalf("delete: %d", status)
	}
	if _, err := os.Stat(docPath(te.host.ws.Path, id)); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("the document stays: %v", err)
	}
	if status, _ := te.call(t, "GET", "/api/w/w1/canvases/"+id, "", ""); status != http.StatusNotFound {
		t.Fatalf("a canvas deleted: %d", status)
	}
	if meta := cockpit.LoadMeta(te.host.ws.SessionDir, session); meta.Canvas != "" {
		t.Fatalf("the session of an agent that never ran stays the canvas's: %+v", meta)
	}
}

func TestBatchesApplyWholeOrNotAtAll(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Ops")
	a := te.ops(t, id, `[{"op":"node.add","node":{"preset":"note","title":"A"}}]`).IDs["0"]
	nodes := func() (int, int64) {
		c.mu.Lock()
		defer c.mu.Unlock()
		return len(c.doc.Nodes), c.doc.Rev
	}

	status, body := te.call(t, "POST", "/api/w/w1/canvases/"+id+"/ops",
		`{"ops":[{"op":"node.add","node":{"preset":"note","title":"B"}},{"op":"node.update","id":"n_missing","set":{"x":1}}]}`, "")
	if n, rev := nodes(); status != http.StatusNotFound || n != 1 || rev != 1 {
		t.Fatalf("a batch that fails half: %d %s, %d nodes at rev %d", status, body, n, rev)
	}

	// A batch made on an older revision still moves what is there: the
	// last move wins.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/ops", `{"base_rev":0,"ops":[{"op":"node.update","id":"`+a+`","set":{"x":96,"y":-40}}]}`, "", nil)
	if n := c.node(a); n.X != 96 || n.Y != -40 {
		t.Fatalf("moved to %d,%d", n.X, n.Y)
	}
	// But one that names what is gone conflicts, and says the revision.
	te.ops(t, id, `[{"op":"node.remove","id":"`+a+`","grace_s":0}]`)
	var conflict struct {
		Error string `json:"error"`
		Rev   int64  `json:"rev"`
	}
	te.must(t, http.StatusConflict, "POST", "/api/w/w1/canvases/"+id+"/ops", `{"base_rev":2,"ops":[{"op":"node.update","id":"`+a+`","set":{"title":"X"}}]}`, "", &conflict)
	if conflict.Rev != 3 || !strings.Contains(conflict.Error, "gone") {
		t.Fatalf("conflict %+v", conflict)
	}

	// $n names what the batch added; an edge added twice is one.
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Go"}},
		{"op":"node.add","node":{"preset":"agent","title":"Worker"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$0","port":"out"},"to":{"node":"$1","port":"in"}}},
		{"op":"node.add","node":{"preset":"note","title":"N"}}
	]`)
	source, worker, edge, note := res.IDs["0"], res.IDs["1"], res.IDs["2"], res.IDs["4"]
	if res.IDs["3"] != edge {
		t.Fatalf("the same edge twice: %v", res.IDs)
	}
	c.mu.Lock()
	e := *c.doc.edge(edge)
	kinds := []string{c.doc.node(source).Kind, c.doc.node(worker).Kind, c.doc.node(note).Kind}
	c.mu.Unlock()
	if e.From != (Port{source, "out"}) || e.To != (Port{worker, "in"}) || !slices.Equal(kinds, []string{KindSource, KindAgent, KindNote}) {
		t.Fatalf("edge %+v, kinds %v", e, kinds)
	}

	for _, bad := range []struct{ ops, says string }{
		{`[{"op":"edge.add","edge":{"from":{"node":"` + note + `"},"to":{"node":"` + worker + `"}}}]`, "no output"},
		{`[{"op":"edge.add","edge":{"from":{"node":"` + worker + `"},"to":{"node":"` + source + `"}}}]`, "no input"},
		{`[{"op":"edge.add","edge":{"from":{"node":"` + worker + `"},"to":{"node":"` + worker + `"}}}]`, "itself"},
		{`[{"op":"node.update","id":"` + worker + `","set":{"color":"red"}}]`, "cannot be set"},
		{`[{"op":"node.add","node":{"kind":"terminal","preset":"command"}}]`, "config.command"},
		{`[{"op":"node.add","node":{"kind":"terminal","preset":"nothing-like-it"}}]`, "no terminal preset"},
		{`[{"op":"node.add","node":{"kind":"agent","preset":"robot"}}]`, "foreman or none"},
		{`[{"op":"node.add","node":{"preset":"note","side":"inside","near":"` + worker + `"}}]`, "side"},
		{`[{"op":"edge.update","id":"` + edge + `","set":{"mode":"sometimes"}}]`, "mode"},
		{`[{"op":"teleport"}]`, "no operation"},
		{`[]`, "needs operations"},
	} {
		var answer struct {
			Error string `json:"error"`
		}
		te.must(t, http.StatusBadRequest, "POST", "/api/w/w1/canvases/"+id+"/ops", `{"ops":`+bad.ops+`}`, "", &answer)
		if !strings.Contains(answer.Error, bad.says) {
			t.Errorf("%s: %q, want it to say %q", bad.ops, answer.Error, bad.says)
		}
	}

	te.ops(t, id, `[{"op":"edge.update","id":"`+edge+`","set":{"mode":"approve","template":"#{{text}}"}},{"op":"canvas.update","set":{"title":"Changed"}}]`)
	c.mu.Lock()
	e, title := *c.doc.edge(edge), c.doc.Title
	c.mu.Unlock()
	if e.Mode != ModeApprove || e.Template != "#{{text}}" || title != "Changed" {
		t.Fatalf("edge %+v, title %q", e, title)
	}
	te.ops(t, id, `[{"op":"edge.remove","id":"`+edge+`"}]`)

	// A page that makes a canvas with its first node.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/fresh-1/ops", `{"create":true,"title":"Fresh","ops":[{"op":"node.add","node":{"preset":"note"}}]}`, "", nil)
	if fresh := te.canvas(t, "fresh-1"); fresh.summary().Title != "Fresh" || fresh.summary().Nodes != 1 {
		t.Fatalf("fresh: %+v", fresh.summary())
	}
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases/fresh-2/ops", `{"ops":[{"op":"node.add","node":{"preset":"note"}}]}`, ""); status != http.StatusNotFound {
		t.Fatalf("a canvas not made: %d", status)
	}
}

func TestOutputsGoAlongEdgesFromHopToHop(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Triage")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"kind":"agent","title":"Alpha","config":{"model":"m-alpha"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Beta"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$2"}}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$2"}}}
	]`)
	source, alpha, beta := res.IDs["0"], res.IDs["1"], res.IDs["2"]
	sa, sb := sessionOf(t, c, alpha), sessionOf(t, c, beta)

	// The event goes to both agents, under where it comes from, after
	// what each is told of the canvas.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 42"}`, "", nil)
	eventually(t, 5*time.Second, "both agents prompted", func() bool {
		return len(te.host.promptsOf(sa)) == 1 && len(te.host.promptsOf(sb)) == 1
	})
	first := te.host.promptsOf(sa)[0]
	if !strings.HasPrefix(first.text, "You are node «Alpha» ("+alpha+", kou agent) on the kou-conveyor canvas «Triage».") ||
		!strings.Contains(first.text, "\nNodes you can message: «Beta» ("+beta+", kou agent).\n") ||
		!strings.Contains(first.text, "your edges take it to «Beta» ("+beta+")") ||
		!strings.Contains(first.text, `CanvasSend once — {"node":"<id or title>","text":"…","wait":true}`) ||
		!strings.Contains(first.text, "Your access: build") ||
		!strings.HasSuffix(first.text, "\n\n[canvas] from «Issues» ("+source+"):\nissue 42") || first.model != "m-alpha" || first.force {
		t.Fatalf("Alpha was given %+v", first)
	}
	// Its node's feed shows the message, not the brief before it.
	if shown := compactEntry(&cockpit.Entry{Kind: cockpit.KindUser, Text: first.text}).Text; shown != "[canvas] from «Issues» ("+source+"):\nissue 42" {
		t.Fatalf("the feed shows %q", shown)
	}

	// Alpha's answer goes on to Beta, one hop further along the same
	// chain; Beta knows the canvas already.
	waitDelivered(t, c, 2)
	te.RunStarted("w1", sa)
	if state := c.status(alpha).State; state != StateBusy {
		t.Fatalf("Alpha runs, and is %s", state)
	}
	te.RunActivity("w1", sa, "Thinking")
	if activity := c.status(alpha).Activity; activity != "Thinking" {
		t.Fatalf("activity %q", activity)
	}
	te.answer(t, sa, "triaged: a bug")
	eventually(t, 5*time.Second, "Beta prompted again", func() bool { return len(te.host.promptsOf(sb)) == 2 })
	if second := te.host.promptsOf(sb)[1]; second.text != "[canvas] from «Alpha» ("+alpha+"):\ntriaged: a bug" || second.model != "" {
		t.Fatalf("Beta was given %+v", second)
	}
	waitDelivered(t, c, 3)
	var fromSource, fromAlpha Message
	for _, m := range messagesIn(c, MessageDelivered) {
		switch {
		case m.From.Node == source && m.To.Node == alpha:
			fromSource = m
		case m.From.Node == alpha:
			fromAlpha = m
		}
	}
	if fromSource.Hops != 0 || fromAlpha.Hops != 1 || fromAlpha.Chain != fromSource.Chain || fromAlpha.To.Node != beta {
		t.Fatalf("chain: %+v then %+v", fromSource, fromAlpha)
	}
	if state := c.status(alpha).State; state != StateIdle {
		t.Fatalf("Alpha is done, and %s", state)
	}

	var read struct {
		Text string `json:"text"`
	}
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+alpha+"/read?what=answer", "", "", &read)
	if read.Text != "triaged: a bug" {
		t.Fatalf("Alpha's answer reads %q", read.Text)
	}

	// A run that failed is an error, and gives nothing.
	te.RunFinished("w1", sb, "", "failed", nil)
	if state := c.status(beta).State; state != StateError {
		t.Fatalf("Beta failed, and is %s", state)
	}

	// Its session's runs see the canvas: its variables and a token of its
	// access, and its sandbox.
	te.ops(t, id, `[{"op":"node.update","id":"`+alpha+`","set":{"config":{"sandbox":"worktree"}}}]`)
	env := te.RunEnv("w1", sa)
	values := map[string]string{}
	for _, entry := range env {
		name, value, _ := strings.Cut(entry, "=")
		values[name] = value
	}
	claims, err := parseToken(te.secret, values["KOU_CANVAS_TOKEN"])
	if err != nil || claims.Node != alpha || claims.Scope != ScopeBuild || values["KOU_CANVAS_ID"] != id ||
		values["KOU_CANVAS_URL"] != te.srv.URL || values[runconfig.SandboxEnvironment] != runconfig.SandboxWorktree {
		t.Fatalf("env %v (%v)", env, err)
	}
	if env := te.RunEnv("w1", "not-a-canvas-session"); env != nil {
		t.Fatalf("a session of no canvas: %v", env)
	}
}

func TestALoopEndsAtTheMostHops(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Loop")
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"settings":{"routing":{"max_hops":3}}}`, "", nil)
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Ping"}},
		{"op":"node.add","node":{"kind":"agent","title":"Pong"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$0"}}},
		{"op":"node.add","node":{"preset":"manual","title":"Serve"}},
		{"op":"edge.add","edge":{"from":{"node":"$4"},"to":{"node":"$0"}}}
	]`)
	ping, pong, back, serve := res.IDs["0"], res.IDs["1"], res.IDs["3"], res.IDs["4"]
	c.mu.Lock()
	mode := c.doc.edge(back).Mode
	c.mu.Unlock()
	if mode != ModeApprove {
		t.Fatalf("the edge that closes a loop of agents is %q", mode)
	}
	te.ops(t, id, `[{"op":"edge.update","id":"`+back+`","set":{"mode":"auto"}}]`)

	sessions := []string{sessionOf(t, c, ping), sessionOf(t, c, pong)}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+serve+"/fire", `{"text":"serve"}`, "", nil)
	// Ping (hop 0), Pong (1), Ping (2), Pong (3): what Pong says then is a
	// fourth hop, over the three the canvas allows.
	for i := range 4 {
		session := sessions[i%2]
		eventually(t, 5*time.Second, fmt.Sprintf("prompt %d", i), func() bool { return len(te.host.promptsOf(session)) == i/2+1 })
		waitDelivered(t, c, i+1)
		te.answer(t, session, fmt.Sprintf("answer %d", i))
	}
	eventually(t, 5*time.Second, "a message dropped", func() bool { return len(messagesIn(c, MessageDropped)) == 1 })
	if dropped := messagesIn(c, MessageDropped)[0]; dropped.Hops != 4 || !strings.Contains(dropped.Reason, "hops") {
		t.Fatalf("dropped %+v", dropped)
	}
	if !noticed(c, "looks like a loop") {
		t.Fatal("no notice of the loop")
	}
	time.Sleep(100 * time.Millisecond)
	if len(te.host.promptsOf(sessions[0])) != 2 || len(te.host.promptsOf(sessions[1])) != 2 {
		t.Fatal("the loop went on")
	}
}

func TestApprovalHoldsAMessageUntilTheUserSays(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Gate")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Bell"}},
		{"op":"node.add","node":{"kind":"agent","title":"Desk"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"},"mode":"approve"}}
	]`)
	bell, desk := res.IDs["0"], res.IDs["1"]
	session := sessionOf(t, c, desk)
	fire := func(text string) Message {
		t.Helper()
		before := len(messagesIn(c, MessageAwaiting))
		te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+bell+"/fire", `{"text":"`+text+`"}`, "", nil)
		eventually(t, 5*time.Second, "a message awaiting approval", func() bool { return len(messagesIn(c, MessageAwaiting)) > before })
		awaiting := messagesIn(c, MessageAwaiting)
		return awaiting[len(awaiting)-1]
	}

	m := fire("ring")
	var snap snapshot
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id, "", "", &snap)
	if snap.Pending[desk] != 1 || len(snap.Queue) != 1 {
		t.Fatalf("pending %v, queue %d", snap.Pending, len(snap.Queue))
	}
	time.Sleep(100 * time.Millisecond)
	if len(te.host.promptsOf(session)) != 0 {
		t.Fatal("a message to approve went on its own")
	}
	var whole Message
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/messages/"+m.ID, "", "", &whole)
	if whole.Text != "[canvas] from «Bell» ("+bell+"):\nring" {
		t.Fatalf("the message: %+v", whole)
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/messages/"+m.ID+"/approve", `{"text":"ring, edited"}`, "", nil)
	eventually(t, 5*time.Second, "the approved message delivered", func() bool { return len(te.host.promptsOf(session)) == 1 })
	if text := te.host.promptsOf(session)[0].text; !strings.HasSuffix(text, "\n\nring, edited") {
		t.Fatalf("delivered %q", text)
	}
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases/"+id+"/messages/"+m.ID+"/approve", "", ""); status != http.StatusNotFound {
		t.Fatalf("approved twice: %d", status)
	}

	m = fire("again")
	if status, _ := te.call(t, "DELETE", "/api/w/w1/canvases/"+id+"/messages/"+m.ID, "", ""); status != http.StatusNoContent {
		t.Fatalf("drop: %d", status)
	}
	time.Sleep(100 * time.Millisecond)
	if len(te.host.promptsOf(session)) != 1 {
		t.Fatal("a message dropped was delivered")
	}
	if dropped := messagesIn(c, MessageDropped); len(dropped) != 1 || dropped[0].ID != m.ID {
		t.Fatalf("dropped %+v", dropped)
	}
}

func TestMessagesThatWaitOutliveTheServer(t *testing.T) {
	host := newHost(t, nil)
	secret := filepath.Join(t.TempDir(), "canvas.secret")
	te := startEngine(t, host, secret)
	id, c := te.newCanvas(t, "Later")
	// The writer puts out every answer, those to the user too.
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Writer","config":{"output":"all"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Reader"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	writer, reader := res.IDs["0"], res.IDs["1"]
	sw, sr := sessionOf(t, c, writer), sessionOf(t, c, reader)

	// A paused canvas keeps what its nodes give.
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"live":false}`, "", nil)
	te.RunFinished("w1", sw, "draft", "done", nil)
	eventually(t, 5*time.Second, "a message waiting", func() bool { return len(messagesIn(c, MessagePending)) == 1 })
	time.Sleep(100 * time.Millisecond)
	if len(host.promptsOf(sr)) != 0 {
		t.Fatal("a paused canvas delivered")
	}
	var live struct {
		Live int `json:"live"`
	}
	te.must(t, http.StatusOK, "GET", "/api/canvas/live", "", "", &live)
	if live.Live != 0 {
		t.Fatalf("live canvases: %d", live.Live)
	}
	te.Close()

	// The next run of the server finds it waiting, and delivers it once
	// the canvas is live again.
	next := startEngine(t, host, secret)
	if env := next.RunEnv("w1", sw); !slices.Contains(env, "KOU_CANVAS_NODE="+writer) {
		t.Fatalf("a run of the writer's session, after a restart: %v", env)
	}
	c = next.canvas(t, id)
	if waiting := messagesIn(c, MessagePending); len(waiting) != 1 || waiting[0].To.Node != reader {
		t.Fatalf("waiting after the restart: %+v", waiting)
	}
	next.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"live":true}`, "", nil)
	eventually(t, 5*time.Second, "the message delivered", func() bool { return len(host.promptsOf(sr)) == 1 })
	if text := host.promptsOf(sr)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Writer» ("+writer+"):\ndraft") {
		t.Fatalf("delivered %q", text)
	}
	next.must(t, http.StatusOK, "POST", "/api/canvas/pause", "", "", nil)
	if c.summary().Live {
		t.Fatal("pause all left it live")
	}
}

func TestLongMessagesAreKeptInAFile(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Long")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Author","config":{"output":"all"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Editor"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	author, editor := sessionOf(t, c, res.IDs["0"]), sessionOf(t, c, res.IDs["1"])
	text := strings.Repeat("words ", 8<<10) + "THE END"
	te.RunFinished("w1", author, text, "done", nil)
	eventually(t, 5*time.Second, "the editor prompted", func() bool { return len(te.host.promptsOf(editor)) == 1 })
	delivered := te.host.promptsOf(editor)[0].text
	_, path, ok := strings.Cut(delivered, "Full text: ")
	if !ok || len(delivered) > maxBrief+deliveredHead+1024 {
		t.Fatalf("delivered %d bytes", len(delivered))
	}
	whole, err := os.ReadFile(path)
	if err != nil || !strings.HasSuffix(string(whole), "THE END") || !strings.HasPrefix(string(whole), "[canvas] from «Author» ("+res.IDs["0"]+"):\n") {
		t.Fatalf("the file %s: %d bytes, %v", path, len(whole), err)
	}
}

func TestAnEdgeWhoseDeliveriesFailTurnsOff(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Breaker")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Talker","config":{"output":"all"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Gone"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	talker, edge := sessionOf(t, c, res.IDs["0"]), res.IDs["2"]
	sessionOf(t, c, res.IDs["1"])
	te.host.setFailing(errors.New("the session is gone"))
	for i := range breakerFailures {
		te.RunFinished("w1", talker, fmt.Sprintf("try %d", i), "done", nil)
		eventually(t, 5*time.Second, "a delivery failed", func() bool { return len(messagesIn(c, MessageDropped)) == i+1 })
	}
	eventually(t, 5*time.Second, "the edge off", func() bool {
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.doc.edge(edge).Mode == ModeOff
	})
	if !noticed(c, "turned off") {
		t.Fatal("no notice of the edge turned off")
	}
	if dropped := messagesIn(c, MessageDropped)[0]; !strings.Contains(dropped.Reason, "the session is gone") {
		t.Fatalf("dropped %+v", dropped)
	}
}

func TestAnEdgeDeliversSoManyAMinute(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Rate")
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"settings":{"routing":{"edge_rate_per_minute":1}}}`, "", nil)
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Fast","config":{"output":"all"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Slow"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	fast, slow := sessionOf(t, c, res.IDs["0"]), sessionOf(t, c, res.IDs["1"])
	te.RunFinished("w1", fast, "one", "done", nil)
	eventually(t, 5*time.Second, "the first delivered", func() bool { return len(te.host.promptsOf(slow)) == 1 })
	te.RunFinished("w1", fast, "two", "done", nil)
	eventually(t, 5*time.Second, "the second held", func() bool {
		waiting := messagesIn(c, MessagePending)
		return len(waiting) == 1 && strings.HasPrefix(waiting[0].Reason, "rate-limited")
	})
	if len(te.host.promptsOf(slow)) != 1 {
		t.Fatal("over the rate")
	}
}

func TestTheForemanIsMadeByTheFirstQuestion(t *testing.T) {
	te := newEngine(t)
	var asked struct {
		Node    string `json:"node"`
		Created bool   `json:"created"`
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/fresh-canvas/foreman", `{"text":"Make two shells","title":"Shells"}`, "", &asked)
	c := te.canvas(t, "fresh-canvas")
	n := c.node(asked.Node)
	if !asked.Created || n == nil || n.Preset != "foreman" || n.Access != ScopeAdmin || n.Title != "Foreman" || c.summary().Title != "Shells" {
		t.Fatalf("asked %+v, node %+v", asked, n)
	}
	session := sessionOf(t, c, asked.Node)
	eventually(t, 5*time.Second, "the foreman prompted", func() bool { return len(te.host.promptsOf(session)) == 1 })
	if text := te.host.promptsOf(session)[0].text; !strings.Contains(text, "You are the canvas's foreman") || !strings.HasSuffix(text, "\n\nMake two shells") {
		t.Fatalf("the foreman was told %q", text)
	}
	asked.Created = false
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/fresh-canvas/foreman", `{"text":"And a note"}`, "", &asked)
	eventually(t, 5*time.Second, "the foreman asked again", func() bool { return len(te.host.promptsOf(session)) == 2 })
	if asked.Created || asked.Node != n.ID || te.host.promptsOf(session)[1].text != "And a note" {
		t.Fatalf("asked again: %+v, %q", asked, te.host.promptsOf(session)[1].text)
	}
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases/fresh-canvas/foreman", `{"text":"  "}`, ""); status != http.StatusBadRequest {
		t.Fatalf("asked nothing: %d", status)
	}
}

func TestProgramsOfNodesActWithinTheirAccess(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Tokens")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"node.add","node":{"kind":"agent","title":"Other"}},
		{"op":"node.add","node":{"preset":"webhook","title":"Hook"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	lead, other, hook := res.IDs["0"], res.IDs["1"], res.IDs["2"]
	otherSession := sessionOf(t, c, other)

	for _, token := range []string{"", "kc1.x.n_aaaaaa.0.talk.forged", te.token(t, c, lead, ScopeTalk) + "x"} {
		if status, _ := te.call(t, "GET", "/api/canvas/view", "", token); status != http.StatusUnauthorized {
			t.Fatalf("token %q: %d", token, status)
		}
	}

	talk := te.token(t, c, lead, ScopeTalk)
	var v View
	te.must(t, http.StatusOK, "GET", "/api/canvas/view", "", talk, &v)
	if v.Me == nil || v.Me.ID != lead || v.Scope != ScopeTalk || len(v.Nodes) != 3 || len(v.Edges) != 1 || len(v.Free) == 0 || v.Canvas.ID != id {
		t.Fatalf("view %+v", v)
	}
	var self Self
	te.must(t, http.StatusOK, "GET", "/api/canvas/self", "", talk, &self)
	if self.Node.ID != lead || self.Scope != ScopeTalk || !strings.Contains(self.Brief, "«Lead»") || self.Workspace != te.host.ws.Path ||
		self.Page != te.srv.URL+"/#/w/w1/c/"+id {
		t.Fatalf("self %+v", self)
	}
	if status, _ := te.call(t, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"note"}}]}`, talk); status != http.StatusForbidden {
		t.Fatalf("talk built: %d", status)
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+other+"/send", `{"text":"hello from lead"}`, talk, nil)
	eventually(t, 5*time.Second, "Other prompted", func() bool { return len(te.host.promptsOf(otherSession)) == 1 })
	if text := te.host.promptsOf(otherSession)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Lead» ("+lead+"):\nhello from lead") {
		t.Fatalf("Other was told %q", text)
	}

	// Its output, as its own program says it.
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"text":"report"}`, talk, nil)
	eventually(t, 5*time.Second, "the output delivered", func() bool { return len(te.host.promptsOf(otherSession)) == 2 })
	if text := te.host.promptsOf(otherSession)[1].text; text != "[canvas] from «Lead» ("+lead+"):\nreport" {
		t.Fatalf("Other was given %q", text)
	}
	if status, _ := te.call(t, "POST", "/api/canvas/emit", `{"port":"nowhere","text":"x"}`, talk); status != http.StatusBadRequest {
		t.Fatalf("an output of no port: %d", status)
	}

	// build makes nodes beside itself, and removes only its own.
	build := te.token(t, c, lead, ScopeBuild)
	var made Result
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"agent","title":"Helper","connect":{"from":"self"}}}]}`, build, &made)
	helper := made.IDs["0"]
	h, l := c.node(helper), c.node(lead)
	if h.CreatedBy != lead || h.Access != ScopeTalk || h.Depth != 1 || made.Placed[helper].X < l.X+l.W {
		t.Fatalf("helper %+v at %+v", h, made.Placed[helper])
	}
	if v := c.view(lead, ScopeBuild, false); !slices.ContainsFunc(v.Edges, func(e ViewEdge) bool { return e.From == lead+":out" && e.To == helper+":in" }) {
		t.Fatalf("not connected: %+v", v.Edges)
	}
	for _, forbidden := range []string{
		`{"ops":[{"op":"node.remove","id":"` + other + `"}]}`,
		`{"ops":[{"op":"node.remove","id":"self"}]}`,
		`{"ops":[{"op":"node.update","id":"` + other + `","set":{"access":"build"}}]}`,
		`{"ops":[{"op":"canvas.update","set":{"title":"Mine"}}]}`,
	} {
		if status, body := te.call(t, "POST", "/api/canvas/ops", forbidden, build); status != http.StatusForbidden {
			t.Fatalf("%s: %d %s", forbidden, status, body)
		}
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.remove","id":"`+helper+`","grace_s":0}]}`, build, nil)

	// Nodes agents make wait for the user when the canvas says so.
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"settings":{"autonomy":{"spawn":"ask"}}}`, "", nil)
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"agent","title":"Proposed"}}]}`, build, &made)
	proposed := made.IDs["0"]
	if n := c.node(proposed); !n.Proposed || n.Runtime.Session != "" || c.status(proposed).State != StatePaused {
		t.Fatalf("proposed %+v, %s", n, c.status(proposed).State)
	}
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", te.token(t, c, proposed, ScopeTalk)); status != http.StatusForbidden {
		t.Fatalf("a proposed node's program: %d", status)
	}
	if status, _ := te.call(t, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.update","id":"`+proposed+`","set":{"proposed":false}}]}`, build); status != http.StatusForbidden {
		t.Fatalf("an agent approved: %d", status)
	}
	te.ops(t, id, `[{"op":"node.update","id":"`+proposed+`","set":{"proposed":false}}]`)
	sessionOf(t, c, proposed)
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"settings":{"autonomy":{"spawn":"deny"}}}`, "", nil)
	if status, _ := te.call(t, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"note"}}]}`, build); status != http.StatusForbidden {
		t.Fatalf("spawn denied: %d", status)
	}

	// A token is no more than its node's access now.
	te.ops(t, id, `[{"op":"node.update","id":"`+lead+`","set":{"access":"observe"}}]`)
	if status, _ := te.call(t, "POST", "/api/canvas/nodes/"+other+"/send", `{"text":"x"}`, build); status != http.StatusForbidden {
		t.Fatalf("an observer sent: %d", status)
	}
	var waited WaitResult
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/"+other+"/wait?until=idle&timeout=2", "", build, &waited)
	if waited.Node != other || waited.TimedOut {
		t.Fatalf("waited %+v", waited)
	}
	var read map[string]any
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/self/read?what=answer", "", build, &read)
	if read["node"] != lead {
		t.Fatalf("self read %v", read)
	}

	// A new token voids the old ones.
	var rotated struct {
		Epoch int `json:"epoch"`
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+lead+"/token", "", "", &rotated)
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", build); rotated.Epoch != 1 || status != http.StatusUnauthorized {
		t.Fatalf("epoch %d, the old token: %d", rotated.Epoch, status)
	}
	te.must(t, http.StatusOK, "GET", "/api/canvas/view", "", te.token(t, c, lead, ScopeBuild), nil)

	// A source's process gives its node's events, and does nothing else.
	source := te.token(t, c, hook, scopeSource)
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", source); status != http.StatusForbidden {
		t.Fatalf("a source viewed: %d", status)
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"text":"event","status":"ok"}`, source, nil)
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", mintToken(te.secret, claims{Canvas: id, Node: lead, Scope: scopeSource})); status != http.StatusUnauthorized {
		t.Fatalf("a source's token of an agent: %d", status)
	}

	// The node gone, so are its tokens.
	te.ops(t, id, `[{"op":"node.remove","id":"`+lead+`","grace_s":0}]`)
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", te.token(t, c, other, ScopeTalk)); status != http.StatusOK {
		t.Fatalf("another node's token: %d", status)
	}
	if status, _ := te.call(t, "GET", "/api/canvas/view", "", talk); status != http.StatusUnauthorized {
		t.Fatalf("a removed node's token: %d", status)
	}
}

func TestSourcesOfTheCanvas(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Sources")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"webhook","title":"Deploys"}},
		{"op":"node.add","node":{"preset":"files","title":"Notes","config":{"pattern":"*.txt","interval":"400ms"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Watcher"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$2"},"template":"deploy {{data.sha}}: {{text}}"}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$2"},"template":"{{title}}: {{text}}"}}
	]`)
	hook := res.IDs["0"]
	session := sessionOf(t, c, res.IDs["2"])

	var snap snapshot
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id, "", "", &snap)
	address := snap.Hooks[hook]
	if !strings.HasPrefix(address, "/api/canvas/hooks/") {
		t.Fatalf("hooks %v", snap.Hooks)
	}
	te.must(t, http.StatusAccepted, "POST", address, `{"text":"done","sha":"abc123"}`, "", nil)
	eventually(t, 5*time.Second, "the hook's event delivered", func() bool { return len(te.host.promptsOf(session)) == 1 })
	if text := te.host.promptsOf(session)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Deploys» ("+hook+"):\ndeploy abc123: done") {
		t.Fatalf("delivered %q", text)
	}
	if status, _ := te.call(t, "POST", "/api/canvas/hooks/not-a-hook", "{}", ""); status != http.StatusNotFound {
		t.Fatalf("no such hook: %d", status)
	}

	time.Sleep(500 * time.Millisecond) // the files source looks a first time
	if err := os.WriteFile(filepath.Join(te.host.ws.Path, "todo.txt"), []byte("milk"), 0o600); err != nil {
		t.Fatal(err)
	}
	eventually(t, 10*time.Second, "the files' event delivered", func() bool { return len(te.host.promptsOf(session)) == 2 })
	if text := te.host.promptsOf(session)[1].text; text != "[canvas] from «Notes» ("+res.IDs["1"]+"):\n1 files changed: todo.txt" {
		t.Fatalf("delivered %q", text)
	}

	// Paused, the sources stop.
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"live":false}`, "", nil)
	if state := c.status(hook).State; state != StateStopped {
		t.Fatalf("a paused canvas's source is %s", state)
	}
	if status, _ := te.call(t, "POST", address, "{}", ""); status != http.StatusConflict {
		t.Fatalf("the hook of a paused canvas: %d", status)
	}
	te.must(t, http.StatusOK, "PATCH", "/api/w/w1/canvases/"+id, `{"live":true}`, "", nil)
	te.must(t, http.StatusAccepted, "POST", address, `plain text`, "", nil)
	eventually(t, 5*time.Second, "the hook's event delivered again", func() bool { return len(te.host.promptsOf(session)) == 3 })
}

func TestAPluginsSourceRunsItsProgram(t *testing.T) {
	host := newHost(t, nil)
	dir := t.TempDir()
	script := `#!/bin/sh
read -r input
case "$input" in *'"first_run":true'*) first=yes ;; *) first=no ;; esac
echo '{"type":"log","text":"started"}'
echo '{"type":"event","port":"opened","key":"1","title":"#1","text":"first issue","data":{"number":1}}'
echo '{"type":"event","port":"opened","key":"1","title":"#1","text":"the same again"}'
echo '{"type":"event","port":"nowhere","text":"lost"}'
echo "{\"type\":\"event\",\"port\":\"opened\",\"key\":\"2\",\"title\":\"#2\",\"text\":\"first run: $first, state in $KOU_CANVAS_STATE_DIR\"}"
echo 'not json'
echo '{"type":"status","state":"ok","text":"watching"}'
cat >/dev/null
`
	if err := os.MkdirAll(filepath.Join(dir, "bin"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "bin", "issues"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "github-events", Canvas: &plugin.Canvas{Sources: []plugin.CanvasSource{{
			ID: "issues", Title: "Issues", Run: []string{"./bin/issues"}, Mode: "stream",
			Outputs:  []plugin.CanvasPort{{ID: "opened", Title: "Opened"}},
			Template: "Issue {{data.number}}: {{text}}",
		}}}},
		Directory: dir, Files: os.DirFS(dir), Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Plugin")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"github-events/issues","config":{"repo":"o/r"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Triage"}},
		{"op":"edge.add","edge":{"from":{"node":"$0","port":"opened"},"to":{"node":"$1"},"template":"{{title}} {{text}} {{data.number}}"}}
	]`)
	source := res.IDs["0"]
	if n := c.node(source); n.Kind != KindSource || n.Plugin != "github-events" || n.Preset != "issues" || n.Title != "Issues" {
		t.Fatalf("source %+v", n)
	}
	// An edge that says no template reads the events as the source says.
	plain := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Reader"}},{"op":"edge.add","edge":{"from":{"node":"`+source+`","port":"opened"},"to":{"node":"$0"}}}]`).IDs["1"]
	c.mu.Lock()
	template := c.doc.edge(plain).Template
	c.mu.Unlock()
	if template != "Issue {{data.number}}: {{text}}" {
		t.Fatalf("the edge's template: %q", template)
	}
	session := sessionOf(t, c, res.IDs["1"])
	eventually(t, 10*time.Second, "two events delivered", func() bool { return len(te.host.promptsOf(session)) == 2 })
	prompts := te.host.promptsOf(session)
	state := stateDir(te.host.ws.Path, id, source)
	if !strings.HasSuffix(prompts[0].text, "#1 first issue 1") || prompts[1].text != "[canvas] from «Issues» ("+source+"):\n#2 first run: yes, state in "+state+" " {
		t.Fatalf("delivered %q and %q", prompts[0].text, prompts[1].text)
	}
	eventually(t, 5*time.Second, "the source's status", func() bool { return c.status(source).Detail == "watching" })
	var read struct {
		Text string `json:"text"`
	}
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/read?what=tail", "", "", &read)
	for _, line := range []string{"started", "an event on a port the source does not declare: nowhere", "not a line of JSON: not json"} {
		if !strings.Contains(read.Text, line) {
			t.Errorf("its log has no %q:\n%s", line, read.Text)
		}
	}

	// The node removed, its program is told to stop, and ends.
	te.ops(t, id, `[{"op":"node.remove","id":"`+source+`","grace_s":0}]`)
	eventually(t, 5*time.Second, "its state gone", func() bool {
		_, err := os.Stat(state)
		return errors.Is(err, fs.ErrNotExist)
	})
}

func TestASourceWaitsWhileItsPluginIsOff(t *testing.T) {
	host := newHost(t, nil)
	dir := t.TempDir()
	script := `#!/bin/sh
read -r input
echo '{"type":"status","state":"ok","text":"watching"}'
cat >/dev/null
`
	if err := os.WriteFile(filepath.Join(dir, "watch"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "events", Canvas: &plugin.Canvas{Sources: []plugin.CanvasSource{{
			ID: "watch", Title: "Watch", Run: []string{"./watch"}, Mode: "stream",
			Outputs: []plugin.CanvasPort{{ID: "out"}},
		}}}},
		Directory: dir, Files: os.DirFS(dir), Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Plugins")
	source := te.ops(t, id, `[{"op":"node.add","node":{"preset":"events/watch"}}]`).IDs["0"]
	eventually(t, 10*time.Second, "the source runs", func() bool { return c.status(source).Detail == "watching" })

	// Its plugin turned off, its program stops and the node waits for it.
	te.host.setActive("events", false)
	te.forgetPresets()
	eventually(t, 10*time.Second, "the source stopped", func() bool {
		st := c.status(source)
		return st.State == StateStopped && st.Detail == "its plugin is off"
	})

	// Back on, it runs again.
	te.host.setActive("events", true)
	te.forgetPresets()
	eventually(t, 10*time.Second, "the source runs again", func() bool { return c.status(source).Detail == "watching" })
}

func TestEventsFollowTheCanvas(t *testing.T) {
	te := newEngine(t)
	id, _ := te.newCanvas(t, "Events")

	type event struct {
		id   string
		data map[string]any
	}
	open := func(last string) (*bufio.Reader, func()) {
		t.Helper()
		ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
		req, _ := http.NewRequestWithContext(ctx, "GET", te.srv.URL+"/api/w/w1/canvases/"+id+"/events", nil)
		if last != "" {
			req.Header.Set("Last-Event-ID", last)
		}
		res, err := te.srv.Client().Do(req)
		if err != nil {
			t.Fatal(err)
		}
		if res.StatusCode != http.StatusOK || !strings.HasPrefix(res.Header.Get("Content-Type"), "text/event-stream") {
			t.Fatalf("events: %d %s", res.StatusCode, res.Header.Get("Content-Type"))
		}
		return bufio.NewReader(res.Body), func() { res.Body.Close(); cancel() }
	}
	next := func(r *bufio.Reader) event {
		t.Helper()
		var ev event
		for {
			line, err := r.ReadString('\n')
			if err != nil {
				t.Fatalf("the stream ended: %v", err)
			}
			line = strings.TrimRight(line, "\n")
			switch {
			case line == "" && ev.data != nil:
				return ev
			case strings.HasPrefix(line, "id: "):
				ev.id = strings.TrimPrefix(line, "id: ")
			case strings.HasPrefix(line, "data: "):
				if err := json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &ev.data); err != nil {
					t.Fatal(err)
				}
			}
		}
	}
	until := func(r *bufio.Reader, kind string) event {
		t.Helper()
		for {
			if ev := next(r); ev.data["type"] == kind {
				return ev
			}
		}
	}

	r, done := open("")
	snap := next(r)
	if snap.data["type"] != "snapshot" || snap.id == "" {
		t.Fatalf("first %+v", snap)
	}
	te.ops(t, id, `[{"op":"node.add","node":{"preset":"note","title":"Hi"}}]`)
	ops := until(r, "ops")
	changes, _ := ops.data["changes"].([]any)
	if ops.data["rev"] != float64(1) || len(changes) != 1 || changes[0].(map[string]any)["op"] != "node.add" {
		t.Fatalf("ops %+v", ops.data)
	}
	if actor, _ := ops.data["actor"].(map[string]any); actor["kind"] != "user" {
		t.Fatalf("actor %v", ops.data["actor"])
	}
	done()

	// A page that comes back is given what it missed; one of another run
	// of the server starts over.
	r, done = open(snap.id)
	if again := next(r); again.data["type"] != "ops" || again.id != ops.id {
		t.Fatalf("resumed with %+v", again)
	}
	done()
	r, done = open("another-run-1")
	if again := next(r); again.data["type"] != "snapshot" {
		t.Fatalf("another run's ID: %+v", again)
	}
	defer done()

	// The canvas deleted, the stream says so and ends.
	te.must(t, http.StatusNoContent, "DELETE", "/api/w/w1/canvases/"+id, "", "", nil)
	until(r, "deleted")
}

func TestTemplatesMakeCanvasesAndCanvasesTemplates(t *testing.T) {
	te := newEngine(t)
	var made struct {
		Canvas Summary `json:"canvas"`
		Result *Result `json:"result"`
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases", `{"template":"issue-triage"}`, "", &made)
	if made.Canvas.Title != "Issue triage" || made.Result == nil || len(made.Result.IDs) != 4 {
		t.Fatalf("made %+v", made)
	}
	c := te.canvas(t, made.Canvas.ID)
	c.mu.Lock()
	nodes, edges := len(c.doc.Nodes), len(c.doc.Edges)
	triage := *c.doc.Nodes[1]
	c.mu.Unlock()
	if nodes != 3 || edges != 1 || triage.X != 400 || triage.Y != 0 || triage.configString("instructions") == "" {
		t.Fatalf("%d nodes, %d edges, triage %+v", nodes, edges, triage)
	}

	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+made.Canvas.ID+"/template", `{"name":"My board","description":"mine"}`, "", nil)
	if _, err := os.Stat(filepath.Join(te.host.ws.Path, ".harness", "canvas-templates", "my-board.json")); err != nil {
		t.Fatal(err)
	}
	var list struct {
		Templates []map[string]any `json:"templates"`
	}
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvas/templates", "", "", &list)
	var ids []string
	for _, item := range list.Templates {
		ids = append(ids, item["id"].(string))
	}
	if !slices.Equal(ids, []string{"pair", "fan-out", "issue-triage", "empty", "workspace/my-board"}) {
		t.Fatalf("templates %v", ids)
	}
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases", `{"template":"workspace/my-board","title":"Copy"}`, "", &made)
	if made.Canvas.Title != "Copy" || made.Canvas.Nodes != 3 {
		t.Fatalf("a copy %+v", made.Canvas)
	}
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases", `{"template":"nothing"}`, ""); status != http.StatusNotFound {
		t.Fatalf("no such template: %d", status)
	}
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases/"+made.Canvas.ID+"/template", `{"name":"../up"}`, ""); status != http.StatusBadRequest {
		t.Fatalf("a template's name that is a path: %d", status)
	}

	var kinds struct {
		Harnesses []map[string]any `json:"harnesses"`
		Sources   []map[string]any `json:"sources"`
	}
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvas/kinds", "", "", &kinds)
	var names []string
	for _, item := range append(kinds.Harnesses, kinds.Sources...) {
		names = append(names, item["id"].(string))
	}
	if !slices.Equal(names, []string{"shell", "command", "manual", "timer", "files", "webhook"}) {
		t.Fatalf("kinds %v", names)
	}
}
