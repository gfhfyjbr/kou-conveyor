package main

import (
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

// The rules of the canvas's style sheet: their selectors, each.
func canvasSelectors(t *testing.T) []string {
	t.Helper()
	css, err := os.ReadFile("plugins/canvas/web/canvas.css")
	if err != nil {
		t.Fatal(err)
	}
	text := regexp.MustCompile(`(?s)/\*.*?\*/`).ReplaceAllString(string(css), "")
	var selectors []string
	var buf strings.Builder
	for _, r := range text {
		switch r {
		case '{':
			head := strings.TrimSpace(buf.String())
			buf.Reset()
			if head == "" || strings.HasPrefix(head, "@") || regexp.MustCompile(`^(from|to|[\d.]+%)(\s*,\s*(from|to|[\d.]+%))*$`).MatchString(head) {
				continue
			}
			for _, s := range strings.Split(head, ",") {
				selectors = append(selectors, strings.Join(strings.Fields(s), " "))
			}
		case '}', ';':
			buf.Reset()
		default:
			buf.WriteRune(r)
		}
	}
	return selectors
}

// The canvas styles only what is its own: its classes, and the page in an
// agent node's frame under the attribute only that page has. Nothing of it
// reaches the thinking of the transcript, which the timeline styles alone.
func TestCanvasChatStylesAreScoped(t *testing.T) {
	selectors := canvasSelectors(t)
	if len(selectors) < 100 {
		t.Fatalf("too few selectors read: %d", len(selectors))
	}
	// From before the chat came; the canvas's buttons in menus of its own.
	before := map[string]bool{".act.danger": true, ".act.danger:hover": true}
	embed := 0
	for _, s := range selectors {
		switch {
		case strings.Contains(s, "data-cv-embed"):
			embed++
			if !strings.HasPrefix(s, `:root[data-cv-embed="chat"] `) {
				t.Errorf("a rule for the chat's page that is not under its attribute: %q", s)
			}
		case strings.Contains(s, "cv-"), before[s]:
		default:
			t.Errorf("a rule of the canvas for what is not its own: %q", s)
		}
		// The feed's own row of thinking (.cv-e-thought) is the canvas's.
		if regexp.MustCompile(`(?i)(^|[^-\w])(\.|\[[^\]]*)(thought|reasoning|think)`).MatchString(s) {
			t.Errorf("the canvas styles the thinking of a transcript: %q", s)
		}
	}
	if embed == 0 {
		t.Error("no rule for the chat's page")
	}
}

// Only the page of an agent's chat may be framed, and only by a page of
// this origin; the cockpit itself, its other addresses and its API are
// framed by no one.
func TestCanvasChatFraming(t *testing.T) {
	h := newHarness(t)
	for _, c := range []struct {
		method, path, ancestors, xfo string
	}{
		{"GET", "/?embed=canvas-chat&ws=w&session=s", "frame-ancestors 'self'", "SAMEORIGIN"},
		{"GET", "/?embed=canvas-chat", "frame-ancestors 'self'", "SAMEORIGIN"},
		{"GET", "/", "frame-ancestors 'none'", "DENY"},
		{"GET", "/?embed=other", "frame-ancestors 'none'", "DENY"},
		{"GET", "/kernel/host.js?embed=canvas-chat", "frame-ancestors 'none'", "DENY"},
		{"GET", "/api/config?embed=canvas-chat", "frame-ancestors 'none'", "DENY"},
		{"POST", "/?embed=canvas-chat", "frame-ancestors 'none'", "DENY"},
	} {
		req, err := http.NewRequest(c.method, h.http.URL+c.path, nil)
		if err != nil {
			t.Fatal(err)
		}
		res, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		if csp := res.Header.Get("Content-Security-Policy"); !strings.Contains(csp, c.ancestors) {
			t.Errorf("%s %s: CSP %q, want %s", c.method, c.path, csp, c.ancestors)
		}
		if xfo := res.Header.Get("X-Frame-Options"); xfo != c.xfo {
			t.Errorf("%s %s: X-Frame-Options %q, want %s", c.method, c.path, xfo, c.xfo)
		}
	}
}

// What the page and the page in a frame agree on: the chat's address, the
// messages taken, the addresses followed, and how many chats there are.
func TestCanvasChatScripts(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("no node")
	}
	dir, err := filepath.Abs("plugins/canvas/web")
	if err != nil {
		t.Fatal(err)
	}
	script := `
import assert from 'node:assert/strict';
const embed = await import(process.argv[1] + '/embed.js');
const lod = await import(process.argv[1] + '/lod.js');

assert.equal(embed.chatURL('ws1', 'abc-1'), '/?embed=canvas-chat&ws=ws1&session=abc-1#/w/ws1/s/abc-1');
assert.equal(embed.chatURL('ws1', ''), '');
assert.equal(embed.chatURL('ws/1', 'abc'), '');
assert.equal(embed.chatURL('ws1', 'a"><script>'), '');

assert.ok(embed.openable('#/w/ws1/s/abc-1'));
assert.ok(embed.openable('#/w/ws1/c/board'));
assert.ok(embed.openable('#/w/ws1'));
assert.ok(!embed.openable('javascript:alert(1)'));
assert.ok(!embed.openable('https://example.com/#/w/ws1'));
assert.ok(!embed.openable('#/w/ws1/s/abc?x'));
assert.ok(!embed.openable(null));

const from = {};
const ok = { source: from, origin: 'http://h', data: { 'kou-canvas-chat': true, type: 'ready' } };
assert.equal(embed.message(ok, from, 'http://h').type, 'ready');
assert.equal(embed.message({ ...ok, source: {} }, from, 'http://h'), null);
assert.equal(embed.message({ ...ok, origin: 'http://evil' }, from, 'http://h'), null);
assert.equal(embed.message({ ...ok, data: { type: 'ready' } }, from, 'http://h'), null);
assert.equal(embed.message({ ...ok, data: 'ready' }, from, 'http://h'), null);

const loc = (search, origin = 'http://h') => ({ search, origin });
const framed = (origin) => {
  const win = {};
  win.parent = { get location() { if (origin === null) throw new Error('cross-origin'); return { origin }; } };
  return win;
};
const q = '?embed=canvas-chat&ws=ws1&session=abc';
assert.deepEqual(embed.embedded(loc(q), framed('http://h')), { ws: 'ws1', sid: 'abc' });
const top = {}; top.parent = top;
assert.equal(embed.embedded(loc(q), top), null, 'not framed');
assert.equal(embed.embedded(loc(q), framed('http://other')), null, 'framed by another origin');
assert.equal(embed.embedded(loc(q), framed(null)), null, 'framed by a page it may not read');
assert.equal(embed.embedded(loc('?embed=other&ws=ws1&session=abc'), framed('http://h')), null);
assert.equal(embed.embedded(loc('?embed=canvas-chat&ws=ws1&session=a/b'), framed('http://h')), null);
assert.equal(embed.embedded(loc(''), framed('http://h')), null);

// One chat where its streams hold the page's few connections (HTTP/1.1,
// EventSources); as many as CHATS over sockets or HTTP/2, fewer on a phone.
assert.equal(lod.chatsFor('http/1.1'), 1);
assert.equal(lod.chatsFor(''), 1);
assert.equal(lod.chatsFor(undefined), 1);
assert.equal(lod.chatsFor('http/1.1', false, true), 1);
assert.equal(lod.chatsFor('h2'), lod.CHATS);
assert.equal(lod.chatsFor('h3'), lod.CHATS);
assert.equal(lod.chatsFor('http/1.1', true), lod.CHATS, 'streams over sockets');
assert.equal(lod.chatsFor('', true), lod.CHATS);
assert.equal(lod.chatsFor('http/1.1', true, true), lod.COARSE_CHATS);
assert.equal(lod.chatsFor('h2', false, true), lod.COARSE_CHATS);
assert.ok(lod.CHATS > lod.COARSE_CHATS && lod.COARSE_CHATS > 1);
assert.equal(lod.PROTOCOL, '', 'no page here');

const budget = lod.createBudget(8);
budget.grant(['a', 'b', 'c']);
budget.touch('b');
assert.deepEqual([...budget.grant(['a', 'b', 'c'], 1)], ['b']);
assert.equal(budget.grant(['a', 'b', 'c']).size, 3);
budget.touch('c');
assert.deepEqual([...budget.grant(['a', 'b', 'c'], 1)], ['c']);

// The chat stays with the node used last: one that has just come — a
// child given its session — is behind it.
const chats = lod.createBudget(8);
assert.deepEqual([...chats.grant(['a', 'b'], 1, { behind: true })], ['a']);
chats.touch('b');
assert.deepEqual([...chats.grant(['a', 'b', 'n'], 1, { behind: true })], ['b']);
assert.deepEqual([...chats.grant(['n', 'a', 'b'], 1, { behind: true })], ['b']);
// The chats go to as many as there is room for, those used last first.
assert.deepEqual([...chats.grant(['n', 'a', 'b'], 2, { behind: true })], ['b', 'n']);

// The nodes used, the latest first, go with the canvas (canvas.used): a
// page opened anew gives them their chats first — before the nodes never
// used, after those used since.
const tick = () => { const t = performance.now(); while (performance.now() === t); };
const used = lod.createBudget(8);
used.grant(['a', 'b', 'c'], 1, { behind: true });
assert.deepEqual(used.order(), [], 'wanting is no use');
used.touch('c');
tick();
used.touch('a');
assert.deepEqual(used.order(), ['a', 'c']);
const again = lod.createBudget(8);
again.restore([...used.order(), 7, null, 'a']);
assert.deepEqual(again.order(), ['a', 'c']);
assert.deepEqual([...again.grant(['n', 'c', 'a'], 1, { behind: true })], ['a']);
assert.deepEqual([...again.grant(['n', 'c'], 1, { behind: true })], ['c']);
again.touch('n');
assert.deepEqual([...again.grant(['n', 'c', 'a'], 2, { behind: true })], ['n', 'a']);
assert.deepEqual(again.order(), ['n', 'a', 'c']);
again.restore(['c', 'n']);
assert.deepEqual(again.order(), ['n', 'a', 'c'], 'what is known stays as it is');

// The server's no is asked again when told (the server came back); its yes
// holds for the page.
let asked = 0;
let csp = "default-src 'self'; frame-ancestors 'none'";
globalThis.fetch = async () => { asked++; return { ok: true, headers: { get: () => csp } }; };
assert.equal(await embed.framesAllowed(), false);
assert.equal(await embed.framesAllowed(), false);
assert.equal(asked, 1, 'a no holds until it is asked again');
csp = "default-src 'self'; frame-ancestors 'self'";
embed.askFramesAgain();
assert.equal(await embed.framesAllowed(), true);
assert.equal(asked, 2);
embed.askFramesAgain();
assert.equal(await embed.framesAllowed(), true);
assert.equal(asked, 2, 'a yes holds');
console.log('ok');
`
	cmd := exec.Command(node, "--input-type=module", "-e", script, "file://"+dir)
	out, err := cmd.CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "ok" {
		t.Fatalf("node: %v\n%s", err, out)
	}
}

// TestCanvasChatServe serves a canvas with a kou agent that has thought,
// run a tool and answered, another that answered once, and one none of
// whose runs began (its session has an ID, no transcript), for a browser
// to look at: it runs only with KOU_CONVEYOR_CANVAS_CHAT_SERVE naming a
// directory, writes the address there (url), the canvas's (canvas) and the
// session of the one that never ran (fresh), and serves until a file named
// done is there; a file named drop breaks the connections, as a restart
// does. KOU_CONVEYOR_CANVAS_CHAT_ASSETS serves other assets (a build from
// before, to compare with).
func TestCanvasChatServe(t *testing.T) {
	out := os.Getenv("KOU_CONVEYOR_CANVAS_CHAT_SERVE")
	if out == "" {
		t.Skip("KOU_CONVEYOR_CANVAS_CHAT_SERVE is not set")
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CACHE_HOME", home+"/.cache")
	t.Setenv("XDG_CONFIG_HOME", home+"/.config")
	h := newHarnessWith(t, func(o *options) {
		o.canvas = true
		if dir := os.Getenv("KOU_CONVEYOR_CANVAS_CHAT_ASSETS"); dir != "" {
			a, err := resolveAssets(dir)
			if err != nil {
				t.Fatal(err)
			}
			o.assets = a
		}
	})
	res, applied := h.do("POST", "/api/canvases/board/ops", `{"create":true,"title":"Board","ops":[
		{"op":"node.add","node":{"kind":"agent","title":"Helper","x":64,"y":64,"w":620,"h":640}},
		{"op":"node.add","node":{"kind":"agent","title":"Second","x":736,"y":64}},
		{"op":"node.add","node":{"kind":"agent","title":"Fresh","x":736,"y":660,"w":440,"h":300}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("ops: %d %v", res.StatusCode, applied)
	}
	ids, _ := applied["ids"].(map[string]any)
	agent, _ := ids["0"].(string)
	second, _ := ids["1"].(string)
	fresh, _ := ids["2"].(string)
	session := func(node string) string {
		_, snapshot := h.do("GET", "/api/canvases/board", "")
		doc, _ := snapshot["doc"].(map[string]any)
		nodes, _ := doc["nodes"].([]any)
		for _, n := range nodes {
			if n, _ := n.(map[string]any); n["id"] == node {
				runtime, _ := n["runtime"].(map[string]any)
				s, _ := runtime["session"].(string)
				return s
			}
		}
		return ""
	}
	// idle waits for the node's runs to end.
	idle := func(node string, prompts int) {
		deadline := time.Now().Add(20 * time.Second)
		for {
			_, view := h.do("GET", "/api/sessions/"+session(node), "")
			entries, _ := view["entries"].([]any)
			users := 0
			for _, e := range entries {
				if e, _ := e.(map[string]any); e["kind"] == "user" {
					users++
				}
			}
			if users >= prompts && view["run"] == nil {
				return
			}
			if time.Now().After(deadline) {
				t.Fatalf("the runs did not end: %v", view)
			}
			time.Sleep(50 * time.Millisecond)
		}
	}
	for i, text := range []string{"tool", "unfinished", "hello **there**, with `code`"} {
		if res, sent := h.do("POST", "/api/canvases/board/nodes/"+agent+"/send", `{"text":"`+strings.ReplaceAll(text, `"`, `\"`)+`"}`); res.StatusCode != http.StatusOK {
			t.Fatalf("send: %d %v", res.StatusCode, sent)
		}
		idle(agent, i+1)
	}
	if res, _ := h.do("POST", "/api/canvases/board/nodes/"+second+"/send", `{"text":"hi"}`); res.StatusCode != http.StatusOK {
		t.Fatal("send to the second")
	}
	idle(second, 1)
	// The third has its session's ID from when it was made, and no session
	// to show: none of its runs began.
	freshSession := session(fresh)
	if freshSession == "" {
		t.Fatal("the node that never ran has no session")
	}
	if res, view := h.do("GET", "/api/sessions/"+freshSession, ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("the session of the node that never ran: %d %v", res.StatusCode, view)
	}
	ws := h.server.workspaces.startup().ID
	write := func(name, text string) {
		if err := os.WriteFile(filepath.Join(out, name), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("fresh", freshSession)
	write("session", session(agent))
	write("canvas", h.http.URL+"/#/w/"+ws+"/c/board")
	write("chat", h.http.URL+"/#/w/"+ws+"/s/"+session(agent))
	write("url", h.http.URL)
	deadline := time.Now().Add(15 * time.Minute)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(filepath.Join(out, "done")); err == nil {
			return
		}
		// A file named drop: the server goes as a new build of it takes
		// over (rebuild.go) — every connection breaks, the socket stays.
		if err := os.Remove(filepath.Join(out, "drop")); err == nil {
			h.http.CloseClientConnections()
			h.server.sockets.drop()
		}
		time.Sleep(200 * time.Millisecond)
	}
}
