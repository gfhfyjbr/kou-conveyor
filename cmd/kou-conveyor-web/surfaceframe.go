package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"io"
	"net/http"
	"os"
	"path"
	"regexp"
	"strings"
	"time"
)

// The live HTML of a surface of the genui plugin (examples/plugins/genui):
// the page of an Html component, which the surface's card frames. The UI
// tool keeps each version of a surface in
// .harness/ui/<session>/<surface>/v<N>.json; an Html of it holds its HTML,
// or names a file of the workspace that does, which is read anew each time.
//
// What the page shows comes from the agent, and so from whatever the agent
// read. It runs sandboxed, in an origin of its own: it reaches neither the
// cockpit, its API and its cookies, nor the network. It talks only to the
// card that frames it, by messages, through the bridge served ahead of it:
// window.kou.

const (
	maxSurfaceState = 16 << 20 // bytes of a version of a surface
	maxSurfaceHTML  = 5 << 20  // bytes of an Html's file
)

// surfaceFramePolicy is the page's CSP, in place of the cockpit's: a sandbox
// with scripts but without an origin, eval, forms or popups; styles,
// pictures, fonts and media only from the page itself; no request of any
// kind; and only a page of the cockpit may frame it.
const surfaceFramePolicy = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; " +
	"img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; base-uri 'none'; form-action 'none'; " +
	"frame-ancestors 'self'"

// What names a page, as the UI tool writes it: the session's folder (its ID,
// with what is not a letter, a digit, "-" or "_" made "_"), the surface's
// ID, the component's ID, and the version.
var (
	surfaceFolder    = regexp.MustCompile(`^[A-Za-z0-9_-]{1,80}$`)
	surfaceName      = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,47}$`)
	surfaceComponent = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$`)
	surfaceVersion   = regexp.MustCompile(`^[1-9][0-9]{0,8}$`)
)

var (
	errSurfaceTooLarge = errors.New("too large")
	errSurfaceNotFile  = errors.New("not a file")
)

// handleSurfaceFrame serves the page of an Html component:
// GET …/sessions/{id}/surfaces/{surface}/html/{component}?v=<version>. The
// card asks with HEAD first, and then again for an Html of a file, whose
// ETag tells it the file changed.
func (s *server) handleSurfaceFrame(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	folder, surface, id, version := r.PathValue("id"), r.PathValue("surface"), r.PathValue("component"), r.URL.Query().Get("v")
	if !surfaceFolder.MatchString(folder) || !surfaceName.MatchString(surface) || !surfaceComponent.MatchString(id) || !surfaceVersion.MatchString(version) {
		writeError(w, http.StatusBadRequest, "not an Html of a surface: …/sessions/<session>/surfaces/<surface>/html/<component>?v=<version>")
		return
	}
	root, err := os.OpenRoot(ws.Path)
	if err != nil {
		writeError(w, http.StatusConflict, "the workspace's folder is missing: "+display(ws.Path))
		return
	}
	defer root.Close()
	stored := path.Join(".harness/ui", folder, surface, "v"+version+".json")
	raw, err := readSurfaceFile(root, stored, maxSurfaceState)
	if err != nil {
		writeError(w, http.StatusNotFound, "no version "+version+" of surface "+surface+": "+stored)
		return
	}
	var state struct {
		Components []map[string]any `json:"components"`
	}
	if err := json.Unmarshal(raw, &state); err != nil {
		writeError(w, http.StatusUnprocessableEntity, stored+" is not a version of a surface: "+err.Error())
		return
	}
	var html map[string]any
	for _, c := range state.Components {
		if c["id"] == id {
			html = c
			break
		}
	}
	if html == nil || html["component"] != "Html" {
		writeError(w, http.StatusNotFound, "no Html "+id+" in version "+version+" of surface "+surface)
		return
	}
	var content []byte
	if inline, _ := html["html"].(string); inline != "" {
		content = []byte(inline)
	} else if file, _ := html["file"].(string); strings.TrimSpace(file) != "" {
		relative, ok := workspaceRelative(strings.TrimPrefix(strings.TrimSpace(file), "$"))
		if ext := strings.ToLower(path.Ext(relative)); !ok || relative == "." || ext != ".html" && ext != ".htm" {
			writeError(w, http.StatusBadRequest, "an Html's file is a .html or .htm file of the workspace: "+file)
			return
		}
		if hiddenSurfaceFile(relative) {
			writeError(w, http.StatusBadRequest, "a surface does not show .git or .env files: "+relative)
			return
		}
		content, err = readSurfaceFile(root, relative, maxSurfaceHTML)
		switch {
		case errors.Is(err, errSurfaceTooLarge):
			writeError(w, http.StatusRequestEntityTooLarge, relative+" is over 5 MiB")
			return
		case errors.Is(err, errSurfaceNotFile):
			writeError(w, http.StatusBadRequest, "not a file: "+relative)
			return
		case err != nil:
			writeError(w, http.StatusNotFound, "no such file: "+relative)
			return
		}
	} else {
		writeError(w, http.StatusNotFound, "the Html "+id+" has neither html nor a file")
		return
	}

	page := surfacePage(content)
	sum := sha256.Sum256(page)
	h := w.Header()
	// In place of the cockpit's headers (server.go's secure): the page is
	// framed, by the cockpit only, under its own policy.
	h.Set("Content-Security-Policy", surfaceFramePolicy)
	h.Set("X-Frame-Options", "SAMEORIGIN")
	h.Set("Content-Type", "text/html; charset=utf-8")
	h.Set("Cache-Control", "no-cache")
	h.Set("ETag", `"`+hex.EncodeToString(sum[:12])+`"`)
	http.ServeContent(w, r, "", time.Time{}, bytes.NewReader(page))
}

// hiddenSurfaceFile is whether a surface keeps away from a file of the
// workspace, as the genui tool does (workspace_path in its ui.py): what is
// in .git, and .env files.
func hiddenSurfaceFile(relative string) bool {
	parts := strings.Split(relative, "/")
	if parts[0] == ".git" {
		return true
	}
	for _, part := range parts {
		if part == ".env" || strings.HasPrefix(part, ".env.") {
			return true
		}
	}
	return false
}

// readSurfaceFile reads a file of the workspace, of at most limit bytes.
func readSurfaceFile(root *os.Root, name string, limit int64) ([]byte, error) {
	file, err := root.Open(name)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, err
	}
	if info.IsDir() {
		return nil, errSurfaceNotFile
	}
	if info.Size() > limit {
		return nil, errSurfaceTooLarge
	}
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, errSurfaceTooLarge
	}
	return data, nil
}

// surfacePage is the page of an Html: what the agent wrote, after a head of
// the cockpit's, which gives it the bridge and the theme's font and colors.
// What it wrote may be a whole document, or a part of one.
func surfacePage(content []byte) []byte {
	var b bytes.Buffer
	b.Grow(len(surfacePageHead) + len(content))
	b.WriteString(surfacePageHead)
	b.Write(content)
	return b.Bytes()
}

const surfacePageHead = `<!doctype html><meta charset="utf-8">` +
	`<style>:where(html){color-scheme:light dark;color:var(--fg,CanvasText);font:13px/1.5 var(--sans,system-ui,sans-serif)}:where(body){margin:0}</style>` +
	"<script>" + surfaceBridge + "</script>\n"

// surfaceBridge is window.kou: the page's side of its messages with its
// card (examples/plugins/genui/web/genui.js), each {kou: 1, type, …}.
//
//   - kou.theme is {scheme, vars}: the cockpit's colors and fonts, set on
//     the page's root as custom properties (--bg, --fg, --accent, … and the
//     series' --s0 … --s5) before it is first drawn, from the frame's URL's
//     fragment, and again whenever the cockpit's theme changes.
//   - kou.data is what the card gives the page: the values of the pointers
//     the Html names in "inputs", by pointer. kou.on('data', fn) has fn
//     called with it when it comes, and whenever it changes; registered
//     after it came, fn is called with it at once.
//   - kou.set(pointer, value) sets one of those values in the card, as a
//     field of the surface would; kou.action(name, data) sends one of the
//     events the Html names in "actions" to the agent, as a button would;
//     kou.openFile(path, line) opens a file the surface reads. The card
//     takes them only while the user works in the page.
//   - The page tells the card how tall it is, as it changes, from when it
//     has its data (or a moment has passed without it): before, it is not
//     yet what it shows, and a frame loaded again, its card drawn again,
//     would shrink and grow back. kou.resize(height) sets that height
//     instead, and kou.resize() goes back to measuring it.
//   - In the iPhone app the page is a web view's own, not in a frame
//     (ios/KouConveyor/Views/SurfaceWebView.swift). What it says goes out as
//     a "kou:post" event on the window, its detail the message in JSON, which
//     the app hears from a script world of its own: one the page has no
//     reach into, which tells it whether the user is at work in the page.
//     What the app says comes in as a "kou:native" event, its detail the
//     message in JSON. The app sends it from its world through a
//     MessageChannel: WebKit takes a script the app runs for a gesture of
//     the user's, and the page's handlers would run inside it. The "ready"
//     message says which bridge this is (bridge: 2), for the app to tell an
//     older cockpit's.
const surfaceBridge = `(() => {
  'use strict';
  const up = window.parent;
  const native = up === window;
  const root = document.documentElement;
  const hooks = { data: new Set(), theme: new Set() };
  let data = {};
  let theme = { scheme: 'dark', vars: {} };
  let received = false;
  let settled = false;
  let manual = null;
  let reported = -1;
  let errors = 0;
  const post = (message) => {
    try {
      if (native) dispatchEvent(new CustomEvent('kou:post', { detail: JSON.stringify({ ...message, kou: 1 }) }));
      else up.postMessage({ ...message, kou: 1 }, '*');
    } catch (error) { console.warn('kou:', error); }
  };
  const call = (fn, value) => {
    try { fn(value); } catch (error) { console.error(error); }
  };
  const emit = (type, value) => { for (const fn of [...hooks[type]]) call(fn, value); };
  function paint(next) {
    if (!next || typeof next !== 'object') return false;
    const vars = {};
    for (const [name, value] of Object.entries(next.vars && typeof next.vars === 'object' ? next.vars : {})) {
      if (!/^--[A-Za-z0-9-]{1,40}$/.test(name) || typeof value !== 'string' || value.length > 400) continue;
      vars[name] = value;
      root.style.setProperty(name, value);
    }
    const scheme = next.scheme === 'light' ? 'light' : 'dark';
    root.style.colorScheme = scheme;
    root.setAttribute('data-theme', scheme);
    theme = { scheme, vars };
    return true;
  }
  try { paint(JSON.parse(decodeURIComponent(location.hash.slice(1)))); } catch {}
  function report() {
    if (!settled) return;
    const height = manual ?? Math.ceil(root.getBoundingClientRect().height);
    if (height === reported) return;
    reported = height;
    post({ type: 'size', height });
  }
  const kou = Object.freeze({
    get data() { return data; },
    get theme() { return theme; },
    on(type, fn) {
      if (!hooks[type] || typeof fn !== 'function') return () => {};
      hooks[type].add(fn);
      if (type === 'data' && received) queueMicrotask(() => { if (hooks.data.has(fn)) call(fn, data); });
      return () => hooks[type].delete(fn);
    },
    off(type, fn) { hooks[type]?.delete(fn); },
    action(name, value) { post({ type: 'action', name: String(name), data: value === undefined ? null : value }); },
    set(pointer, value) { post({ type: 'set', pointer: String(pointer), value: value === undefined ? null : value }); },
    openFile(file, line) { post({ type: 'open', path: String(file), line: Number.isInteger(line) && line > 0 ? line : null }); },
    resize(height) {
      manual = height == null ? null : Math.max(0, Math.ceil(Number(height)) || 0);
      report();
    },
  });
  window.kou = kou;
  function receive(message) {
    if (!message || message.kou !== 1) return;
    if (message.theme && paint(message.theme)) emit('theme', theme);
    if ((message.type === 'init' || message.type === 'data') && message.data && typeof message.data === 'object') {
      data = message.data;
      received = true;
      emit('data', data);
    }
    if (message.type === 'init') settle();
  }
  addEventListener('message', (event) => {
    if (native || event.source !== up) return;
    receive(event.data);
  });
  if (native) {
    addEventListener('kou:native', (event) => {
      if (typeof event.detail !== 'string') return;
      let message;
      try { message = JSON.parse(event.detail); } catch { return; }
      receive(message);
    });
  }
  function settle() {
    if (settled) return;
    settled = true;
    report();
  }
  setTimeout(settle, 1500);
  addEventListener('error', (event) => {
    if (errors++ < 3) post({ type: 'error', message: String(event.message || event.error || 'error').slice(0, 300) });
  });
  new ResizeObserver(report).observe(root);
  post({ type: 'ready', bridge: 2 });
})();`
