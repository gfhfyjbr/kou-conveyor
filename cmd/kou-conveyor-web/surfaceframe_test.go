package main

import (
	"io"
	"net/http"
	"os/exec"
	"strings"
	"testing"
)

// TestSurfaceBridge runs window.kou in a frame, where it talks to the card
// by messages, and as a web view's own page (the iPhone app's), where it
// talks to the app by "kou:post" events out and "kou:native" events in.
func TestSurfaceBridge(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("no node")
	}
	script := `
import assert from 'node:assert/strict';
const bridge = process.argv[1];

// page makes a window with what the bridge uses, and runs the bridge in it.
function page(framed) {
  const listeners = {};
  const props = {};
  const posted = [];
  const win = {
    addEventListener(type, fn) { (listeners[type] ??= []).push(fn); },
    dispatchEvent(event) { for (const fn of listeners[event.type] ?? []) fn(event); return true; },
  };
  win.window = win;
  win.parent = framed ? { postMessage(message) { posted.push(message); } } : win;
  const root = {
    style: { setProperty(name, value) { props[name] = value; }, colorScheme: '' },
    attributes: {},
    setAttribute(name, value) { this.attributes[name] = value; },
    getBoundingClientRect() { return { height: 120.4 }; },
  };
  const env = {
    window: win, document: { documentElement: root }, location: { hash: '' },
    addEventListener: win.addEventListener, dispatchEvent: win.dispatchEvent,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    ResizeObserver: class { observe() {} },
    setTimeout: () => 0, queueMicrotask, console, JSON, Object, Math, Number, String, Set,
  };
  new Function(...Object.keys(env), bridge)(...Object.values(env));
  return { win, posted, props, root, listeners };
}

// In a frame: messages to the parent, none to an app.
{
  const p = page(true);
  assert.equal(p.listeners['kou:native'], undefined);
  assert.deepEqual(p.posted, [{ type: 'ready', bridge: 2, kou: 1 }]);
  const message = { kou: 1, type: 'init', data: { '/a': 1 }, theme: { scheme: 'light', vars: { '--bg': '#fff' } } };
  for (const fn of p.listeners.message) fn({ source: {}, data: message });
  assert.deepEqual(p.win.kou.data, {}, 'a message from another window');
  for (const fn of p.listeners.message) fn({ source: p.win.parent, data: message });
  assert.deepEqual(p.win.kou.data, { '/a': 1 });
  assert.equal(p.props['--bg'], '#fff');
  assert.deepEqual(p.posted.at(-1), { type: 'size', height: 121, kou: 1 });
}

// A web view's own page: "kou:post" events out, "kou:native" events in.
{
  const p = page(false);
  const out = [];
  p.win.addEventListener('kou:post', (event) => out.push(JSON.parse(event.detail)));
  p.win.kou.action('explain', { id: 'x' });
  assert.deepEqual(out, [{ type: 'action', name: 'explain', data: { id: 'x' }, kou: 1 }]);
  for (const fn of p.listeners.message) fn({ source: p.win, data: { kou: 1, type: 'init', data: { '/a': 2 } } });
  assert.deepEqual(p.win.kou.data, {}, 'a message posted to itself');
  const app = (detail) => p.win.dispatchEvent({ type: 'kou:native', detail });
  let seen = null;
  p.win.kou.on('data', (data) => { seen = data; });
  app(JSON.stringify({ kou: 1, type: 'init', data: { '/a': 3 }, theme: { scheme: 'dark', vars: { '--fg': '#eee', 'bad': 'x' } } }));
  assert.deepEqual(seen, { '/a': 3 });
  assert.equal(p.props['--fg'], '#eee');
  assert.equal(p.props.bad, undefined);
  assert.equal(p.root.attributes['data-theme'], 'dark');
  assert.deepEqual(out.at(-1), { type: 'size', height: 121, kou: 1 });
  app(JSON.stringify({ type: 'data', data: { '/a': 4 } }));
  assert.deepEqual(p.win.kou.data, { '/a': 3 }, 'not a kou message');
  app('{"kou":1,"type":"data"');
  app({ kou: 1, type: 'data', data: { '/a': 5 } });
  assert.deepEqual(p.win.kou.data, { '/a': 3 }, 'not JSON, or not a string');
  app(JSON.stringify({ kou: 1, type: 'data', data: { '/a': 6 } }));
  assert.deepEqual(seen, { '/a': 6 });
  p.win.kou.set('/filter', 'x');
  p.win.kou.openFile('go.mod', 3);
  assert.deepEqual(out.slice(-2), [{ type: 'set', pointer: '/filter', value: 'x', kou: 1 }, { type: 'open', path: 'go.mod', line: 3, kou: 1 }]);
  assert.equal(p.win.__kouNative, undefined);
}
console.log('ok');
`
	out, err := exec.Command(node, "--input-type=module", "-e", script, surfaceBridge).CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "ok" {
		t.Fatalf("node: %v\n%s", err, out)
	}
}

// page asks the harness's server for a page, and gives its body.
func page(h *harness, method, path string, headers ...string) (*http.Response, string) {
	h.Helper()
	req, err := http.NewRequest(method, h.http.URL+path, nil)
	if err != nil {
		h.Fatal(err)
	}
	for i := 0; i+1 < len(headers); i += 2 {
		req.Header.Set(headers[i], headers[i+1])
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	body, err := io.ReadAll(res.Body)
	if err != nil {
		h.Fatal(err)
	}
	return res, string(body)
}

func TestSurfaceFrame(t *testing.T) {
	h := newHarness(t)
	workspaceFiles(t, h, map[string]string{
		".harness/ui/s-1/deps/v1.json": `{"kou": 1, "surface": "deps", "version": 1, "components": [
 {"id": "root", "component": "Column", "children": ["map", "page", "text"]},
 {"id": "map", "component": "Html", "html": "<p id=x>hi</p>", "alt": "a map"},
 {"id": "page", "component": "Html", "file": "web/deps.html", "alt": "from a file"},
 {"id": "gone", "component": "Html", "file": "web/gone.html", "alt": "a file that is not there"},
 {"id": "notes", "component": "Html", "file": "notes.txt", "alt": "not HTML"},
 {"id": "big", "component": "Html", "file": "big.html", "alt": "too large"},
 {"id": "git", "component": "Html", "file": ".git/info.html", "alt": "in .git"},
 {"id": "env", "component": "Html", "file": "web/.env.html", "alt": "an .env file"},
 {"id": "empty", "component": "Html", "alt": "neither"},
 {"id": "text", "component": "Text", "text": "not an Html"}]}`,
		"web/deps.html":  "<h1>deps</h1>",
		"notes.txt":      "<b>notes</b>",
		"big.html":       strings.Repeat("x", maxSurfaceHTML+1),
		".git/info.html": "<b>git</b>",
		"web/.env.html":  "<b>secret</b>",
	})
	base := "/api/sessions/s-1/surfaces/deps/html/"

	res, body := page(h, "GET", base+"map?v=1")
	if res.StatusCode != http.StatusOK {
		t.Fatalf("GET map: %d %s", res.StatusCode, body)
	}
	for name, want := range map[string]string{
		"Content-Security-Policy": surfaceFramePolicy,
		"X-Frame-Options":         "SAMEORIGIN",
		"Content-Type":            "text/html; charset=utf-8",
		"Cache-Control":           "no-cache",
		"X-Content-Type-Options":  "nosniff",
		"Referrer-Policy":         "no-referrer",
	} {
		if got := res.Header.Get(name); got != want {
			t.Errorf("%s = %q, want %q", name, got, want)
		}
	}
	if !strings.HasPrefix(body, surfacePageHead) || !strings.HasSuffix(body, "<p id=x>hi</p>") {
		t.Errorf("the page is not the bridge and the HTML:\n%.300s", body)
	}
	if strings.Contains(strings.ToLower(surfaceBridge), "</script") {
		t.Error("the bridge ends its own script")
	}
	etag := res.Header.Get("ETag")
	if etag == "" {
		t.Fatal("no ETag")
	}

	// What the card asks first, and asks again of an Html of a file.
	res, body = page(h, "HEAD", base+"map?v=1")
	if res.StatusCode != http.StatusOK || body != "" || res.Header.Get("ETag") != etag {
		t.Errorf("HEAD map: %d %q ETag %q, want 200, no body, %q", res.StatusCode, body, res.Header.Get("ETag"), etag)
	}
	if res, _ = page(h, "GET", base+"map?v=1", "If-None-Match", etag); res.StatusCode != http.StatusNotModified {
		t.Errorf("GET map, as it was: %d, want 304", res.StatusCode)
	}
	ws := h.server.workspaces.startup().ID
	if res, body = page(h, "GET", "/api/w/"+ws+"/sessions/s-1/surfaces/deps/html/map?v=1&rev=x"); res.StatusCode != http.StatusOK || !strings.HasSuffix(body, "<p id=x>hi</p>") {
		t.Errorf("GET map of the workspace: %d %.200s", res.StatusCode, body)
	}

	res, body = page(h, "GET", base+"page?v=1")
	if res.StatusCode != http.StatusOK || !strings.HasSuffix(body, "<h1>deps</h1>") || res.Header.Get("Content-Security-Policy") != surfaceFramePolicy {
		t.Fatalf("GET page: %d %.200s", res.StatusCode, body)
	}
	first := res.Header.Get("ETag")
	workspaceFiles(t, h, map[string]string{"web/deps.html": "<h1>deps, changed</h1>"})
	if res, _ = page(h, "HEAD", base+"page?v=1", "If-None-Match", first); res.StatusCode != http.StatusOK || res.Header.Get("ETag") == first {
		t.Errorf("HEAD page, changed: %d ETag %q, want 200 and a new ETag", res.StatusCode, res.Header.Get("ETag"))
	}

	for path, want := range map[string]int{
		base + "map?v=2":   http.StatusNotFound, // no such version
		base + "nope?v=1":  http.StatusNotFound,
		base + "text?v=1":  http.StatusNotFound, // not an Html
		base + "empty?v=1": http.StatusNotFound,
		base + "gone?v=1":  http.StatusNotFound,
		base + "notes?v=1": http.StatusBadRequest,
		base + "big?v=1":   http.StatusRequestEntityTooLarge,
		base + "git?v=1":   http.StatusBadRequest,
		base + "env?v=1":   http.StatusBadRequest,
		base + "map":       http.StatusBadRequest,
		base + "map?v=0":   http.StatusBadRequest,
		base + "map?v=1x":  http.StatusBadRequest,
		base + ".map?v=1":  http.StatusBadRequest,
		"/api/sessions/s-1/surfaces/Deps/html/map?v=1":      http.StatusBadRequest,
		"/api/sessions/..%2Fs-1/surfaces/deps/html/map?v=1": http.StatusBadRequest,
	} {
		res, body := page(h, "GET", path)
		if res.StatusCode != want {
			t.Errorf("GET %s: %d %s, want %d", path, res.StatusCode, body, want)
		}
		// What fails is no page: it is framed by no one.
		if res.Header.Get("X-Frame-Options") != "DENY" || !strings.Contains(res.Header.Get("Content-Security-Policy"), "frame-ancestors 'none'") {
			t.Errorf("GET %s may be framed: %q, %q", path, res.Header.Get("X-Frame-Options"), res.Header.Get("Content-Security-Policy"))
		}
	}

	// The cockpit's own pages are still framed by no one.
	res, _ = page(h, "GET", "/")
	if res.Header.Get("X-Frame-Options") != "DENY" || !strings.Contains(res.Header.Get("Content-Security-Policy"), "frame-ancestors 'none'") {
		t.Errorf("GET / may be framed: %q, %q", res.Header.Get("X-Frame-Options"), res.Header.Get("Content-Security-Policy"))
	}
}
