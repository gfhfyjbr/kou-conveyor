package canvascli

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// recorded is a request the fake server got.
type recorded struct {
	method, path, query string
	auth                string
	body                map[string]any
}

// fakeCanvas is a server that answers as the canvas's does, and records
// what it was asked.
type fakeCanvas struct {
	mu       sync.Mutex
	requests []recorded
	server   *httptest.Server
	self     string // what /api/canvas/self answers, when not a kou agent's
}

func newFakeCanvas(t *testing.T) *fakeCanvas {
	f := &fakeCanvas{}
	f.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		req := recorded{method: r.Method, path: r.URL.Path, query: r.URL.RawQuery, auth: r.Header.Get("Authorization")}
		if len(data) != 0 {
			if err := json.Unmarshal(data, &req.body); err != nil {
				t.Errorf("%s %s: body %q: %v", r.Method, r.URL.Path, data, err)
			}
		}
		f.mu.Lock()
		f.requests = append(f.requests, req)
		self := f.self
		f.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.URL.Path == "/api/canvas/view":
			io.WriteString(w, `{"canvas":{"id":"c1","title":"Triage","live":true,"rev":3},"scope":"build",
				"me":{"id":"n_me","kind":"agent","title":"Triage","status":"busy","x":0,"y":0,"w":440,"h":560,"inputs":["in"],"outputs":["out"]},
				"nodes":[{"id":"n_me","kind":"agent","title":"Triage","status":"busy","x":0,"y":0,"w":440,"h":560,"inputs":["in"],"outputs":["out"]},
				{"id":"n_new","kind":"terminal","preset":"codex","title":"worker","status":"starting","x":480,"y":0,"w":760,"h":520,"inputs":["in"],"outputs":["out"],"branch":"kou/fix-a","created_by":"n_me"}],
				"edges":[{"id":"e_1","from":"n_new:out","to":"n_me:in","mode":"auto","deliver":"queue"}],
				"free":[{"side":"below","x":0,"y":600}]}`)
		case r.URL.Path == "/api/canvas/ops" && strings.Contains(string(data), `"node.remove"`):
			w.WriteHeader(http.StatusForbidden)
			io.WriteString(w, `{"error":"your access is talk; this needs build"}`)
		case r.URL.Path == "/api/canvas/ops":
			io.WriteString(w, `{"rev":4,"ids":{"0":"n_new"},"placed":{"n_new":{"x":480,"y":0,"w":760,"h":520}}}`)
		case strings.HasSuffix(r.URL.Path, "/read"):
			io.WriteString(w, `{"node":"n_new","what":"output","text":"--- FAIL: TestX\nexit status 1","status":{"state":"exited","detail":"exit 1"}}`)
		case strings.HasSuffix(r.URL.Path, "/send") && req.body["wait"] == true:
			io.WriteString(w, `{"message":{"id":"m_1","state":"delivered"},"node":"n_new","outputs":2,"answered":true,"answer":"pong\n","status":{"state":"idle"}}`)
		case strings.HasSuffix(r.URL.Path, "/send"):
			io.WriteString(w, `{"message":{"id":"m_1","state":"pending"},"node":"n_new","outputs":2}`)
		case r.URL.Path == "/api/canvas/wait":
			io.WriteString(w, `{"fired":"n_new","until":"answer","next":"n_new:3,n_b:0","nodes":[
				{"node":"n_new","title":"worker","status":{"state":"idle"},"outputs":3},
				{"node":"n_b","title":"tests","status":{"state":"busy","activity":"go test"},"outputs":0}]}`)
		case r.URL.Path == "/api/canvas/emit":
			io.WriteString(w, `{"ok":true}`)
		case r.URL.Path == "/api/canvas/self" && self != "":
			io.WriteString(w, self)
		case r.URL.Path == "/api/canvas/self":
			io.WriteString(w, `{"canvas":{"id":"c1","title":"Triage","live":true},"node":{"id":"n_me","kind":"agent","title":"Triage","x":0,"y":0,"w":440,"h":560},"scope":"build","brief":"You are node «Triage».","workspace":"/w","page":"http://x/#/w/1/c/c1"}`)
		default:
			w.WriteHeader(http.StatusForbidden)
			io.WriteString(w, `{"error":"your access is talk; this needs build"}`)
		}
	}))
	t.Cleanup(f.server.Close)
	return f
}

func (f *fakeCanvas) take() []recorded {
	f.mu.Lock()
	defer f.mu.Unlock()
	requests := f.requests
	f.requests = nil
	return requests
}

// added is the node the requests since the last take added.
func (f *fakeCanvas) added(t *testing.T) map[string]any {
	t.Helper()
	requests := f.take()
	for _, r := range requests {
		if r.method == "POST" && r.path == "/api/canvas/ops" {
			return r.body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
		}
	}
	t.Fatalf("no node added: %+v", requests)
	return nil
}

// run runs kou-canvas against the fake server, as node n_me.
func (f *fakeCanvas) run(t *testing.T, stdin string, args ...string) (int, string, string) {
	t.Helper()
	var out, errOut bytes.Buffer
	env := map[string]string{envURL: f.server.URL, envToken: "kc1.test", envNode: "n_me", envID: "c1"}
	a := &app{in: strings.NewReader(stdin), out: &out, err: &errOut, getenv: func(name string) string { return env[name] }}
	code := a.run(context.Background(), args)
	return code, out.String(), errOut.String()
}

func TestPluginManifestIsInSync(t *testing.T) {
	want, err := PluginManifest()
	if err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(filepath.Join("..", "..", "..", "harness", "plugin", "builtin", "canvas-agent", "plugin.json"))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatal("harness/plugin/builtin/canvas-agent/plugin.json is not kou-canvas's tools: run go generate ./cmd/internal/canvascli")
	}
}

func TestToolSchemasAreObjects(t *testing.T) {
	for _, tool := range Tools() {
		var schema map[string]any
		if err := json.Unmarshal([]byte(tool.Schema), &schema); err != nil || schema["type"] != "object" {
			t.Errorf("%s: schema %v, %v", tool.Name, schema, err)
		}
		if properties, _ := schema["properties"].(map[string]any); len(properties) == 0 {
			t.Errorf("%s has no properties", tool.Name)
		}
	}
}

func TestSpawnFromTheCommandLine(t *testing.T) {
	f := newFakeCanvas(t)
	code, out, errOut := f.run(t, "", "spawn", "codex", "--title", "worker", "--worktree", "fix-a", "--include", "vendor, tools/oracle", "--right-of", "self", "--prompt", "Fix it", "--connect-to", "self")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, errOut)
	}
	if !strings.Contains(out, "created n_new «worker» (terminal/codex) at (480,0 760×520)") || !strings.Contains(out, "connected n_new:out → n_me:in (e_1)") {
		t.Fatalf("output:\n%s", out)
	}
	requests := f.take()
	if len(requests) != 2 || requests[0].method != "POST" || requests[0].path != "/api/canvas/ops" || requests[0].auth != "Bearer kc1.test" {
		t.Fatalf("requests = %+v", requests)
	}
	op := requests[0].body["ops"].([]any)[0].(map[string]any)
	node := op["node"].(map[string]any)
	config := node["config"].(map[string]any)
	if op["op"] != "node.add" || node["preset"] != "codex" || node["title"] != "worker" || node["near"] != "self" || node["side"] != "right" ||
		node["prompt"] != "Fix it" || config["worktree"].(map[string]any)["name"] != "fix-a" || node["connect"].(map[string]any)["to"] != "self" ||
		fmt.Sprint(config["worktree"].(map[string]any)["include"]) != "[vendor tools/oracle]" || config["worktree"].(map[string]any)["link"] != nil {
		t.Fatalf("node = %v", node)
	}
}

// TestSpawnAKouAgentFromTheCommandLine: with no preset, kou-canvas spawn
// makes a kou agent, and --worktree takes a name or a preset after it.
func TestSpawnAKouAgentFromTheCommandLine(t *testing.T) {
	f := newFakeCanvas(t)
	for _, test := range []struct {
		args            []string
		kind, preset    string
		sandbox, wtName any
	}{
		{[]string{"spawn", "--title", "w", "--worktree", "fix-a"}, "agent", "", "worktree", nil},
		{[]string{"spawn", "--worktree", "shell", "--title", "s"}, "", "shell", nil, ""},
		{[]string{"spawn", "--title", "w", "--config", `{"effort":"high"}`}, "agent", "", nil, nil},
	} {
		if code, _, errOut := f.run(t, "", test.args...); code != 0 {
			t.Fatalf("%v: exit %d: %s", test.args, code, errOut)
		}
		node := f.added(t)
		kind, _ := node["kind"].(string)
		preset, _ := node["preset"].(string)
		config, _ := node["config"].(map[string]any)
		if kind != test.kind || preset != test.preset || config["sandbox"] != test.sandbox {
			t.Errorf("%v: node %v", test.args, node)
		}
		if test.wtName != nil {
			if spec, _ := config["worktree"].(map[string]any); spec == nil || spec["name"] != nil {
				t.Errorf("%v: worktree %v, want one named after the node", test.args, config["worktree"])
			}
		}
	}
}

func TestSpawnAnAgentInAWorktree(t *testing.T) {
	f := newFakeCanvas(t)
	tool, _ := toolNamed("CanvasSpawn")
	c := &client{base: f.server.URL, token: "kc1.test", node: "n_me", http: f.server.Client()}
	if _, err := tool.call(context.Background(), c, []byte(`{"preset":"agent","title":"reviewer","worktree":"x","x":100.0,"y":-40}`)); err != nil {
		t.Fatal(err)
	}
	node := f.take()[0].body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
	if node["kind"] != "agent" || node["preset"] != nil || node["config"].(map[string]any)["sandbox"] != "worktree" || node["x"] != 100.0 || node["y"] != -40.0 ||
		node["config"].(map[string]any)["worktree"] != nil {
		t.Fatalf("node = %v", node)
	}
	// What its worktree gets beside git's files: a list, or one string.
	if _, err := tool.call(context.Background(), c, []byte(`{"preset":"agent","title":"builder","include":["vendor/"],"link":"fonts,tools/oracle"}`)); err != nil {
		t.Fatal(err)
	}
	node = f.take()[0].body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
	config := node["config"].(map[string]any)
	spec, _ := config["worktree"].(map[string]any)
	if config["sandbox"] != "worktree" || fmt.Sprint(spec["include"]) != "[vendor/]" || fmt.Sprint(spec["link"]) != "[fonts tools/oracle]" || spec["name"] != nil {
		t.Fatalf("node = %v", node)
	}
	// No preset: a kou agent; no preset but a command: a terminal running it.
	if _, err := tool.call(context.Background(), c, []byte(`{"title":"x","config":{"effort":"high"}}`)); err != nil {
		t.Fatal(err)
	}
	node = f.take()[0].body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
	if node["kind"] != "agent" || node["preset"] != nil || node["config"].(map[string]any)["effort"] != "high" {
		t.Fatalf("a node of no preset: %v", node)
	}
	if _, err := tool.call(context.Background(), c, []byte(`{"title":"x","command":"make test"}`)); err != nil {
		t.Fatal(err)
	}
	node = f.take()[0].body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
	if node["kind"] != nil || node["preset"] != "command" || node["config"].(map[string]any)["command"] != "make test" {
		t.Fatalf("a node of a command: %v", node)
	}
	if _, err := tool.call(context.Background(), c, []byte(`[1]`)); err == nil {
		t.Fatal("arguments that are not an object were taken")
	}
}

// TestSpawnWiresAnAgentToItsSpawner: an agent an agent spawns answers it —
// its output is wired to the spawner's input — unless connect says
// otherwise; nothing else is wired by itself, nor what a shell spawns.
func TestSpawnWiresAnAgentToItsSpawner(t *testing.T) {
	f := newFakeCanvas(t)
	tool, _ := toolNamed("CanvasSpawn")
	c := &client{base: f.server.URL, token: "kc1.test", node: "n_me", http: f.server.Client()}
	for _, test := range []struct {
		args, connect string
	}{
		{`{"title":"coder"}`, `map[to:self]`},
		{`{"preset":"agent","title":"coder","prompt":"go"}`, `map[to:self]`},
		{`{"preset":"codex","title":"codex"}`, `map[to:self]`},
		{`{"preset":"claude-code","title":"claude"}`, `map[to:self]`},
		{`{"preset":"foreman","title":"lead"}`, `map[to:self]`},
		{`{"title":"coder","connect":{}}`, `<nil>`},
		{`{"title":"coder","connect":{"from":"self"}}`, `map[from:self]`},
		{`{"title":"coder","connect":{"to":"n_b:in"}}`, `map[to:n_b:in]`},
		{`{"preset":"shell","title":"sh"}`, `<nil>`},
		{`{"title":"tests","command":"go test ./..."}`, `<nil>`},
		{`{"preset":"note","prompt":"plan"}`, `<nil>`},
		{`{"preset":"acme/feed","title":"feed"}`, `<nil>`},
	} {
		if _, err := tool.call(context.Background(), c, []byte(test.args)); err != nil {
			t.Fatalf("%s: %v", test.args, err)
		}
		if node := f.added(t); fmt.Sprint(node["connect"]) != test.connect {
			t.Errorf("%s: connect %v, want %s", test.args, node["connect"], test.connect)
		}
	}

	// From the command line, as a tool: for an agent's program.
	for _, test := range []struct {
		args    []string
		connect string
	}{
		{[]string{"spawn", "--title", "coder"}, `map[to:self]`},
		{[]string{"spawn", "codex", "--title", "codex"}, `map[to:self]`},
		{[]string{"spawn", "--title", "coder", "--no-connect"}, `<nil>`},
		{[]string{"spawn", "--title", "coder", "--connect-from", "self"}, `map[from:self]`},
		{[]string{"spawn", "shell", "--title", "sh"}, `<nil>`},
	} {
		if code, _, errOut := f.run(t, "", test.args...); code != 0 {
			t.Fatalf("%v: exit %d: %s", test.args, code, errOut)
		}
		if node := f.added(t); fmt.Sprint(node["connect"]) != test.connect {
			t.Errorf("%v: connect %v, want %s", test.args, node["connect"], test.connect)
		}
	}

	// A shell's command line spawns an agent unwired: the agent's answers
	// would be typed in as commands. A coding agent's terminal is wired.
	for _, test := range []struct {
		self, connect string
	}{
		{`{"node":{"id":"n_me","kind":"terminal","preset":"shell","title":"sh"}}`, `<nil>`},
		{`{"node":{"id":"n_me","kind":"terminal","preset":"command","title":"make"}}`, `<nil>`},
		{`{"node":{"id":"n_me","kind":"terminal","preset":"claude-code","title":"claude"}}`, `map[to:self]`},
	} {
		f.mu.Lock()
		f.self = test.self
		f.mu.Unlock()
		if code, _, errOut := f.run(t, "", "spawn", "--title", "coder"); code != 0 {
			t.Fatalf("%s: exit %d: %s", test.self, code, errOut)
		}
		if node := f.added(t); fmt.Sprint(node["connect"]) != test.connect {
			t.Errorf("as %s: connect %v, want %s", test.self, node["connect"], test.connect)
		}
	}
}

func TestViewPrintsTheCanvas(t *testing.T) {
	f := newFakeCanvas(t)
	code, out, _ := f.run(t, "", "view")
	if code != 0 {
		t.Fatal(code)
	}
	for _, want := range []string{
		"canvas «Triage» (live) · you: n_me «Triage» agent · access build",
		"n_new     ◎ worker",
		"⎇ kou/fix-a",
		"made by n_me",
		"e_1       n_new:out «worker» → n_me:in «Triage»",
		"free: below you at (0,600)",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("no %q in:\n%s", want, out)
		}
	}
	code, out, _ = f.run(t, "", "view", "--json")
	if code != 0 || !strings.HasPrefix(out, "{\n") || !jsontext.Value(out).IsValid() {
		t.Fatalf("--json: %d %q", code, out)
	}
}

func TestReadWaitsOnTheServer(t *testing.T) {
	f := newFakeCanvas(t)
	code, out, errOut := f.run(t, "", "read", "n_new", "--output", "--wait", "exit", "--timeout", "15m")
	if code != 0 {
		t.Fatal(errOut)
	}
	requests := f.take()
	if len(requests) != 1 || requests[0].path != "/api/canvas/nodes/n_new/read" || requests[0].query != "lines=80&timeout=900&wait=exit&what=output" {
		t.Fatalf("requests = %+v", requests)
	}
	if !strings.HasPrefix(out, "n_new · exited (exit 1) · 2 lines\n--- FAIL: TestX") {
		t.Fatalf("output:\n%s", out)
	}
}

func TestWaitForSeveralNodes(t *testing.T) {
	f := newFakeCanvas(t)
	tool, _ := toolNamed("CanvasWait")
	c := &client{base: f.server.URL, token: "kc1.test", http: f.server.Client()}
	for _, args := range []string{`{"nodes":["worker","tests"],"after":"n_new:2","timeout_s":"120"}`, `{"nodes":"worker, tests","after":"n_new:2","timeout_s":120}`} {
		res, err := tool.call(context.Background(), c, []byte(args))
		if err != nil {
			t.Fatal(err)
		}
		if requests := f.take(); len(requests) != 1 || requests[0].path != "/api/canvas/wait" || requests[0].query != "after=n_new%3A2&nodes=worker%2Ctests&timeout=120" {
			t.Fatalf("%s: requests = %+v", args, requests)
		}
		for _, want := range []string{"answer: n_new «worker».", "n_b «tests» · busy (go test) · 0 outputs", "next: n_new:3,n_b:0"} {
			if !strings.Contains(res.text, want) {
				t.Fatalf("no %q in:\n%s", want, res.text)
			}
		}
	}
	// The command line waits for the children when no node is named; for
	// one node as before.
	if code, _, errOut := f.run(t, "", "wait", "--until", "idle"); code != 0 {
		t.Fatal(errOut)
	}
	if requests := f.take(); len(requests) != 1 || requests[0].path != "/api/canvas/wait" || requests[0].query != "timeout=600&until=idle" {
		t.Fatalf("requests = %+v", requests)
	}
	f.run(t, "", "wait", "n_new", "--until", "output", "--after", "2")
	if requests := f.take(); len(requests) != 1 || requests[0].path != "/api/canvas/nodes/n_new/wait" {
		t.Fatalf("requests = %+v", requests)
	}
}

func TestSendAndKeys(t *testing.T) {
	f := newFakeCanvas(t)
	if code, out, _ := f.run(t, "", "send", "n_new", "go", "test", "--no-enter", "--now"); code != 0 || !strings.Contains(out, "outputs so far: 2") {
		t.Fatalf("send: %d %s", code, out)
	}
	if code, _, _ := f.run(t, "", "keys", "n_new", "C-c", "Enter"); code != 0 {
		t.Fatal("keys failed")
	}
	requests := f.take()
	if body := requests[0].body; body["text"] != "go test" || body["submit"] != false || body["when"] != "now" {
		t.Fatalf("send = %v", body)
	}
	if body := requests[1].body; body["text"] != "" || len(body["keys"].([]any)) != 2 || body["when"] != "now" {
		t.Fatalf("keys = %v", body)
	}
}

func TestSendWaitsForTheAnswer(t *testing.T) {
	f := newFakeCanvas(t)
	// The command line, by the node's title.
	code, out, errOut := f.run(t, "", "send", "worker", "ping?", "--wait", "--timeout", "2m")
	if code != 0 || out != "n_new answered:\npong\n" {
		t.Fatalf("send --wait: %d %q %s", code, out, errOut)
	}
	if body := f.take()[0].body; body["text"] != "ping?" || body["wait"] != true || body["timeout_s"] != 120.0 {
		t.Fatalf("send = %v", body)
	}
	// The tool, its wait written as models write it.
	tool, _ := toolNamed("CanvasSend")
	c := &client{base: f.server.URL, token: "kc1.test", node: "n_me", http: f.server.Client()}
	res, err := tool.call(context.Background(), c, []byte(`{"node":"worker","text":"ping?","wait":"true"}`))
	if err != nil || res.text != "n_new answered:\npong" {
		t.Fatalf("CanvasSend: %q, %v", res.text, err)
	}
	if body := f.take()[0].body; body["wait"] != true || body["timeout_s"] != 600.0 {
		t.Fatalf("CanvasSend = %v", body)
	}
	// Without wait, it says where the answer comes.
	res, err = tool.call(context.Background(), c, []byte(`{"node":"worker","text":"ping?","wait":false}`))
	if err != nil || !strings.Contains(res.text, "comes to you later as a reply") {
		t.Fatalf("CanvasSend without wait: %q, %v", res.text, err)
	}
	if body := f.take()[0].body; body["wait"] != nil {
		t.Fatalf("CanvasSend without wait = %v", body)
	}
}

// replace asks the server to put the text in place of the sender's last
// prompt; it takes text only.
func TestSendReplacesTheLastPrompt(t *testing.T) {
	f := newFakeCanvas(t)
	code, out, errOut := f.run(t, "", "send", "worker", "put", "otherwise", "--replace", "--wait")
	if code != 0 || !strings.HasPrefix(out, "your last prompt to n_new is replaced") || !strings.Contains(out, "n_new answered:\npong") {
		t.Fatalf("send --replace: %d %q %s", code, out, errOut)
	}
	if body := f.take()[0].body; body["text"] != "put otherwise" || body["replace"] != true || body["wait"] != true {
		t.Fatalf("send --replace = %v", body)
	}
	tool, _ := toolNamed("CanvasSend")
	c := &client{base: f.server.URL, token: "kc1.test", node: "n_me", http: f.server.Client()}
	if _, err := tool.call(context.Background(), c, []byte(`{"node":"worker","text":"again","replace":"true"}`)); err != nil {
		t.Fatalf("CanvasSend with replace: %v", err)
	}
	if body := f.take()[0].body; body["replace"] != true {
		t.Fatalf("CanvasSend with replace = %v", body)
	}
	if _, err := tool.call(context.Background(), c, []byte(`{"node":"worker","keys":["C-c"],"replace":true}`)); err == nil || !strings.Contains(err.Error(), "not keys") {
		t.Fatalf("CanvasSend with keys and replace: %v", err)
	}
	if res, err := tool.call(context.Background(), c, []byte(`{"node":"worker","text":"plain"}`)); err != nil || strings.Contains(res.text, "replaced") {
		t.Fatalf("CanvasSend without replace: %q, %v", res.text, err)
	}
	if requests := f.take(); len(requests) != 1 || requests[0].body["replace"] != nil {
		t.Fatalf("the requests after: %v", requests)
	}
}

func TestErrorsAndNoCanvas(t *testing.T) {
	f := newFakeCanvas(t)
	if code, _, errOut := f.run(t, "", "rm", "n_x", "--nope"); code != 2 || !strings.Contains(errOut, "no flag --nope") {
		t.Fatalf("bad flag: %d %s", code, errOut)
	}
	if code, out, _ := f.run(t, `{"node":"n_x"}`, "tool", "CanvasRemove"); code == 0 || !strings.Contains(out, "your access is talk") {
		t.Fatalf("tool error: %d %s", code, out)
	}
	var out, errOut bytes.Buffer
	a := &app{in: strings.NewReader(`{"hook_event_name":"Stop"}`), out: &out, err: &errOut, getenv: func(string) string { return "" }}
	if code := a.run(context.Background(), []string{"view"}); code != 1 || !strings.Contains(errOut.String(), "not on a canvas") {
		t.Fatalf("no token: %d %s", code, errOut.String())
	}
	if code := a.run(context.Background(), []string{"hook", "claude"}); code != 0 || out.Len() != 0 {
		t.Fatalf("hook without a token: %d %q", code, out.String())
	}
}

func TestEmitAndSelf(t *testing.T) {
	f := newFakeCanvas(t)
	if code, out, _ := f.run(t, "", "emit", "--port", "done", "all", "green", "--data", `{"n":1}`); code != 0 || out != "put out on done\n" {
		t.Fatalf("emit: %d %q", code, out)
	}
	if body := f.take()[0].body; body["text"] != "all green" || body["port"] != "done" || body["data"].(map[string]any)["n"] != 1.0 {
		t.Fatalf("emit = %v", body)
	}
	if code, out, _ := f.run(t, "", "self"); code != 0 || !strings.Contains(out, "you are n_me «Triage» (agent) on canvas «Triage» (live) · access build") || !strings.Contains(out, "You are node «Triage».") {
		t.Fatalf("self: %s", out)
	}
}

func TestMCPServesTheTools(t *testing.T) {
	f := newFakeCanvas(t)
	input := strings.Join([]string{
		`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}`,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		`{"jsonrpc":"2.0","id":2,"method":"tools/list"}`,
		`{"jsonrpc":"2.0","id":"three","method":"tools/call","params":{"name":"CanvasView","arguments":{}}}`,
		`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"CanvasRemove","arguments":{"node":"n_x"}}}`,
		`{"jsonrpc":"2.0","id":5,"method":"nope"}`,
	}, "\n") + "\n"
	code, out, _ := f.run(t, input, "mcp")
	if code != 0 {
		t.Fatal(code)
	}
	responses := map[string]map[string]any{}
	scanner := bufio.NewScanner(strings.NewReader(out))
	scanner.Buffer(nil, 1<<20)
	for scanner.Scan() {
		var response map[string]any
		if err := json.Unmarshal(scanner.Bytes(), &response); err != nil {
			t.Fatal(err)
		}
		id, _ := json.Marshal(response["id"])
		responses[string(id)] = response
	}
	if len(responses) != 5 {
		t.Fatalf("responses:\n%s", out)
	}
	if result := responses["1"]["result"].(map[string]any); result["protocolVersion"] != "2025-03-26" {
		t.Fatalf("initialize = %v", result)
	}
	if list := responses["2"]["result"].(map[string]any)["tools"].([]any); len(list) != len(Tools()) || list[0].(map[string]any)["inputSchema"] == nil {
		t.Fatalf("tools/list = %v", list)
	}
	view := responses[`"three"`]["result"].(map[string]any)
	if view["isError"] != false || !strings.Contains(view["content"].([]any)[0].(map[string]any)["text"].(string), "canvas «Triage»") {
		t.Fatalf("CanvasView = %v", view)
	}
	if remove := responses["4"]["result"].(map[string]any); remove["isError"] != true {
		t.Fatalf("CanvasRemove = %v", remove)
	}
	if responses["5"]["error"].(map[string]any)["code"] != -32601.0 {
		t.Fatalf("nope = %v", responses["5"])
	}
}

func TestClaudeHooks(t *testing.T) {
	dir := t.TempDir()
	transcript := filepath.Join(dir, "session.jsonl")
	lines := []string{
		`{"type":"user","message":{"role":"user","content":"Fix the bug"}}`,
		`{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Looking."}]}}`,
		`{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}`,
		`{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}`,
		`{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}`,
		`{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Tests pass."}]}}`,
		`{"type":"system","subtype":"stop_hook_summary"}`,
	}
	if err := os.WriteFile(transcript, []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	report := claudeReport([]byte(`{"session_id":"s1","transcript_path":"` + transcript + `","hook_event_name":"Stop"}`))
	if report["status"] != "idle" || report["text"] != "Fixed it.\n\nTests pass." || report["agent_session"] != "s1" {
		t.Fatalf("Stop = %v", report)
	}
	if report := claudeReport([]byte(`{"hook_event_name":"UserPromptSubmit","prompt":"x"}`)); report["status"] != "busy" {
		t.Fatalf("UserPromptSubmit = %v", report)
	}
	if report := claudeReport([]byte(`{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash"}`)); report["status"] != "waiting" {
		t.Fatalf("Notification = %v", report)
	}
	if report := claudeReport([]byte(`{"hook_event_name":"Notification","message":"Claude is waiting for your input"}`)); report != nil {
		t.Fatalf("idle Notification = %v", report)
	}
	if report := claudeReport([]byte(`{"hook_event_name":"SessionStart","session_id":"s2"}`)); report["agent_session"] != "s2" {
		t.Fatalf("SessionStart = %v", report)
	}

	f := newFakeCanvas(t)
	if code, out, _ := f.run(t, `{"hook_event_name":"UserPromptSubmit"}`, "hook", "claude"); code != 0 || out != "" {
		t.Fatalf("hook: %d %q", code, out)
	}
	if requests := f.take(); len(requests) != 1 || requests[0].path != "/api/canvas/emit" || requests[0].body["status"] != "busy" {
		t.Fatalf("requests = %+v", requests)
	}
}

func TestCodexNotify(t *testing.T) {
	f := newFakeCanvas(t)
	event := `{"type":"agent-turn-complete","thread-id":"th1","turn-id":"1","input-messages":["x"],"last-assistant-message":"Done: 3 files."}`
	if code, _, _ := f.run(t, "", "hook", "codex", event); code != 0 {
		t.Fatal(code)
	}
	if body := f.take()[0].body; body["text"] != "Done: 3 files." || body["status"] != "idle" || body["answer"] != true || body["agent_session"] != "th1" {
		t.Fatalf("emit = %v", body)
	}
	if report := codexReport([]byte(`{"type":"something-else"}`)); report != nil {
		t.Fatalf("report = %v", report)
	}
}
