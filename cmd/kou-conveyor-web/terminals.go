package main

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/gorilla/websocket"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
)

// Terminals are shells in pseudo-terminals (cmd/internal/terminal), for the
// terminal tabs of the browser cockpit's sidebar. A page starts one in a
// workspace and attaches to it over a WebSocket:
//
//	page → server   binary frames: what is typed, as it is typed
//	                text frames:   {"type":"resize","cols","rows","width","height"}
//	server → page   {"type":"hello","terminal":{…},"replay":n}, then n bytes
//	                of output to draw the screen from, {"type":"live"}, then
//	                binary frames of output as it comes;
//	                {"type":"meta","title","cwd"} when the shell says where it is;
//	                {"type":"exit","code"} when it ends, and the socket closes.
//
// A shell runs on while no page shows it — the page reattaches, from its
// tab, after a reload — until its tab is closed or it ends, and outlives the
// server's own restarts with a new build (rebuild.go). Only pages of the
// cockpit's own origin attach: the upgrade checks Origin against Host, and
// Host is one of the server's names (server.go).

// terminalsEnvironment hands the shells over to a new build of the server.
const terminalsEnvironment = "KOU_CONVEYOR_WEB_TERMINALS"

// startTerminals sets the terminals up, taking up those a build before this
// one handed over.
func (s *server) startTerminals() {
	s.terminals = terminal.NewManager(s.shellFiles, buildLabel())
	path := os.Getenv(terminalsEnvironment)
	if path == "" {
		return
	}
	os.Unsetenv(terminalsEnvironment)
	n, err := s.terminals.Adopt(path)
	if n > 0 {
		fmt.Printf("terminals         %d taken up from the build before\n", n)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: terminals:", err)
	}
}

// shellFiles are the shell integration files: the shell/ directory of the
// built-in terminal plugin, read anew for every shell, so that changes to
// a checkout's apply to the next.
func (s *server) shellFiles() (fs.FS, error) {
	builtins, _ := s.assets.builtins()
	for _, current := range builtins {
		if current.Name == "terminal" && current.Files != nil {
			return fs.Sub(current.Files, "shell")
		}
	}
	return nil, errors.New("the terminal plugin has no shell files")
}

// terminalTheme says whether shells start with kou-conveyor's theme.
func (s *server) terminalTheme() bool {
	return cockpit.LoadPreferences(cockpit.PreferencesPath(s.opt.SettingsFile)).TerminalTheme != cockpit.TerminalThemeShell
}

func (s *server) handleTerminals(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"terminals": s.terminals.List(ws.ID)})
}

// handleStartTerminal starts a shell: {"cols", "rows"} its size, in the
// workspace, or where terminal "from" is (a split), or in "cwd".
func (s *server) handleStartTerminal(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	var req struct {
		Cols int    `json:"cols"`
		Rows int    `json:"rows"`
		Dir  string `json:"cwd"`
		From string `json:"from"`
	}
	if !decodeJSON(w, r, &req) {
		return
	}
	dir := ws.Path
	switch {
	case req.From != "":
		if from, err := s.terminals.Get(req.From); err == nil {
			if at := from.Dir(); at != "" {
				dir = at
			}
		}
	case req.Dir != "":
		at := req.Dir
		if !filepath.IsAbs(at) {
			at = filepath.Join(ws.Path, at)
		}
		if info, err := os.Stat(at); err != nil || !info.IsDir() {
			writeError(w, http.StatusBadRequest, "no such folder: "+req.Dir)
			return
		}
		dir = filepath.Clean(at)
	}
	if info, err := os.Stat(dir); err != nil || !info.IsDir() {
		writeError(w, http.StatusConflict, "the workspace's folder is missing: "+display(ws.Path))
		return
	}
	session, err := s.terminals.Start(terminal.Spec{Workspace: ws.ID, Dir: dir, Theme: s.terminalTheme(), Cols: req.Cols, Rows: req.Rows})
	switch {
	case errors.Is(err, terminal.ErrTooMany):
		writeError(w, http.StatusTooManyRequests, "too many terminals: close some first")
		return
	case errors.Is(err, terminal.ErrClosed):
		writeError(w, http.StatusServiceUnavailable, err.Error())
		return
	case err != nil:
		writeError(w, http.StatusInternalServerError, "cannot start a shell: "+err.Error())
		return
	}
	writeJSON(w, http.StatusCreated, session.Info())
}

func (s *server) handleTerminal(w http.ResponseWriter, r *http.Request) {
	session, err := s.terminals.Get(r.PathValue("id"))
	if err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, session.Info())
}

// handleKillTerminal ends a shell, as closing its tab does: at once, or
// after ?after= seconds, while its tab may come back to it (keep).
func (s *server) handleKillTerminal(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	after := 0
	if raw := r.URL.Query().Get("after"); raw != "" {
		if _, err := fmt.Sscan(raw, &after); err != nil || after < 0 {
			writeError(w, http.StatusBadRequest, "after must be seconds")
			return
		}
	}
	var err error
	if after > 0 {
		err = s.terminals.CloseAfter(id, time.Duration(after)*time.Second)
	} else {
		err = s.terminals.Kill(id)
	}
	if err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	if after > 0 {
		session, err := s.terminals.Get(id)
		if err == nil {
			writeJSON(w, http.StatusOK, session.Info())
			return
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"killed": true})
}

// handleKeepTerminal keeps a shell a closed tab would end: the tab came back.
func (s *server) handleKeepTerminal(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if err := s.terminals.Keep(id); err != nil {
		writeError(w, http.StatusNotFound, "the shell ended")
		return
	}
	s.handleTerminal(w, r)
}

// terminalUpgrader takes the pages of the cockpit's own origin only: its
// check of Origin against Host is the default.
var terminalUpgrader = websocket.Upgrader{ReadBufferSize: 16 << 10, WriteBufferSize: 64 << 10}

const (
	// terminalPing is how often a page is pinged, and terminalIdle how long
	// one that says nothing, pongs included, is kept.
	terminalPing = 30 * time.Second
	terminalIdle = 90 * time.Second
	// terminalFrame bounds the output sent in one frame.
	terminalFrame = 256 << 10
)

// handleTerminalSocket attaches a page to a shell.
func (s *server) handleTerminalSocket(w http.ResponseWriter, r *http.Request) {
	session, err := s.terminals.Get(r.PathValue("id"))
	if err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	conn, err := terminalUpgrader.Upgrade(w, r, nil)
	if err != nil {
		return // the upgrader answered
	}
	defer conn.Close()
	conn.SetReadLimit(1 << 20)
	client, attachment := session.Attach()
	written := make(chan struct{})
	go writeTerminal(conn, client, attachment, written)
	alive := func() error { return conn.SetReadDeadline(time.Now().Add(terminalIdle)) }
	_ = alive()
	conn.SetPongHandler(func(string) error { return alive() })
	resized := false
	for {
		kind, data, err := conn.ReadMessage()
		if err != nil {
			break
		}
		_ = alive()
		switch kind {
		case websocket.BinaryMessage:
			_ = session.Write(data)
		case websocket.TextMessage:
			var message struct {
				Type   string `json:"type"`
				Data   string `json:"data"`
				Cols   int    `json:"cols"`
				Rows   int    `json:"rows"`
				Width  int    `json:"width"`
				Height int    `json:"height"`
			}
			if json.Unmarshal(data, &message) != nil {
				continue
			}
			switch message.Type {
			case "input":
				_ = session.Write([]byte(message.Data))
			case "resize":
				if session.Resize(message.Cols, message.Rows, message.Width, message.Height) == nil && !resized {
					resized = true
					// A program on the alternate screen draws it anew for
					// the page that attached.
					if attachment.FullScreen && len(attachment.Replay) > 0 {
						go session.Redraw()
					}
				}
			}
		}
	}
	client.Detach()
	<-written
}

// writeTerminal sends a page what it attached to, then the shell's output
// and news as they come, until the page goes or the shell ends.
func writeTerminal(conn *websocket.Conn, client *terminal.Client, attachment terminal.Attachment, done chan<- struct{}) {
	defer close(done)
	write := func(kind int, data []byte) bool {
		_ = conn.SetWriteDeadline(time.Now().Add(15 * time.Second))
		if err := conn.WriteMessage(kind, data); err != nil {
			conn.Close()
			return false
		}
		return true
	}
	send := func(v any) bool {
		data, err := json.Marshal(v)
		return err == nil && write(websocket.TextMessage, data)
	}
	if !send(map[string]any{"type": "hello", "terminal": attachment.Info, "replay": len(attachment.Replay)}) {
		return
	}
	if len(attachment.Replay) > 0 && !write(websocket.BinaryMessage, attachment.Replay) {
		return
	}
	if !send(map[string]any{"type": "live"}) {
		return
	}
	ping := time.NewTicker(terminalPing)
	defer ping.Stop()
	var pending *terminal.Message
	for {
		var m terminal.Message
		if pending != nil {
			m, pending = *pending, nil
		} else {
			select {
			case m = <-client.Messages():
			case <-client.Gone():
				// Detached, or too far behind: a page that fell behind
				// attaches again and draws from the output kept.
				conn.Close()
				return
			case <-ping.C:
				if conn.WriteControl(websocket.PingMessage, nil, time.Now().Add(10*time.Second)) != nil {
					conn.Close()
					return
				}
				continue
			}
		}
		switch {
		case m.Output != nil:
			// What else waits goes in the same frame.
			buf := append([]byte(nil), m.Output...)
		gather:
			for len(buf) < terminalFrame {
				select {
				case next := <-client.Messages():
					if next.Output == nil {
						pending = &next
						break gather
					}
					buf = append(buf, next.Output...)
				default:
					break gather
				}
			}
			if !write(websocket.BinaryMessage, buf) {
				return
			}
		case m.Meta != nil:
			if !send(map[string]any{"type": "meta", "title": m.Meta.Title, "cwd": m.Meta.Dir}) {
				return
			}
		case m.Exit != nil:
			send(map[string]any{"type": "exit", "code": *m.Exit})
			_ = conn.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, "the shell ended"), time.Now().Add(time.Second))
			conn.Close()
			return
		}
	}
}

// handleTerminalSettings says how terminals start shells.
func (s *server) handleTerminalSettings(w http.ResponseWriter, r *http.Request) {
	theme := cockpit.TerminalThemeKou
	if !s.terminalTheme() {
		theme = cockpit.TerminalThemeShell
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"theme": theme, "shell": terminal.UserShell(), "integration": terminal.Integrated(terminal.UserShell()),
		"saved": s.opt.SettingsFile != "",
	})
}

// handleSaveTerminalSettings records {"theme": "kou" or "shell"}; the
// shells started from then on follow it.
func (s *server) handleSaveTerminalSettings(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Theme string `json:"theme"`
	}
	if !decodeJSON(w, r, &req) {
		return
	}
	if req.Theme != cockpit.TerminalThemeKou && req.Theme != cockpit.TerminalThemeShell {
		writeError(w, http.StatusBadRequest, `theme must be "kou" or "shell"`)
		return
	}
	path := cockpit.PreferencesPath(s.opt.SettingsFile)
	if path == "" {
		writeError(w, http.StatusConflict, "preferences are unavailable: set KOU_CONVEYOR_CONFIG")
		return
	}
	s.settingsMu.Lock()
	err := cockpit.SaveTerminalTheme(path, req.Theme)
	s.settingsMu.Unlock()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.handleTerminalSettings(w, r)
}
