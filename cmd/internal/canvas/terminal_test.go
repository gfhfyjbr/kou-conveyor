//go:build unix

package canvas

import (
	"encoding/json/jsontext"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// shellFiles are the terminal plugin's shell integration files.
func shellFiles() (fs.FS, error) {
	return os.DirFS(filepath.Join("..", "..", "kou-conveyor-web", "plugins", "terminal", "shell")), nil
}

// integratedShell is a shell whose integration marks its commands and
// their ends: zsh, or bash 4.4 or later, whose PS0 marks where a command
// starts.
func integratedShell(t *testing.T) string {
	t.Helper()
	if path, err := exec.LookPath("zsh"); err == nil {
		return path
	}
	if path, err := exec.LookPath("bash"); err == nil {
		out, err := exec.Command(path, "-c", `echo "${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"`).Output()
		var major, minor int
		if _, scanErr := fmt.Sscan(string(out), &major, &minor); err == nil && scanErr == nil && (major > 4 || major == 4 && minor >= 4) {
			return path
		}
	}
	t.Skip("no zsh, nor bash 4.4 or later")
	return ""
}

// tail is the end of what a node's shell shows.
func tail(c *Canvas, node string) string {
	text, _ := c.read(node, "tail", 40)
	return text
}

func TestShellNodesRunTakeTextAndEnd(t *testing.T) {
	t.Setenv("SHELL", "/bin/sh")
	host := newHost(t, nil)
	secret := filepath.Join(t.TempDir(), "canvas.secret")
	te := startEngine(t, host, secret)
	id, c := te.newCanvas(t, "Shells")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"shell","title":"Box"}},
		{"op":"node.add","node":{"kind":"agent","title":"Listener"}},
		{"op":"edge.add","edge":{"from":{"node":"$0","port":"exit"},"to":{"node":"$1"}}}
	]`)
	box, listener := res.IDs["0"], res.IDs["1"]
	session := sessionOf(t, c, listener)
	eventually(t, 10*time.Second, "the shell idle", func() bool { return c.status(box).State == StateIdle })
	term := c.node(box).Runtime.Terminal
	if term == "" || c.node(box).Access != ScopeTalk {
		t.Fatalf("node %+v", c.node(box))
	}
	// The node's shell is not the sidebar's.
	if list := host.terminals.List("w1"); len(list) != 0 {
		t.Fatalf("the sidebar lists %+v", list)
	}
	if owned := host.terminals.ListOwned(ownerPrefix); len(owned) != 1 || owned[0].ID != term || owned[0].Owner != ownerOf("w1", id, box) {
		t.Fatalf("owned %+v", owned)
	}

	// Text sent goes in as typed, with the canvas's variables there.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"text":"echo canvas-$((40+2)) $KOU_CANVAS_NODE ${KOU_CANVAS_TOKEN%%.*}"}`, "", nil)
	eventually(t, 10*time.Second, "the command's output", func() bool { return strings.Contains(tail(c, box), "canvas-42 "+box+" kc1") })
	eventually(t, 10*time.Second, "the shell idle again", func() bool { return c.status(box).State == StateIdle })

	// The server restarts: the node takes its shell up again.
	te.Close()
	next := startEngine(t, host, secret)
	c = next.canvas(t, id)
	eventually(t, 10*time.Second, "the shell bound again", func() bool { return c.status(box).State == StateIdle })
	if again := c.node(box).Runtime.Terminal; again != term {
		t.Fatalf("a new shell: %s, was %s", again, term)
	}
	next.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"text":"echo again-$((1+1))"}`, "", nil)
	eventually(t, 10*time.Second, "the output after the restart", func() bool { return strings.Contains(tail(c, box), "again-2") })
	eventually(t, 10*time.Second, "the shell idle again", func() bool { return c.status(box).State == StateIdle })

	// The shell ends, and the node says how on its exit port.
	next.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"text":"exit 3"}`, "", nil)
	eventually(t, 10*time.Second, "the node exited", func() bool { return c.status(box).State == StateExited })
	if detail := c.status(box).Detail; detail != "exit 3" {
		t.Fatalf("exited: %q", detail)
	}
	eventually(t, 5*time.Second, "the exit delivered", func() bool { return len(host.promptsOf(session)) == 1 })
	if text := host.promptsOf(session)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Box» ("+box+"):\nexit 3") {
		t.Fatalf("the listener was told %q", text)
	}

	// Restarted, it has a shell again.
	next.must(t, http.StatusAccepted, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/restart", "", "", nil)
	eventually(t, 10*time.Second, "a new shell", func() bool {
		return c.status(box).State == StateIdle && c.node(box).Runtime.Terminal != term
	})
}

func TestARemovedNodesShellLivesOnAWhile(t *testing.T) {
	t.Setenv("SHELL", "/bin/sh")
	host := newHost(t, nil)
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Undo")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"shell","title":"Keep"}},
		{"op":"node.add","node":{"preset":"note","title":"Beside"}}
	]`)
	box := res.IDs["0"]
	eventually(t, 10*time.Second, "the shell idle", func() bool { return c.status(box).State == StateIdle })
	n := c.node(box)
	s, err := host.terminals.Get(n.Runtime.Terminal)
	if err != nil {
		t.Fatal(err)
	}

	// Undone within its grace, the node comes back to the same shell, where
	// it was.
	te.ops(t, id, `[{"op":"node.remove","id":"`+box+`","grace_s":1}]`)
	te.ops(t, id, `[{"op":"node.add","node":{"id":"`+box+`","restore":true}}]`)
	time.Sleep(1500 * time.Millisecond)
	if exited, _ := s.Exited(); exited {
		t.Fatal("the shell of a node brought back ended")
	}
	if back := c.node(box); back.Runtime.Terminal != n.Runtime.Terminal || back.X != n.X || back.Y != n.Y || c.status(box).State != StateIdle {
		t.Fatalf("back as %+v, %s", back, c.status(box).State)
	}

	// Removed for good, its shell ends after the grace.
	te.ops(t, id, `[{"op":"node.remove","id":"`+box+`","grace_s":1}]`)
	eventually(t, 10*time.Second, "the shell ended", func() bool { exited, _ := s.Exited(); return exited })
	if status, _ := te.call(t, "POST", "/api/w/w1/canvases/"+id+"/ops", `{"ops":[{"op":"node.add","node":{"id":"`+box+`","restore":true}}]}`, ""); status != http.StatusBadRequest {
		t.Fatalf("a node gone for good came back: %d", status)
	}
}

func TestShellsOfCanvasesDeletedEndOrGoToTheSidebar(t *testing.T) {
	t.Setenv("SHELL", "/bin/sh")
	host := newHost(t, nil)
	secret := filepath.Join(t.TempDir(), "canvas.secret")
	te := startEngine(t, host, secret)
	gone, c1 := te.newCanvas(t, "Gone")
	kept, c2 := te.newCanvas(t, "Kept")
	one := te.ops(t, gone, `[{"op":"node.add","node":{"preset":"shell"}}]`).IDs["0"]
	two := te.ops(t, kept, `[{"op":"node.add","node":{"preset":"shell"}}]`).IDs["0"]
	eventually(t, 10*time.Second, "the shells idle", func() bool {
		return c1.status(one).State == StateIdle && c2.status(two).State == StateIdle
	})
	first, _ := host.terminals.Get(c1.node(one).Runtime.Terminal)
	second := c2.node(two).Runtime.Terminal

	// A canvas deleted with its terminals kept gives them to the sidebar.
	te.must(t, http.StatusNoContent, "DELETE", "/api/w/w1/canvases/"+kept+"?keep_terminals=1", "", "", nil)
	if list := host.terminals.List("w1"); len(list) != 1 || list[0].ID != second {
		t.Fatalf("the sidebar lists %+v", list)
	}

	// The shells of a canvas that is gone when the server starts end.
	te.Close()
	if err := os.Remove(docPath(host.ws.Path, gone)); err != nil {
		t.Fatal(err)
	}
	startEngine(t, host, secret)
	eventually(t, 10*time.Second, "the shell of a canvas gone ended", func() bool { exited, _ := first.Exited(); return exited })
}

func TestOneShellsOutputIsTheNextOnesInput(t *testing.T) {
	t.Setenv("SHELL", integratedShell(t))
	host := newHost(t, shellFiles)
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Pipeline")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"shell","title":"A"}},
		{"op":"node.add","node":{"preset":"shell","title":"B"}},
		{"op":"node.add","node":{"preset":"command","title":"Say","config":{"command":"echo from-the-command"}}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"},"template":"echo \"got: {{text}}\""}}
	]`)
	a, b, say := res.IDs["0"], res.IDs["1"], res.IDs["2"]
	output := func(node string) string {
		text, _ := c.read(node, "output", 0)
		return text
	}

	// The command typed at its shell's first prompt ran, and what it
	// printed is the node's output.
	eventually(t, 20*time.Second, "the command's output", func() bool { return output(say) == "from-the-command" })
	eventually(t, 20*time.Second, "the shells idle", func() bool {
		return c.status(a).State == StateIdle && c.status(b).State == StateIdle && c.status(say).State == StateIdle
	})

	// What a command prints in A is B's next command.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+a+"/send", `{"text":"echo hi"}`, "", nil)
	eventually(t, 20*time.Second, "B's output", func() bool { return output(b) == "got: hi" })
	if got := output(a); got != "hi" {
		t.Fatalf("A's output: %q", got)
	}
	var along *Message
	for _, m := range messagesIn(c, MessageDelivered) {
		if m.Edge != "" {
			along = &m
		}
	}
	if along == nil || along.Text != `echo "got: hi"` || string(along.Data) != `{"command":"echo hi","exit_code":0}` || along.Title != "echo hi" || along.Hops != 1 {
		t.Fatalf("the message along the edge: %+v", along)
	}
	eventually(t, 20*time.Second, "the shells idle again", func() bool {
		return c.status(a).State == StateIdle && c.status(b).State == StateIdle
	})

	// A command that fails says its code.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+say+"/send", `{"text":"(exit 7)"}`, "", nil)
	failed := func() bool {
		for _, event := range published(c, "output") {
			if event["node"] == say && event["title"] == "(exit 7)" {
				return true
			}
		}
		return false
	}
	for deadline := time.Now().Add(20 * time.Second); !failed(); time.Sleep(20 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatalf("no output of the failure: outputs %v, status %+v, the shell shows:\n%s", published(c, "output"), c.status(say), tail(c, say))
		}
	}
}

// oldBash is a bash before 4.4, which has no PS0 to mark where a command
// starts: macOS's /bin/bash.
func oldBash(t *testing.T) string {
	t.Helper()
	out, err := exec.Command("/bin/bash", "-c", `echo "${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"`).Output()
	var major, minor int
	if _, scanErr := fmt.Sscan(string(out), &major, &minor); err == nil && scanErr == nil && (major < 4 || major == 4 && minor < 4) {
		return "/bin/bash"
	}
	t.Skip("no bash before 4.4")
	return ""
}

func TestCommandsOfAnOldBashHaveOutputsToo(t *testing.T) {
	t.Setenv("SHELL", oldBash(t))
	host := newHost(t, shellFiles)
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Old bash")
	a := te.ops(t, id, `[{"op":"node.add","node":{"preset":"shell","title":"A"}}]`).IDs["0"]
	eventually(t, 20*time.Second, "the shell idle", func() bool { return c.status(a).State == StateIdle })
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+a+"/send", `{"text":"echo hi; echo there"}`, "", nil)
	output := func() string {
		text, _ := c.read(a, "output", 0)
		return text
	}
	for deadline := time.Now().Add(20 * time.Second); output() != "hi\nthere"; time.Sleep(20 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatalf("output %q, status %+v, the shell shows:\n%s", output(), c.status(a), tail(c, a))
		}
	}
}

// A command that shows nothing for a while — a server that serves, a
// program that waits for its input — runs on quietly: still at work, but
// not counted busy, until it ends.
func TestACommandThatShowsNothingRunsOnQuietly(t *testing.T) {
	t.Setenv("SHELL", integratedShell(t))
	host := newHost(t, shellFiles)
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Quiet")
	box := te.ops(t, id, `[{"op":"node.add","node":{"preset":"shell","title":"Server"}}]`).IDs["0"]
	eventually(t, 20*time.Second, "the shell idle", func() bool { return c.status(box).State == StateIdle })

	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"text":"echo serving; sleep 60"}`, "", nil)
	eventually(t, 10*time.Second, "the command at work", func() bool { return c.status(box).State == StateBusy })
	if st, s := c.status(box), c.summary(); st.Quiet || s.Busy != 1 {
		t.Fatalf("at work: status %+v, %d busy", st, s.Busy)
	}
	eventually(t, 15*time.Second, "the command quiet", func() bool { return c.status(box).Quiet })
	if st, s := c.status(box), c.summary(); st.State != StateBusy || s.Busy != 0 {
		t.Fatalf("quiet: status %+v, %d busy", st, s.Busy)
	}

	// It ends: the shell is idle, and no longer quiet.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"keys":["C-c"]}`, "", nil)
	eventually(t, 10*time.Second, "the shell idle again", func() bool {
		st := c.status(box)
		return st.State == StateIdle && !st.Quiet
	})
}

func TestAHarnessWithoutHooksAnswersWithItsScreen(t *testing.T) {
	t.Setenv("SHELL", "/bin/sh")
	host := newHost(t, nil)
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "tuis", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "echo", Title: "Echo", Command: []string{"cat"}, Launch: "type",
			Env:    map[string]string{"ECHO_NODE": "{{node.id}}"},
			Status: []string{"idle"}, IdleMS: 300, Output: "screen",
		}}}},
		Active: true,
	}}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Screens")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"tuis/echo","title":"Echo"}},
		{"op":"node.add","node":{"kind":"agent","title":"Reader"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	echo, reader := res.IDs["0"], res.IDs["1"]

	// The line typed at the first prompt sets the preset's variables; the
	// shell has the canvas's, and no token shows.
	eventually(t, 20*time.Second, "the program typed", func() bool { return strings.Contains(tail(c, echo), "env ECHO_NODE="+echo+" cat") })
	if screen := tail(c, echo); strings.Contains(screen, "KOU_CANVAS") {
		t.Fatalf("the canvas's variables on the line:\n%s", screen)
	}
	eventually(t, 20*time.Second, "it waits", func() bool { return c.status(echo).State == StateIdle })

	// Given a message by a node, it is busy until it falls quiet; then
	// what its screen shows is its answer, which goes out — to the node
	// that asked along the edge, and no more.
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+echo+"/send", `{"text":"hello screen"}`, te.token(t, c, reader, ScopeTalk), nil)
	session := sessionOf(t, c, reader)
	eventually(t, 20*time.Second, "its screen delivered", func() bool {
		prompts := te.host.promptsOf(session)
		return len(prompts) == 1 && strings.Count(prompts[0].text, "hello screen") == 2
	})
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(session); len(prompts) != 1 || !strings.Contains(prompts[0].text, "[canvas] from «Echo» ("+echo+"):\n") {
		t.Fatalf("the reader was given %+v", prompts)
	}
}

// fakeClaude is an agent's program as the shell runs it: it says hello,
// then answers each line it reads, after it works on it a while.
const fakeClaude = `#!/bin/sh
echo "Fake Claude Code 1.0"
while IFS= read -r line; do
	i=0
	while [ $i -lt 6 ]; do printf .; sleep 0.2; i=$((i+1)); done
	echo
	echo "answer to $line"
done
`

func TestAnAgentRunInAShellIsFoundAndAnswers(t *testing.T) {
	t.Setenv("SHELL", integratedShell(t))
	host := newHost(t, shellFiles)
	if err := os.WriteFile(filepath.Join(host.ws.Path, "claude"), []byte(fakeClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	te := startEngine(t, host, filepath.Join(t.TempDir(), "canvas.secret"))
	id, c := te.newCanvas(t, "Found")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"shell","title":"Box"}},
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}}
	]`)
	box, lead := res.IDs["0"], res.IDs["1"]
	eventually(t, 20*time.Second, "the shell idle", func() bool { return c.status(box).State == StateIdle })

	// The user runs the agent in the shell: the node is the agent's, and
	// waits for its prompt.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"text":"./claude"}`, "", nil)
	eventually(t, 20*time.Second, "the agent found", func() bool {
		st := c.status(box)
		return st.Agent != nil && st.State == StateIdle
	})
	if agent := c.status(box).Agent; agent.ID != "claude" || agent.Title != "Claude Code" || !agent.Detected {
		t.Fatalf("found %+v", agent)
	}
	// The program in the foreground of its terminal is it.
	eventually(t, 10*time.Second, "the agent's process", func() bool {
		agent := c.status(box).Agent
		return agent != nil && agent.PID != 0
	})

	// A node asks it, and waits: once it falls quiet, what it shows is the
	// answer.
	asked := te.sendAsync(box, `{"text":"hello agent","wait":true}`, te.token(t, c, lead, ScopeTalk))
	got := receive(t, asked)
	if got.code != http.StatusOK || !got.Answered || !strings.Contains(got.Answer, "answer to hello agent") || got.Status.State != StateIdle {
		t.Fatalf("the send answered %+v; the shell shows:\n%s", got, tail(c, box))
	}
	if !strings.Contains(tail(c, box), "answer to [canvas] from «Lead» ("+lead+"):") {
		t.Fatalf("the agent was not told who asked; the shell shows:\n%s", tail(c, box))
	}

	// It ends: the shell is a shell again.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+box+"/send", `{"keys":["C-d"]}`, "", nil)
	eventually(t, 20*time.Second, "the agent gone", func() bool {
		st := c.status(box)
		return st.Agent == nil && st.State == StateIdle
	})
}

func TestAHarnessIsLaunchedAsItsPresetSays(t *testing.T) {
	t.Setenv("SHELL", "/bin/sh")
	host := newHost(t, nil)
	host.plugins = []plugin.Plugin{{
		Manifest: plugin.Manifest{Name: "agents", Canvas: &plugin.Canvas{Harnesses: []plugin.CanvasHarness{{
			ID: "fake-code", Title: "Fake Code", Command: []string{"fake-code", "--verbose"},
			SessionArg: []string{"--session-id", "{{runtime.agent_session}}"},
			Resume:     []string{"--resume", "{{runtime.agent_session}}"},
			Args: []string{
				"--append-system-prompt", "{{brief}}", "--settings", "{{files.settings}}",
				"--model", "{{config.model}}", "--mode", "{{config.mode}}",
			},
			Files: map[string]jsontext.Value{
				"settings": jsontext.Value(`{"hooks":{"Stop":"kou-canvas hook claude"}}`),
				"notes":    jsontext.Value(`"node {{node.title}}"`),
			},
			Env:    map[string]string{"FAKE_NODE": "{{node.id}}"},
			Status: []string{"hooks"}, Output: "hooks",
			Config: map[string]any{"type": "object", "properties": map[string]any{"mode": map[string]any{"type": "string", "default": "plan"}}},
		}}}},
		Active: true,
	}}
	secret := filepath.Join(t.TempDir(), "canvas.secret")
	te := startEngine(t, host, secret)
	id, c := te.newCanvas(t, "Harness")
	coder := te.ops(t, id, `[{"op":"node.add","node":{"preset":"agents/fake-code","title":"Coder"}}]`).IDs["0"]
	if n := c.node(coder); n.Kind != KindTerminal || n.Plugin != "agents" || n.Preset != "fake-code" || n.Access != ScopeBuild || n.W != 760 || n.H != 520 {
		t.Fatalf("node %+v", n)
	}
	token := te.token(t, c, coder, ScopeBuild)

	var launch Launch
	te.must(t, http.StatusOK, "GET", "/api/canvas/self/launch", "", token, &launch)
	session := c.node(coder).Runtime.AgentSession
	dir := filepath.Join(filepath.Dir(secret), "launch", "w1", id, coder)
	brief := c.brief(coder)
	want := []string{
		"fake-code", "--verbose", "--session-id", session,
		"--append-system-prompt", brief, "--settings", filepath.Join(dir, "settings.json"), "--mode", "plan",
	}
	if session == "" || !slices.Equal(launch.Argv, want) || launch.Dir != host.ws.Path || launch.Title != "Fake Code" {
		t.Fatalf("launch %q in %s, want %q", launch.Argv, launch.Dir, want)
	}
	if !strings.HasPrefix(brief, "You are node «Coder» ("+coder+", Fake Code) on the kou-conveyor canvas «Harness».") {
		t.Fatalf("brief %q", brief)
	}
	if !slices.Contains(launch.Env, "FAKE_NODE="+coder) || !slices.ContainsFunc(launch.Env, func(entry string) bool { return strings.HasPrefix(entry, "KOU_CANVAS_TOKEN=kc1.") }) {
		t.Fatalf("env %q", launch.Env)
	}
	if notes, err := os.ReadFile(filepath.Join(dir, "notes.txt")); err != nil || string(notes) != "node Coder" {
		t.Fatalf("notes %q, %v", notes, err)
	}
	if settings, err := os.ReadFile(filepath.Join(dir, "settings.json")); err != nil || !strings.Contains(string(settings), `"Stop": "kou-canvas hook claude"`) {
		t.Fatalf("settings %s, %v", settings, err)
	}

	// Resumed, it takes its session up again.
	te.must(t, http.StatusOK, "GET", "/api/canvas/self/launch?resume=1", "", token, &launch)
	if !slices.Equal(launch.Argv[:4], []string{"fake-code", "--verbose", "--resume", session}) {
		t.Fatalf("resumed with %q", launch.Argv)
	}

	// Its hooks say what it does, and what it answered.
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"busy"}`, token, nil)
	if state := c.status(coder).State; state != StateBusy {
		t.Fatalf("busy, it is %s", state)
	}
	te.must(t, http.StatusOK, "POST", "/api/canvas/emit", `{"status":"idle","text":"done coding","agent_session":"abc-123"}`, token, nil)
	var read struct {
		Text   string `json:"text"`
		Status Status `json:"status"`
	}
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/self/read?what=answer", "", token, &read)
	if read.Text != "done coding" || read.Status.State != StateIdle || c.node(coder).Runtime.AgentSession != "abc-123" {
		t.Fatalf("read %+v, session %q", read, c.node(coder).Runtime.AgentSession)
	}
	if status, _ := te.call(t, "POST", "/api/canvas/emit", `{"status":"sleeping"}`, token); status != http.StatusBadRequest {
		t.Fatalf("a status of no kind: %d", status)
	}
}
