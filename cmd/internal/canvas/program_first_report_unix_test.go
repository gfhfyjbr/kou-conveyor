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

func TestProgramStatusFirstDoneAfterPromptReturnsScreenAnswer(t *testing.T) {
	t.Setenv("SHELL", "/bin/bash")
	t.Setenv("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1")
	host := newHost(t, shellFiles)
	script := filepath.Join(t.TempDir(), "late-reporter")
	const program = `#!/bin/sh
printf 'LATE_REPORTER_READY\n'
while IFS= read -r line; do
  if [ "$line" = 'hello first done' ]; then
    printf 'screen-result-7501\n\033]7501;state=done:app=claude-code\007'
  fi
done
`
	if err := os.WriteFile(script, []byte(program), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "agents", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "late-reporter", Title: "Late Reporter", Command: []string{script}, Launch: "type", Status: []string{"hooks"}, Output: "screen",
			Input: &plugin.CanvasInput{Paste: "type"},
		}}}}, Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "First done")
	res := te.ops(t, id, `[{"op":"node.add","node":{"preset":"agents/late-reporter","title":"Late Reporter"}},{"op":"node.add","node":{"kind":"agent","title":"Lead"}}]`)
	node, lead := res.IDs["0"], res.IDs["1"]
	eventually(t, 10*time.Second, "late reporter is waiting without any OSC status", func() bool {
		return strings.Contains(tail(c, node), "LATE_REPORTER_READY")
	})
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"idle"}`, te.token(t, c, node, ScopeBuild), nil)
	asked := te.sendAsync(node, `{"text":"hello first done","wait":true}`, te.token(t, c, lead, ScopeTalk))
	got := receive(t, asked)
	if got.code != http.StatusOK || !got.Answered || !strings.Contains(got.Answer, "screen-result-7501") || got.Status.State != StateIdle || got.Status.Program == nil || got.Status.Program.State != "done" {
		t.Fatalf("first done did not answer the delivered prompt: %+v; terminal:\n%s", got, tail(c, node))
	}
}
