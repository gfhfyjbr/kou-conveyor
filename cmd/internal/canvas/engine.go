package canvas

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Workspace is a workspace of the server, as the engine knows it.
type Workspace struct {
	ID         string
	Path       string
	SessionDir string
}

// Host is the server the engine runs in: its workspaces, shells, plugins
// and agents' sessions.
type Host interface {
	// Workspaces lists the workspaces; Workspace finds one by its ID, ""
	// naming the one the server started in.
	Workspaces() []Workspace
	Workspace(id string) (Workspace, bool)
	// Terminals are the server's shells.
	Terminals() *terminal.Manager
	// Plugins finds a workspace's plugins, the cockpit's own among them.
	Plugins(ws Workspace) plugin.Found
	// Enqueue gives an agent's session a prompt: it runs at once when
	// nothing else does, else waits its turn, or, forced, goes to the agent
	// at work. id, a UUID, is the prompt's: RunFinished names the prompts a
	// run answered by it. model, when not empty, is the model it runs with.
	Enqueue(ctx context.Context, ws Workspace, session, id, text, model string, force bool) error
	// StopRun stops the session's run, if one goes on; RunActive reports
	// whether one does.
	StopRun(ws Workspace, session string) bool
	RunActive(ws Workspace, session string) bool
}

// Options set an engine up.
type Options struct {
	Host Host
	// URL is where the programs of the nodes reach the server.
	URL string
	// SecretFile keeps the key tokens are signed with.
	SecretFile string
	// BinDir holds kou-canvas, and goes first on the PATH of the nodes'
	// programs; CLI is kou-canvas's own path there.
	BinDir string
	CLI    string
	// LaunchDir is where the files presets give their programs are
	// written.
	LaunchDir string
	// Logf logs what goes wrong in the background; log.Printf when nil.
	Logf func(format string, args ...any)
}

// Engine keeps the canvases of the server's workspaces. Its methods may be
// called on a nil engine — a server whose canvases are off — and do nothing
// then.
type Engine struct {
	o        Options
	ctx      context.Context
	cancel   context.CancelFunc
	secret   []byte
	instance string

	mu       sync.Mutex
	canvases map[string]*Canvas    // by canvasKey
	sessions map[string]sessionRef // agents' sessions, by sessionKey
	presets  map[string]presetCache
	closed   bool
}

type sessionRef struct {
	canvas *Canvas
	node   string
}

func canvasKey(ws, id string) string       { return ws + "/" + id }
func sessionKey(ws, session string) string { return ws + "/" + session }

// New makes an engine; Start loads the canvases that run.
func New(ctx context.Context, o Options) (*Engine, error) {
	if o.Host == nil {
		return nil, errors.New("canvas: no host")
	}
	if o.Logf == nil {
		o.Logf = log.Printf
	}
	secret, err := LoadSecret(o.SecretFile)
	if err != nil {
		return nil, fmt.Errorf("canvas: the secret of its tokens: %w", err)
	}
	ctx, cancel := context.WithCancel(ctx)
	return &Engine{
		o: o, ctx: ctx, cancel: cancel, secret: secret,
		instance: fmt.Sprintf("%x", time.Now().UnixNano()),
		canvases: map[string]*Canvas{}, sessions: map[string]sessionRef{}, presets: map[string]presetCache{},
	}, nil
}

// ownerPrefix starts the owner of a canvas's shells.
const ownerPrefix = "canvas:"

func ownerOf(ws, canvas, node string) string { return ownerPrefix + ws + "/" + canvas + "/" + node }

// Start loads the canvases that are live, and those whose shells a build
// before this one handed over; the shells of canvases that are gone end.
func (e *Engine) Start() {
	if e == nil {
		return
	}
	owned := map[string][]string{}
	for _, info := range e.o.Host.Terminals().ListOwned(ownerPrefix) {
		parts := strings.Split(strings.TrimPrefix(info.Owner, ownerPrefix), "/")
		if len(parts) == 3 {
			key := canvasKey(parts[0], parts[1])
			owned[key] = append(owned[key], info.ID)
		}
	}
	loaded := map[string]bool{}
	for _, ws := range e.o.Host.Workspaces() {
		for _, id := range listIDs(ws.Path) {
			key := canvasKey(ws.ID, id)
			if len(owned[key]) == 0 {
				doc, _, err := loadDoc(ws.Path, id)
				if err != nil || !doc.Live {
					continue
				}
			}
			if _, err := e.get(ws, id); err != nil {
				e.o.Logf("canvas %s: %v", id, err)
				continue
			}
			loaded[key] = true
		}
	}
	for key, terminals := range owned {
		if loaded[key] {
			continue
		}
		for _, id := range terminals {
			_ = e.o.Host.Terminals().Kill(id)
		}
	}
}

// workspace finds a workspace by its ID.
func (e *Engine) workspace(id string) (Workspace, bool) { return e.o.Host.Workspace(id) }

// get returns a canvas, loading it.
func (e *Engine) get(ws Workspace, id string) (*Canvas, error) {
	if !ValidID(id) {
		return nil, fs.ErrNotExist
	}
	key := canvasKey(ws.ID, id)
	e.mu.Lock()
	if c := e.canvases[key]; c != nil {
		e.mu.Unlock()
		return c, nil
	}
	closed := e.closed
	e.mu.Unlock()
	if closed {
		return nil, errors.New("the server is shutting down")
	}
	doc, readOnly, err := loadDoc(ws.Path, id)
	if err != nil {
		return nil, err
	}
	return e.adopt(ws, doc, readOnly)
}

// adopt takes a document up as a loaded canvas, unless another took it up
// meanwhile.
func (e *Engine) adopt(ws Workspace, doc *Doc, readOnly bool) (*Canvas, error) {
	key := canvasKey(ws.ID, doc.ID)
	e.mu.Lock()
	if c := e.canvases[key]; c != nil {
		e.mu.Unlock()
		return c, nil
	}
	if e.closed {
		e.mu.Unlock()
		return nil, errors.New("the server is shutting down")
	}
	c := newCanvas(e, ws, doc, readOnly)
	e.canvases[key] = c
	e.mu.Unlock()
	c.boot()
	return c, nil
}

// create makes a canvas.
func (e *Engine) create(ws Workspace, id, title string) (*Canvas, error) {
	if !ValidID(id) {
		return nil, errors.New("a canvas's ID is letters, digits and dashes")
	}
	if c, err := e.get(ws, id); err == nil {
		return c, nil
	} else if !errors.Is(err, fs.ErrNotExist) {
		return nil, err
	}
	now := time.Now().UTC().Truncate(time.Millisecond)
	title = cleanTitle(title)
	if title == "" {
		title = "Canvas"
	}
	doc := &Doc{
		Version: Version, ID: id, Title: title, CreatedAt: now, UpdatedAt: now, Live: true,
		Settings: DefaultSettings(), Nodes: []*Node{}, Edges: []*Edge{},
	}
	if err := saveDoc(ws.Path, doc); err != nil {
		return nil, err
	}
	return e.adopt(ws, doc, false)
}

// forget drops a canvas that was deleted.
func (e *Engine) forget(c *Canvas) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.canvases[canvasKey(c.ws.ID, c.id)] == c {
		delete(e.canvases, canvasKey(c.ws.ID, c.id))
	}
	for key, ref := range e.sessions {
		if ref.canvas == c {
			delete(e.sessions, key)
		}
	}
}

// indexSession records the session of an agent node.
func (e *Engine) indexSession(c *Canvas, node, session string) {
	if session == "" {
		return
	}
	e.mu.Lock()
	e.sessions[sessionKey(c.ws.ID, session)] = sessionRef{c, node}
	e.mu.Unlock()
}

// unindexSession forgets the session of a node gone.
func (e *Engine) unindexSession(c *Canvas, session string) {
	e.mu.Lock()
	if ref, ok := e.sessions[sessionKey(c.ws.ID, session)]; ok && ref.canvas == c {
		delete(e.sessions, sessionKey(c.ws.ID, session))
	}
	e.mu.Unlock()
}

// session finds the node whose agent a session is. load has a canvas not
// loaded yet loaded, as its session's metadata names it.
func (e *Engine) session(wsID, session string, load bool) (sessionRef, bool) {
	e.mu.Lock()
	ref, ok := e.sessions[sessionKey(wsID, session)]
	e.mu.Unlock()
	if ok || !load {
		return ref, ok
	}
	ws, found := e.workspace(wsID)
	if !found || !cockpit.ValidSessionID(session) {
		return sessionRef{}, false
	}
	meta := cockpit.LoadMeta(ws.SessionDir, session)
	if meta.Canvas == "" {
		return sessionRef{}, false
	}
	if _, err := e.get(ws, meta.Canvas); err != nil {
		return sessionRef{}, false
	}
	e.mu.Lock()
	ref, ok = e.sessions[sessionKey(wsID, session)]
	e.mu.Unlock()
	return ref, ok
}

// RunEnv is what the runner of a session adds to its environment: for the
// agent of a canvas's node, the canvas's variables, and its sandbox.
func (e *Engine) RunEnv(wsID, session string) []string {
	if e == nil {
		return nil
	}
	ref, ok := e.session(wsID, session, true)
	if !ok {
		return nil
	}
	return ref.canvas.agentEnv(ref.node)
}

// RunStarted is told when a run of a session starts.
func (e *Engine) RunStarted(wsID, session string) {
	if e == nil {
		return
	}
	if ref, ok := e.session(wsID, session, false); ok {
		ref.canvas.agentRunStarted(ref.node)
	}
}

// RunEntry is told of an entry of a run's transcript, as it changes: it
// must not be kept, as the run goes on changing it.
func (e *Engine) RunEntry(wsID, session string, entry *cockpit.Entry) {
	if e == nil || entry == nil {
		return
	}
	if ref, ok := e.session(wsID, session, false); ok {
		ref.canvas.agentEntry(ref.node, entry)
	}
}

// RunActivity is told what a run does now.
func (e *Engine) RunActivity(wsID, session, activity string) {
	if e == nil {
		return
	}
	if ref, ok := e.session(wsID, session, false); ok {
		ref.canvas.agentActivity(ref.node, activity)
	}
}

// RunFinished is told when a run of a session ends: its answer, how it
// ended (done, stopped or failed), and the IDs of the prompts it answered,
// as Host.Enqueue was given them — the user's prompts have others.
func (e *Engine) RunFinished(wsID, session, answer, outcome string, prompts []string) {
	if e == nil {
		return
	}
	if ref, ok := e.session(wsID, session, false); ok {
		ref.canvas.agentRunFinished(ref.node, answer, outcome, prompts)
	}
}

// all lists the canvases loaded.
func (e *Engine) all() []*Canvas {
	e.mu.Lock()
	defer e.mu.Unlock()
	out := make([]*Canvas, 0, len(e.canvases))
	for _, c := range e.canvases {
		out = append(out, c)
	}
	return out
}

// Live counts the live canvases of a workspace, "" for all of them.
func (e *Engine) Live(wsID string) int {
	if e == nil {
		return 0
	}
	n := 0
	for _, c := range e.all() {
		if wsID != "" && c.ws.ID != wsID {
			continue
		}
		c.mu.Lock()
		if c.doc.Live && !c.deleted {
			n++
		}
		c.mu.Unlock()
	}
	return n
}

// Flush writes what waits to be written: before the server execs a new
// build of itself, and when it stops.
func (e *Engine) Flush() {
	if e == nil {
		return
	}
	for _, c := range e.all() {
		c.flush()
	}
}

// Close flushes the canvases and stops what runs for them, but their
// shells: the server ends those itself.
func (e *Engine) Close() {
	if e == nil {
		return
	}
	e.mu.Lock()
	e.closed = true
	e.mu.Unlock()
	for _, c := range e.all() {
		c.shutdown()
	}
	e.cancel()
}

// env builds a program's environment: the canvas's variables, with
// kou-canvas first on its PATH, and a token of scope, unless it is "". A
// token of scope none still says what the node is, launches its harness
// and reports what the harness does.
func (e *Engine) env(c *Canvas, node, scope string, epoch int) []string {
	env := []string{
		"KOU_CANVAS_URL=" + e.o.URL,
		"KOU_CANVAS_ID=" + c.id,
		"KOU_CANVAS_NODE=" + node,
	}
	if scope != "" {
		env = append(env, "KOU_CANVAS_TOKEN="+mintToken(e.secret, claims{Canvas: c.id, Node: node, Epoch: epoch, Scope: scope}))
	}
	if e.o.BinDir != "" {
		env = append(env, "KOU_CANVAS_BIN="+e.o.BinDir, "PATH="+e.o.BinDir+string(os.PathListSeparator)+os.Getenv("PATH"))
	}
	if e.o.CLI != "" {
		env = append(env, "KOU_CANVAS_CLI="+e.o.CLI)
	}
	return env
}

// launchDir is where a node's program's files are written.
func (e *Engine) launchDir(c *Canvas, node string) string {
	base := e.o.LaunchDir
	if base == "" {
		base = filepath.Join(os.TempDir(), "kou-conveyor-canvas")
	}
	return filepath.Join(base, c.ws.ID, c.id, node)
}
