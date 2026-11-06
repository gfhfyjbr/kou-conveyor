//go:build unix

package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func TestProgramStatusOutranksHooksWithTerminalTitlesDisabled(t *testing.T) {
	t.Setenv("SHELL", "/bin/bash")
	t.Setenv("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1")
	host := newHost(t, shellFiles)
	script := filepath.Join(t.TempDir(), "reporting-agent")
	const program = `#!/bin/sh
printf '\033]7501;state=idle:app=claude-code\007'
while IFS= read -r line; do
  case "$line" in
    working) printf '\033]7501;state=working:app=claude-code\007';;
    blocked) printf '\033]7501;state=blocked:app=claude-code:kind=permission:msg=QXBwbHk/\007';;
    done) printf '\033]7501;state=done:app=claude-code\007';;
    error) printf '\033]7501;state=error:app=claude-code:msg=RmFpbGVk\007';;
  esac
done
`
	if err := os.WriteFile(script, []byte(program), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "agents", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "reporting", Title: "Reporting Agent", Command: []string{script}, Launch: "type",
			Status: []string{"hooks", "idle"}, Output: "hooks", IdleMS: 1,
		}}}}, Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "OSC 7501")
	node := te.ops(t, id, `[{"op":"node.add","node":{"preset":"agents/reporting","title":"Reporter"}}]`).IDs["0"]
	eventually(t, 10*time.Second, "the first explicit idle report", func() bool {
		st := c.status(node)
		return st.State == StateIdle && st.Program != nil && st.Program.State == "idle"
	})
	c.mu.Lock()
	session := c.states[node].terminal
	c.mu.Unlock()
	token := te.token(t, c, node, ScopeBuild)
	for _, tc := range []struct{ input, state, report, hook string }{
		{"working", StateBusy, "working", "idle"},
		{"blocked", StateWaiting, "blocked", "busy"},
		{"done", StateIdle, "done", "waiting"},
		{"error", StateError, "error", "idle"},
	} {
		if err := session.Write([]byte(tc.input + "\n")); err != nil {
			t.Fatal(err)
		}
		eventually(t, 5*time.Second, tc.report+" report", func() bool {
			st := c.status(node)
			return st.State == tc.state && st.Program != nil && st.Program.State == tc.report
		})
		te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"`+tc.hook+`"}`, token, nil)
		if st := c.status(node); st.State != tc.state || st.Program == nil || st.Program.State != tc.report {
			t.Fatalf("hook %s replaced explicit %s: %#v", tc.hook, tc.report, st)
		}
	}
	// Hook answer transport remains intact; it is only the hook's status
	// estimate that loses precedence to the program's own report.
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"idle","text":"hook answer"}`, token, nil)
	var read struct {
		Text   string `json:"text"`
		Status Status `json:"status"`
	}
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/self/read?what=answer", "", token, &read)
	if read.Text != "hook answer" || read.Status.State != StateError || read.Status.Program == nil || read.Status.Program.State != "error" {
		t.Fatalf("hook output or reported error was lost: %#v", read)
	}
}
