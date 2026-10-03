package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/canvas"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// The canvas (cmd/internal/canvas) runs in the server: its engine is given
// the server as its host — the workspaces, the shells, the plugins, the
// sessions' queues — and its routes join the server's. With -canvas=off
// there is no engine: its routes are not found, its hooks in the runs do
// nothing, and the pages show no canvas.

// canvasHost is the server as the canvas's engine sees it.
type canvasHost struct{ s *server }

func canvasWorkspace(ws *workspace) canvas.Workspace {
	return canvas.Workspace{ID: ws.ID, Path: ws.Path, SessionDir: ws.opt.SessionDir}
}

func (h canvasHost) Workspaces() []canvas.Workspace {
	list := h.s.workspaces.all()
	out := make([]canvas.Workspace, 0, len(list))
	for _, ws := range list {
		out = append(out, canvasWorkspace(ws))
	}
	return out
}

func (h canvasHost) Workspace(id string) (canvas.Workspace, bool) {
	if id == "" {
		return canvasWorkspace(h.s.workspaces.startup()), true
	}
	if ws := h.s.workspaces.find(id); ws != nil {
		return canvasWorkspace(ws), true
	}
	return canvas.Workspace{}, false
}

func (h canvasHost) Terminals() *terminal.Manager { return h.s.terminals }

func (h canvasHost) Plugins(ws canvas.Workspace) plugin.Found {
	if w := h.s.workspaces.find(ws.ID); w != nil {
		return h.s.plugins(w)
	}
	return plugin.Found{}
}

func (h canvasHost) Enqueue(ctx context.Context, ws canvas.Workspace, session, id, text, model string, force bool) error {
	w := h.s.workspaces.find(ws.ID)
	if w == nil {
		return errors.New("the workspace is gone")
	}
	_, err := h.s.enqueue(ctx, w, session, id, text, model, nil, force)
	return err
}

func (h canvasHost) StopRun(ws canvas.Workspace, session string) bool {
	w := h.s.workspaces.find(ws.ID)
	if w == nil {
		return false
	}
	h.s.mu.Lock()
	current := h.s.attachable(w, session)
	h.s.mu.Unlock()
	if current == nil {
		return false
	}
	current.stop()
	return true
}

func (h canvasHost) RunActive(ws canvas.Workspace, session string) bool {
	w := h.s.workspaces.find(ws.ID)
	if w == nil {
		return false
	}
	h.s.mu.Lock()
	defer h.s.mu.Unlock()
	return h.s.active[activeKey(w, session)] != nil
}

// canvasRunFinished tells the canvas that a run ended, with what answer,
// and to which prompts: what the node of its agent answers, and to whom. A
// run stopped or failed answers nothing, nor does a compaction.
func (s *server) canvasRunFinished(current *run, tr *cockpit.Transcript, outcome string) {
	if s.canvas == nil {
		return
	}
	answer := ""
	if !current.compact && outcome == "done" {
		answer = canvas.FinalAnswer(tr.Entries)
	}
	s.canvas.RunFinished(current.ws.ID, current.sessionID, answer, outcome, runPrompts(current, tr))
}

// runPrompts are the IDs of the prompts a run answered: the one it started
// with, and those handed to it while it ran, which come after that one in
// the transcript.
func runPrompts(current *run, tr *cockpit.Transcript) []string {
	if current.message == "" {
		return nil
	}
	ids := []string{current.message}
	first := slices.IndexFunc(tr.Entries, func(e *cockpit.Entry) bool { return e.ID == "input:"+current.message })
	if first < 0 {
		return ids
	}
	for _, e := range tr.Entries[first+1:] {
		if e.Kind != cockpit.KindUser || e.State == cockpit.Pending || e.State == cockpit.Undelivered {
			continue
		}
		if id, ok := strings.CutPrefix(e.ID, "input:"); ok {
			ids = append(ids, id)
		}
	}
	return ids
}

// startCanvas starts the canvases' engine, once the terminals the build
// before handed over are taken up: the shells of the canvases' nodes among
// them.
func (s *server) startCanvas(addr net.Addr) {
	if !s.opt.canvas {
		return
	}
	config := os.TempDir()
	if s.opt.SettingsFile != "" {
		config = filepath.Dir(s.opt.SettingsFile)
	}
	cache, err := os.UserCacheDir()
	if err != nil {
		cache = os.TempDir()
	}
	cache = filepath.Join(cache, "kou-conveyor")
	binDir := filepath.Join(cache, "canvas-bin")
	cli, err := linkCanvasCLI(binDir)
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: canvas: kou-canvas is not on the nodes' PATH:", err)
		binDir, cli = "", ""
	}
	engine, err := canvas.New(s.ctx, canvas.Options{
		Host: canvasHost{s}, URL: "http://" + displayAddress(addr),
		SecretFile: filepath.Join(config, "canvas.secret"),
		BinDir:     binDir, CLI: cli,
		LaunchDir: filepath.Join(cache, "canvas"),
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: canvas:", err)
		return
	}
	s.canvas = engine
	engine.Start()
}

// linkCanvasCLI puts kou-canvas where the programs of the canvases' nodes
// find it: a link to kou-conveyor-canvas beside the server, or else to the
// server itself, which is kou-canvas too when called by that name.
func linkCanvasCLI(dir string) (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	if resolved, err := filepath.EvalSymlinks(exe); err == nil {
		exe = resolved
	}
	target := filepath.Join(filepath.Dir(exe), "kou-conveyor-canvas")
	if info, err := os.Stat(target); err != nil || info.IsDir() {
		target = exe
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	link := filepath.Join(dir, "kou-canvas")
	if current, err := os.Readlink(link); err == nil && current == target {
		return link, nil
	}
	temporary := link + ".new"
	_ = os.Remove(temporary)
	if err := os.Symlink(target, temporary); err != nil {
		return "", err
	}
	if err := os.Rename(temporary, link); err != nil {
		_ = os.Remove(temporary)
		return "", err
	}
	return link, nil
}
