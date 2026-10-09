// Package terminal runs shells in pseudo-terminals for the browser
// cockpit's terminals. A shell outlives the pages that show it: a page
// attaches to it and detaches, and one that attaches again is given the
// output kept, to draw the screen anew. The server that runs the shells
// hands them over to the build that replaces it (Handover, Adopt), so they
// outlive the server's own restarts too.
package terminal

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"slices"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/creack/pty"
)

const (
	// ringSize is how much of a shell's output is kept for the pages that
	// attach to it.
	ringSize = 2 << 20
	// maxSessions bounds the shells one server runs.
	maxSessions = 64
	// exitedFor is how long a shell that ended stays, so that a page that
	// reconnects learns how it ended.
	exitedFor = 30 * time.Second
	// clientQueue is how many messages wait for a page that reads slowly;
	// one that falls further behind is let go, and attaches again.
	clientQueue = 1024
	// killGrace is how long a shell has to end after it is hung up on,
	// before it is killed.
	killGrace = 2 * time.Second
	// resizeCheck is when a size given is checked, and given again.
	resizeCheck = 150 * time.Millisecond
)

var (
	ErrNotFound = errors.New("no such terminal")
	ErrTooMany  = errors.New("too many terminals")
	ErrClosed   = errors.New("the server is shutting down")
)

// Manager keeps the shells.
type Manager struct {
	// files are the shell integration files, the terminal plugin's shell/
	// directory; nil for none.
	files func() (fs.FS, error)
	// version is what shells are told of the program (TERM_PROGRAM_VERSION).
	version string
	// environment is what shells start from: the server's.
	environment func() []string
	// base is where integration files are written.
	base string

	mu       sync.Mutex
	sessions map[string]*Session
	closed   bool
}

// NewManager returns a manager whose shells get their integration from
// files (the shell/ directory of the terminal plugin), read anew for each
// shell so that changes to it apply to the next.
func NewManager(files func() (fs.FS, error), version string) *Manager {
	return &Manager{
		files: files, version: version, environment: os.Environ, base: integrationBase(),
		sessions: make(map[string]*Session),
	}
}

// Spec says how to start a shell.
type Spec struct {
	// Workspace is the workspace the shell belongs to, for listing.
	Workspace string
	// Dir is where the shell starts.
	Dir string
	// Shell is the program; empty for the user's login shell.
	Shell string
	// Theme gives the shell kou-conveyor's integration and prompt.
	Theme      bool
	Cols, Rows int
	// Owner names what the shell belongs to besides the sidebar's tabs,
	// such as a node of a canvas ("canvas:<ws>/<canvas>/<node>"): List
	// leaves such shells out, ListOwned lists them.
	Owner string
	// Env adds variables to the shell's environment, after the server's.
	Env []string
}

// Info describes a shell.
type Info struct {
	ID        string    `json:"id"`
	Workspace string    `json:"workspace"`
	Shell     string    `json:"shell"`
	Theme     bool      `json:"theme,omitzero"`
	Title     string    `json:"title,omitzero"`
	Dir       string    `json:"cwd,omitzero"`
	Started   time.Time `json:"started_at"`
	Cols      int       `json:"cols"`
	Rows      int       `json:"rows"`
	Clients   int       `json:"clients"`
	// Running names the program in the foreground, when it is not the
	// shell: closing the terminal would end it.
	Running string `json:"running,omitzero"`
	// ClosingAt is when a shell whose tab closed ends, unless it is kept
	// (Keep): until then its tab can come back to it.
	ClosingAt time.Time `json:"closing_at,omitzero"`
	Exited    bool      `json:"exited,omitzero"`
	Code      int       `json:"exit_code,omitzero"`
	// Owner is what the shell belongs to, when it is not a tab's.
	Owner string `json:"owner,omitzero"`
}

// Session is a shell in a pseudo-terminal.
type Session struct {
	id, workspace, shell string
	owner                atomic.Value // string: Spec.Owner, until Disown
	theme                bool
	started              time.Time
	pid                  int
	master               *os.File
	proc                 *os.Process
	manager              *Manager

	writing sync.Mutex // one write to the terminal at a time
	// lastInput is when something was last typed into the terminal, in
	// Unix nanoseconds.
	lastInput atomic.Int64

	mu        sync.Mutex
	ring      *ring
	scan      *scanner // what the output said, all of it
	base      *scanner // what it said up to where the ring starts
	cols      int
	rows      int
	resized   int // how many times the page resized the terminal
	clients   map[*Client]struct{}
	observers map[int]func(Event)
	observed  int // the last observer's number
	exited    bool
	code      int
	closing   *time.Timer // ends the shell, once its tab closed
	closeAt   time.Time
	read      chan struct{} // closed once the output is all read
	ended     chan struct{} // closed once the shell ended and its output was sent
}

func newID() string {
	var b [12]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// Start starts a shell.
func (m *Manager) Start(spec Spec) (*Session, error) {
	m.mu.Lock()
	switch {
	case m.closed:
		m.mu.Unlock()
		return nil, ErrClosed
	case len(m.sessions) >= maxSessions:
		m.mu.Unlock()
		return nil, ErrTooMany
	}
	m.mu.Unlock()
	if spec.Cols <= 0 || spec.Cols > 1000 {
		spec.Cols = 80
	}
	if spec.Rows <= 0 || spec.Rows > 500 {
		spec.Rows = 24
	}
	shell := spec.Shell
	if shell == "" {
		shell = UserShell()
	}
	id := newID()
	var integration *Integration
	if spec.Theme && m.files != nil && kindOf(shell) != "" {
		if files, err := m.files(); err == nil {
			if dir, err := materialize(files, m.base); err == nil {
				integration = &Integration{Dir: dir}
			}
		}
	}
	l := command(shell, spec, id, m.version, m.environment(), integration)
	cmd := &exec.Cmd{Path: l.path, Args: l.args, Env: l.env, Dir: spec.Dir}
	master, err := pty.StartWithSize(cmd, &pty.Winsize{Cols: uint16(spec.Cols), Rows: uint16(spec.Rows)})
	if err != nil {
		return nil, err
	}
	s := &Session{
		id: id, workspace: spec.Workspace, shell: shell, theme: integration != nil, started: time.Now(),
		pid: cmd.Process.Pid, master: master, proc: cmd.Process, manager: m,
		cols: spec.Cols, rows: spec.Rows,
	}
	s.owner.Store(spec.Owner)
	s.init(nil)
	if !m.add(s) {
		s.hangUp()
		return nil, ErrClosed
	}
	s.run()
	return s, nil
}

// init sets up what a session keeps of its output, from what it kept
// before, if anything.
func (s *Session) init(kept *handedOver) {
	s.clients = make(map[*Client]struct{})
	s.observers = make(map[int]func(Event))
	s.read = make(chan struct{})
	s.ended = make(chan struct{})
	s.scan = newScanner()
	s.base = newScanner()
	s.ring = newRing(ringSize, func(p []byte) { s.base.feed(p) })
	if kept != nil {
		s.base.restoreModes(kept.Modes)
		s.base.title, s.base.dir = kept.Title, kept.Dir
		s.scan = s.base.clone()
		s.scan.feed(kept.Output)
		s.ring.write(kept.Output)
		s.scan.title, s.scan.dir = kept.Title, kept.Dir
	}
	// The marks of the output from here on are the observers'.
	s.scan.marking = true
}

// run reads the shell's output and waits for it to end.
func (s *Session) run() {
	go s.pump()
	go s.wait()
}

func (m *Manager) add(s *Session) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closed {
		return false
	}
	m.sessions[s.id] = s
	return true
}

// Get returns the session of an ID.
func (m *Manager) Get(id string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if s := m.sessions[id]; s != nil {
		return s, nil
	}
	return nil, ErrNotFound
}

// List describes the sessions of a workspace, or all of them for "", the
// oldest first. Sessions that have an owner (Spec.Owner) are not the
// sidebar's, and are left out.
func (m *Manager) List(workspace string) []Info {
	return m.list(func(s *Session) bool { return s.Owner() == "" && (workspace == "" || s.workspace == workspace) })
}

// ListOwned describes the sessions whose owner starts with prefix, the
// oldest first.
func (m *Manager) ListOwned(prefix string) []Info {
	return m.list(func(s *Session) bool { owner := s.Owner(); return owner != "" && strings.HasPrefix(owner, prefix) })
}

// Disown gives a session that has an owner to the sidebar: List lists it
// from now on.
func (m *Manager) Disown(id string) error {
	s, err := m.Get(id)
	if err != nil {
		return err
	}
	s.owner.Store("")
	return nil
}

func (m *Manager) list(keep func(*Session) bool) []Info {
	m.mu.Lock()
	sessions := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		if keep(s) {
			sessions = append(sessions, s)
		}
	}
	m.mu.Unlock()
	out := make([]Info, 0, len(sessions))
	for _, s := range sessions {
		out = append(out, s.Info())
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Started.Before(out[j].Started) })
	return out
}

// Kill hangs up on a session's shell, and kills it if it does not end.
func (m *Manager) Kill(id string) error {
	s, err := m.Get(id)
	if err != nil {
		return err
	}
	s.hangUp()
	return nil
}

// Close ends every shell, waiting a little for them.
func (m *Manager) Close() {
	m.mu.Lock()
	m.closed = true
	sessions := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		sessions = append(sessions, s)
	}
	m.mu.Unlock()
	for _, s := range sessions {
		s.hangUp()
	}
	deadline := time.After(killGrace + time.Second)
	for _, s := range sessions {
		select {
		case <-s.ended:
		case <-deadline:
			return
		}
	}
}

func (m *Manager) remove(s *Session) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.sessions[s.id] == s {
		delete(m.sessions, s.id)
	}
}

// ID is the session's.
func (s *Session) ID() string { return s.id }

// Info describes the session.
func (s *Session) Info() Info {
	running := s.running()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.exited {
		running = ""
	}
	return Info{
		ID: s.id, Workspace: s.workspace, Shell: s.shell, Theme: s.theme, Title: s.scan.title, Dir: s.scan.dir,
		Started: s.started, Cols: s.cols, Rows: s.rows, Clients: len(s.clients), Running: running, Exited: s.exited, Code: s.code,
		ClosingAt: s.closeAt, Owner: s.Owner(),
	}
}

// Owner is what the session belongs to, "" for a tab's.
func (s *Session) Owner() string {
	owner, _ := s.owner.Load().(string)
	return owner
}

// Ended is closed once the shell ended.
func (s *Session) Ended() <-chan struct{} { return s.ended }

// maxGrace bounds how long a closed tab's shell waits.
const maxGrace = 5 * time.Minute

// CloseAfter ends a session's shell after grace, unless Keep comes first:
// a tab closed can be opened again, to the same shell, meanwhile.
func (m *Manager) CloseAfter(id string, grace time.Duration) error {
	s, err := m.Get(id)
	if err != nil {
		return err
	}
	grace = min(max(grace, 0), maxGrace)
	if grace == 0 {
		s.hangUp()
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closing != nil {
		s.closing.Stop()
	}
	s.closeAt = time.Now().Add(grace).Truncate(time.Millisecond)
	s.closing = time.AfterFunc(grace, s.hangUp)
	return nil
}

// Keep keeps a shell CloseAfter would end.
func (m *Manager) Keep(id string) error {
	s, err := m.Get(id)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.exited {
		return ErrNotFound
	}
	if s.closing != nil {
		s.closing.Stop()
		s.closing = nil
	}
	s.closeAt = time.Time{}
	return nil
}

// running names the program in the foreground, if it is not the shell.
func (s *Session) running() string {
	group := s.foregroundGroup()
	if group <= 0 || group == s.pid {
		return ""
	}
	if name := processName(group); name != "" {
		return name
	}
	return "a program"
}

// foregroundGroup is the process group in the foreground of the shell's
// terminal; 0 once the shell ended, as its terminal is closed after (wait).
func (s *Session) foregroundGroup() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.exited {
		return 0
	}
	return foreground(s.master)
}

// Dir is where the shell is: where it last said it is, else the working
// directory of the program in the foreground, else the shell's.
func (s *Session) Dir() string {
	s.mu.Lock()
	dir := s.scan.dir
	s.mu.Unlock()
	if dir != "" {
		if info, err := os.Stat(dir); err == nil && info.IsDir() {
			return dir
		}
	}
	for _, pid := range []int{s.foregroundGroup(), s.pid} {
		if pid <= 0 {
			continue
		}
		if dir, err := processDir(pid); err == nil && dir != "" {
			return dir
		}
	}
	return ""
}

// Write types p into the terminal.
func (s *Session) Write(p []byte) error {
	s.writing.Lock()
	defer s.writing.Unlock()
	return s.writeAll(p)
}

// LastInput is when something was last typed into the terminal — by a
// page, a paste, keys —, zero before anything was: what the terminal shows
// right after is mostly its echo.
func (s *Session) LastInput() time.Time {
	if at := s.lastInput.Load(); at != 0 {
		return time.Unix(0, at)
	}
	return time.Time{}
}

// Program is a program in the foreground of a shell's terminal.
type Program struct {
	PID int
	// Name is the name it runs as; Args are its arguments, the first the
	// name it was run by, where the system says them.
	Name string
	Args []string
}

// Foreground describes the program in the foreground of the terminal —
// the leader of the process group there —, when it is not the shell.
func (s *Session) Foreground() (Program, bool) {
	group := s.foregroundGroup()
	if group <= 0 || group == s.pid {
		return Program{}, false
	}
	return Program{PID: group, Name: processName(group), Args: processArgs(group)}, true
}

// Resize gives the terminal a size in cells, and in pixels when known.
func (s *Session) Resize(cols, rows, width, height int) error {
	if cols <= 0 || rows <= 0 || cols > 1000 || rows > 500 {
		return errors.New("a terminal of an impossible size")
	}
	// The terminal's size is set under the lock, while the shell has not
	// ended: its terminal is closed only after it has (wait).
	s.mu.Lock()
	defer s.mu.Unlock()
	s.cols, s.rows = cols, rows
	s.resized++
	resized := s.resized
	if s.exited {
		return nil
	}
	size := &pty.Winsize{Cols: uint16(cols), Rows: uint16(rows), X: uint16(clamp(width)), Y: uint16(clamp(height))}
	if err := pty.Setsize(s.master, size); err != nil {
		return err
	}
	// bash, told of a new size while it finishes a command, may put the
	// size it knew back: the size is set again if it is not the one given.
	time.AfterFunc(resizeCheck, func() {
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.resized != resized || s.exited {
			return
		}
		if rows, cols, err := pty.Getsize(s.master); err == nil && (rows != int(size.Rows) || cols != int(size.Cols)) {
			_ = pty.Setsize(s.master, size)
		}
	})
	return nil
}

// Redraw has the program in the foreground draw its screen anew: it is told
// the size changed, and back.
func (s *Session) Redraw() {
	// Each size is set under the lock, while the shell has not ended: its
	// terminal is closed only after it has (wait).
	resize := func(narrower int) bool {
		s.mu.Lock()
		defer s.mu.Unlock()
		if s.exited || s.cols < 2 {
			return false
		}
		_ = pty.Setsize(s.master, &pty.Winsize{Cols: uint16(s.cols - narrower), Rows: uint16(s.rows)})
		return true
	}
	if resize(1) {
		time.Sleep(20 * time.Millisecond)
		resize(0)
	}
}

// FullScreen reports whether a program holds the alternate screen.
func (s *Session) FullScreen() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.scan.modes[1049] || s.scan.modes[1047] || s.scan.modes[47]
}

func clamp(px int) int { return max(0, min(px, 65535)) }

// hangUp ends the shell as closing a terminal would: its processes are hung
// up on, and killed if they do not end.
func (s *Session) hangUp() {
	hangUpGroup(s.pid)
	_ = s.proc.Signal(syscall.SIGHUP)
	go func() {
		select {
		case <-s.ended:
		case <-time.After(killGrace):
			killGroup(s.pid)
			_ = s.proc.Kill()
		}
	}()
}

// pump reads the output, keeps it and sends it to the pages attached.
func (s *Session) pump() {
	defer close(s.read)
	buf := make([]byte, 64<<10)
	for {
		n, err := s.master.Read(buf)
		if n > 0 {
			chunk := slices.Clone(buf[:n])
			s.mu.Lock()
			s.ring.write(chunk)
			changed := s.scan.feed(chunk)
			marks := s.scan.takeMarks()
			var meta *Meta
			if changed {
				meta = &Meta{Title: s.scan.title, Dir: s.scan.dir}
			}
			for c := range s.clients {
				c.send(Message{Output: chunk})
				if meta != nil {
					c.send(Message{Meta: meta})
				}
			}
			for _, observe := range s.observers {
				observe(Event{Output: chunk, Marks: marks, Meta: meta})
			}
			s.mu.Unlock()
		}
		if err != nil {
			return
		}
	}
}

// wait waits for the shell to end, then tells the pages attached, once its
// last output reached them.
func (s *Session) wait() {
	code := 0
	state, err := s.proc.Wait()
	if err == nil && state != nil {
		code = state.ExitCode()
		if code < 0 {
			code = 128 + signalOf(state)
		}
	}
	// The output a shell printed as it ended is read before it is said to
	// have ended, unless something it started holds the terminal open.
	select {
	case <-s.read:
	case <-time.After(500 * time.Millisecond):
	}
	s.mu.Lock()
	s.exited, s.code = true, code
	for c := range s.clients {
		c.send(Message{Exit: &code})
	}
	for _, observe := range s.observers {
		observe(Event{Exit: &code})
	}
	s.mu.Unlock()
	close(s.ended)
	go func() {
		<-s.read
		s.master.Close()
	}()
	time.AfterFunc(exitedFor, func() { s.manager.remove(s) })
}

func signalOf(state *os.ProcessState) int {
	if status, ok := state.Sys().(syscall.WaitStatus); ok && status.Signaled() {
		return int(status.Signal())
	}
	return 0
}

// ---------------------------------------------------------------- pages

// Meta is what the shell said of itself: its title, where it is.
type Meta struct {
	Title string `json:"title"`
	Dir   string `json:"cwd"`
}

// Message is what a page attached is sent: output, news of the title or
// the directory, or the shell's end.
type Message struct {
	Output []byte
	Meta   *Meta
	Exit   *int
}

// Client is a page attached to a session.
type Client struct {
	s    *Session
	out  chan Message
	gone chan struct{}
	once sync.Once
}

// Attachment is what a page that attaches is given: the output kept, as
// output that sets the terminal modes it needs and then draws the screen,
// and the session as it is.
type Attachment struct {
	Replay []byte
	Info   Info
	// FullScreen is set when a program holds the alternate screen, which
	// may need drawing anew.
	FullScreen bool
}

// Attach adds a page, which gets the output from here on from Messages.
func (s *Session) Attach() (*Client, Attachment) {
	c := &Client{s: s, out: make(chan Message, clientQueue), gone: make(chan struct{})}
	s.mu.Lock()
	kept := s.ring.bytes()
	// Drawing starts at a line: the first bytes kept may be the end of a
	// sequence.
	start := s.base.clone()
	if s.ring.len() == ringSize {
		if at := indexNewline(kept, 4096); at >= 0 {
			start.feed(kept[:at+1])
			kept = kept[at+1:]
		}
	}
	replay := append(start.prefix(), kept...)
	full := s.scan.modes[1049] || s.scan.modes[1047] || s.scan.modes[47]
	exited, code := s.exited, s.code
	if exited {
		c.out <- Message{Exit: &code}
	} else {
		s.clients[c] = struct{}{}
	}
	s.mu.Unlock()
	return c, Attachment{Replay: replay, Info: s.Info(), FullScreen: full}
}

func indexNewline(p []byte, within int) int {
	for i := 0; i < len(p) && i < within; i++ {
		if p[i] == '\n' {
			return i
		}
	}
	return -1
}

// Messages brings what the page is sent.
func (c *Client) Messages() <-chan Message { return c.out }

// Gone is closed when the page is let go: it fell behind, or detached.
func (c *Client) Gone() <-chan struct{} { return c.gone }

// send queues a message; the session's lock is held.
func (c *Client) send(m Message) {
	select {
	case c.out <- m:
	default:
		// Too far behind: the page attaches again and draws from the output
		// kept.
		delete(c.s.clients, c)
		c.once.Do(func() { close(c.gone) })
	}
}

// Detach takes the page away.
func (c *Client) Detach() {
	c.s.mu.Lock()
	delete(c.s.clients, c)
	c.s.mu.Unlock()
	c.once.Do(func() { close(c.gone) })
}
