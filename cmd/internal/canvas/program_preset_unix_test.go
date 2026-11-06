//go:build unix

package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/terminal"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func TestProgramStatusOldBashNonreporterKeepsManualActivityAndExit(t *testing.T) {
	t.Setenv("SHELL", oldBash(t))
	host := newHost(t, shellFiles)
	script := filepath.Join(t.TempDir(), "owned-preset")
	const program = `#!/bin/sh
printf 'OWNED_PRESET_READY\n'
while IFS= read -r line; do
  case "$line" in
    'manual activity')
      i=0
      while [ "$i" -lt 35 ]; do
        printf 'manual work %s\n' "$i"
        i=$((i+1))
        sleep 0.1
      done
      printf 'manual-result\n'
      ;;
    'exit without answer') exit 7 ;;
  esac
done
`
	if err := os.WriteFile(script, []byte(program), 0o755); err != nil {
		t.Fatal(err)
	}
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "agents", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "owned", Title: "Owned Preset", Command: []string{script}, Launch: "type",
			Status: []string{"idle"}, IdleMS: 300, Output: "screen", Input: &plugin.CanvasInput{Paste: "type"},
		}}}}, Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Old Bash preset fallback")
	res := te.ops(t, id, `[{"op":"node.add","node":{"preset":"agents/owned","title":"Owned"}},{"op":"node.add","node":{"kind":"agent","title":"Lead"}}]`)
	node, lead := res.IDs["0"], res.IDs["1"]
	defer func() {
		if t.Failed() {
			t.Logf("status: %#v; terminal:\n%s", c.status(node), tail(c, node))
		}
	}()
	eventually(t, 10*time.Second, "no-C preset idle", func() bool {
		return c.status(node).State == StateIdle && strings.Contains(tail(c, node), "OWNED_PRESET_READY")
	})
	s, err := host.terminals.Get(c.node(node).Runtime.Terminal)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Paste("manual activity", terminal.PasteOptions{Submit: "\r"}); err != nil {
		t.Fatal(err)
	}
	eventually(t, 10*time.Second, "manual sustained output makes the no-C preset busy", func() bool {
		return c.status(node).State == StateBusy
	})
	eventually(t, 10*time.Second, "manual turn falls idle", func() bool {
		return c.status(node).State == StateIdle && strings.Contains(tail(c, node), "manual-result")
	})
	if st := c.status(node); st.Program != nil || s.ProgramState().Reported {
		t.Fatal("fixture must remain a nonreporter throughout fallback")
	}
	asked := te.sendAsync(node, `{"text":"exit without answer","wait":true}`, te.token(t, c, lead, ScopeTalk))
	got := receive(t, asked)
	if got.code != http.StatusOK || got.Answered || !strings.Contains(got.Reason, "Owned Preset exited (7)") || got.Status.State != StateExited {
		t.Fatalf("no-C preset exit lost its delivered cause/status: %+v; terminal:\n%s", got, tail(c, node))
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[node]
	if st.cause != nil || c.readyLocked(c.doc.node(node), st, DeliverQueue) || c.readyLocked(c.doc.node(node), st, DeliverNow) {
		t.Fatal("no-C preset exit left a cause or admitted a prompt into the shell")
	}
}

func TestProgramStatusNonreporterInputStartsANewActivityBurst(t *testing.T) {
	m := terminal.NewManager(nil, "test")
	defer m.Close()
	s, err := m.Start(terminal.Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/cat", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Write([]byte("x")); err != nil {
		t.Fatal(err)
	}
	input := s.LastInput()
	ti, _ := programTestInstance()
	ti.session, ti.harnessUp, ti.inLine = s, false, true
	ti.launching.Store(true)
	ti.def.Harness.Status = []string{"idle"}
	ti.activeSince, ti.lastActive = input.Add(-time.Hour), input.Add(-time.Second)
	ti.c.states[ti.id].status = Status{State: StateIdle, Since: input.Add(-100 * time.Millisecond)}
	ti.noteOutput(input.Add(echoWindow / 2))
	if !ti.activeSince.Equal(input.Add(-time.Hour)) || !ti.lastActive.Equal(input.Add(-time.Second)) {
		t.Fatal("input echo counted as new activity")
	}
	at := input.Add(echoWindow + time.Millisecond)
	ti.noteOutput(at)
	if !ti.activeSince.Equal(at) {
		t.Fatal("manual input within activityGap failed to start a new output burst")
	}
	ti.noteOutput(at.Add(sustained))
	ti.followHarness(at.Add(sustained), ti.c.status(ti.id))
	if st := ti.c.status(ti.id); st.State != StateBusy {
		t.Fatalf("new sustained manual output did not resume the owned preset: %#v", st)
	}
}
