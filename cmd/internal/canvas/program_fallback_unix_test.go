//go:build unix

package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func TestProgramStatusNonreporterKeepsHooksWithTerminalTitlesDisabled(t *testing.T) {
	t.Setenv("SHELL", "/bin/bash")
	t.Setenv("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1")
	host := newHost(t, shellFiles)
	script := filepath.Join(t.TempDir(), "nonreporting-agent")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nprintf 'NONREPORTER_READY\\n'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "agents", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "nonreporter", Title: "Nonreporter", Command: []string{script}, Launch: "type", Status: []string{"hooks"}, Output: "hooks",
		}}}}, Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Nonreporter fallback")
	node := te.ops(t, id, `[{"op":"node.add","node":{"preset":"agents/nonreporter","title":"Nonreporter"}}]`).IDs["0"]
	eventually(t, 10*time.Second, "nonreporting program started", func() bool {
		c.mu.Lock()
		session := c.states[node].terminal
		c.mu.Unlock()
		return session != nil && strings.Contains(session.Tail(10), "NONREPORTER_READY")
	})
	token := te.token(t, c, node, ScopeBuild)
	for _, state := range []string{"idle", "busy", "waiting"} {
		te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"`+state+`"}`, token, nil)
		if st := c.status(node); st.State != state || st.Program != nil {
			t.Fatalf("title-disabled nonreporter lost hook %s fallback: %#v", state, st)
		}
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"idle","text":"fallback answer"}`, token, nil)
	var read struct {
		Text   string `json:"text"`
		Status Status `json:"status"`
	}
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/self/read?what=answer", "", token, &read)
	if read.Text != "fallback answer" || read.Status.State != StateIdle || read.Status.Program != nil {
		t.Fatalf("nonreporter hook answer/status fallback was lost: %#v", read)
	}
}
