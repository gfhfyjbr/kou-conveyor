package main

import (
	"bufio"
	"context"
	"encoding/json/v2"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func writeFiles(t *testing.T, directory string, files map[string]string) {
	t.Helper()
	for name, content := range files {
		path := filepath.Join(directory, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

func (h *harness) get(path string) (int, string, string) {
	h.Helper()
	res, err := http.Get(h.http.URL + path)
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(res.Body)
	return res.StatusCode, res.Header.Get("Content-Type"), string(body)
}

func pluginNamed(t *testing.T, listing map[string]any, name string) map[string]any {
	t.Helper()
	var found map[string]any
	for _, value := range listing["plugins"].([]any) {
		if current := value.(map[string]any); current["name"] == name {
			found = current // the last of the name: the one that replaces the others
		}
	}
	if found == nil {
		t.Fatalf("no plugin %q in %v", name, listing)
	}
	return found
}

// userPlugins is where the harness's user plugins live.
func (h *harness) userPlugins() string {
	return filepath.Join(filepath.Dir(h.server.opt.SettingsFile), "plugins")
}

func TestPluginsAreListedTrustedAndServed(t *testing.T) {
	h := newHarness(t)
	ws := h.server.workspaces.startup()
	writeFiles(t, filepath.Join(h.userPlugins(), "colors"), map[string]string{
		"plugin.json": `{"name": "colors", "description": "Colors", "web": {"style": "colors.css"}}`,
		"colors.css":  ":root { --accent: teal; }",
	})
	writeFiles(t, plugin.WorkspaceDirectory(ws.Path)+"/glance", map[string]string{
		"plugin.json":   `{"name": "glance", "tools": [{"name": "Glance", "description": "Look", "run": ["./glance.sh"]}], "commands": [{"name": "review", "description": "Review", "prompt": "Review {{args}}"}], "web": {"script": "web/plugin.js", "after": ["layout"]}}`,
		"glance.sh":     "echo hi",
		"web/plugin.js": "export default (cockpit) => cockpit.toast('hi');",
		"web/lib.js":    "export const x = 1;",
		"web/.secret":   "no",
		"notes.md":      "no",
	})

	res, listing := h.do("GET", "/api/plugins", "")
	if res.StatusCode != http.StatusOK || listing["trusted"] != false || listing["workspace_plugins"] != 1.0 || listing["workspace"] != ws.ID {
		t.Fatalf("listing = %v", listing)
	}
	core, colors, glance := pluginNamed(t, listing, "core"), pluginNamed(t, listing, "colors"), pluginNamed(t, listing, "glance")
	if core["source"] != "builtin" || core["active"] != true || len(core["tools"].([]any)) != 3 {
		t.Fatalf("core = %v", core)
	}
	style, _ := colors["style"].(string)
	if colors["active"] != true || !strings.HasPrefix(style, "/api/w/"+ws.ID+"/plugins/colors/v/") || !strings.HasSuffix(style, "/colors.css") ||
		colors["style_version"] == nil || colors["live"] != true {
		t.Fatalf("colors = %v", colors)
	}
	if glance["active"] != false || glance["reason"] != "the workspace is not trusted" || glance["script"] != nil {
		t.Fatalf("untrusted glance = %v", glance)
	}
	if status, _, _ := h.get("/api/w/" + ws.ID + "/plugins/glance/files/web/plugin.js"); status != http.StatusNotFound {
		t.Fatalf("an untrusted plugin served its script: %d", status)
	}
	if status, kind, body := h.get(style); status != http.StatusOK || !strings.HasPrefix(kind, "text/css") || !strings.Contains(body, "teal") {
		t.Fatalf("style: %d %s %q", status, kind, body)
	}

	res, listing = h.do("PUT", "/api/plugins/trust", `{"trusted": true}`)
	glance = pluginNamed(t, listing, "glance")
	script, _ := glance["script"].(string)
	if res.StatusCode != http.StatusOK || listing["trusted"] != true || glance["active"] != true || !strings.Contains(script, "/plugins/glance/v/") ||
		!strings.HasSuffix(script, "/web/plugin.js") || !slices.Equal(glance["after"].([]any), []any{"layout"}) {
		t.Fatalf("trusted listing = %v", listing)
	}
	if commands := glance["commands"].([]any); len(commands) != 1 || commands[0].(map[string]any)["prompt"] != "Review {{args}}" {
		t.Fatalf("commands = %v", commands)
	}
	// A module imports its neighbours from beside it, under the same version.
	neighbour := strings.TrimSuffix(script, "plugin.js") + "lib.js"
	if status, kind, body := h.get(neighbour); status != http.StatusOK || !strings.HasPrefix(kind, "text/javascript") || body != "export const x = 1;" {
		t.Fatalf("module: %d %s %q", status, kind, body)
	}
	base := "/api/w/" + ws.ID + "/plugins/glance/files/"
	if status, _, body := h.get(base + "web/lib.js"); status != http.StatusOK || body != "export const x = 1;" {
		t.Fatalf("unversioned: %d %q", status, body)
	}
	for _, path := range []string{"web/.secret", "glance.sh", "web/../../../../etc/passwd.js", "web/missing.js", "web"} {
		if status, _, _ := h.get(base + path); status != http.StatusNotFound {
			t.Errorf("%s: %d", path, status)
		}
	}
	// A symbolic link may not lead out of the plugin.
	outside := filepath.Join(t.TempDir(), "outside.js")
	os.WriteFile(outside, []byte("secret"), 0o644)
	os.Symlink(outside, filepath.Join(plugin.WorkspaceDirectory(ws.Path), "glance", "web", "link.js"))
	if status, _, _ := h.get(base + "web/link.js"); status != http.StatusNotFound {
		t.Fatalf("followed a link out of the plugin: %d", status)
	}

	res, listing = h.do("PUT", "/api/plugins/colors/enabled", `{"enabled": false}`)
	if colors = pluginNamed(t, listing, "colors"); res.StatusCode != http.StatusOK || colors["active"] != false || colors["reason"] != "turned off" {
		t.Fatalf("disabled colors = %v", colors)
	}
	if status, _, _ := h.get(style); status != http.StatusNotFound {
		t.Fatalf("a disabled plugin served its style: %d", status)
	}
	if res, _ := h.do("PUT", "/api/plugins/trust", `{}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("empty trust: %d", res.StatusCode)
	}
}

// The page is built by the cockpit's own plugins, compiled in: each is
// listed with its script and style, which are served.
func TestBuiltinPluginsBuildThePage(t *testing.T) {
	h := newHarness(t)
	_, listing := h.do("GET", "/api/plugins", "")
	if errs := listing["errors"].([]any); len(errs) != 0 {
		t.Fatalf("errors: %v", errs)
	}
	names := map[string]bool{}
	for _, value := range listing["plugins"].([]any) {
		names[value.(map[string]any)["name"].(string)] = true
	}
	for _, name := range []string{"theme", "layout", "ui", "session", "timeline", "composer", "commands", "models", "effort", "queue", "edit",
		"images", "session-list", "workspaces", "header", "inspector", "changes", "palette", "help", "connection", "accounts", "markdown", "plugins"} {
		p := pluginNamed(t, listing, name)
		if p["source"] != "builtin" || p["active"] != true || p["live"] != nil || p["code_version"] == nil {
			t.Errorf("%s = %v", name, p)
			continue
		}
		script, _ := p["script"].(string)
		if status, kind, body := h.get(script); status != http.StatusOK || !strings.HasPrefix(kind, "text/javascript") || !strings.Contains(body, "export default") {
			t.Errorf("%s: script %s: %d %s", name, script, status, kind)
		}
		if style, _ := p["style"].(string); style != "" {
			if status, kind, _ := h.get(style); status != http.StatusOK || !strings.HasPrefix(kind, "text/css") {
				t.Errorf("%s: style %s: %d %s", name, style, status, kind)
			}
		}
		after, _ := p["after"].([]any)
		for _, after := range after {
			if !names[after.(string)] {
				t.Errorf("%s loads after %s, which is not a plugin", name, after)
			}
		}
	}
}

// A user plugin replaces the built-in one of its name: the page is the
// user's to change, whole.
func TestUserPluginReplacesABuiltinOne(t *testing.T) {
	h := newHarness(t)
	writeFiles(t, filepath.Join(h.userPlugins(), "header"), map[string]string{
		"plugin.json": `{"name": "header", "web": {"script": "header.js"}}`,
		"header.js":   "export default () => {};",
	})
	_, listing := h.do("GET", "/api/plugins", "")
	var builtin, user map[string]any
	for _, value := range listing["plugins"].([]any) {
		if current := value.(map[string]any); current["name"] == "header" {
			if current["source"] == "builtin" {
				builtin = current
			} else {
				user = current
			}
		}
	}
	if builtin == nil || builtin["active"] != false || builtin["reason"] != "replaced by the user plugin of the same name" || builtin["script"] != nil {
		t.Fatalf("built-in header = %v", builtin)
	}
	if user == nil || user["active"] != true || !strings.HasSuffix(user["script"].(string), "/header.js") {
		t.Fatalf("user header = %v", user)
	}
	if status, _, body := h.get(user["script"].(string)); status != http.StatusOK || body != "export default () => {};" {
		t.Fatalf("user header's script: %d %q", status, body)
	}
}

// pluginEvents reads the plugin event stream of a workspace.
type pluginEvents struct {
	t      *testing.T
	events chan [2]string
}

func (h *harness) pluginEvents(path string) *pluginEvents {
	h.Helper()
	ctx, cancel := context.WithCancel(h.Context())
	h.Cleanup(cancel)
	req, _ := http.NewRequestWithContext(ctx, "GET", h.http.URL+path, nil)
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		h.Fatal(err)
	}
	if res.StatusCode != http.StatusOK || !strings.HasPrefix(res.Header.Get("Content-Type"), "text/event-stream") {
		h.Fatalf("events: %d %s", res.StatusCode, res.Header.Get("Content-Type"))
	}
	stream := &pluginEvents{t: h.T, events: make(chan [2]string, 16)}
	go func() {
		defer res.Body.Close()
		scanner := bufio.NewScanner(res.Body)
		scanner.Buffer(make([]byte, 1<<20), 8<<20)
		kind := ""
		for scanner.Scan() {
			line := scanner.Text()
			switch {
			case strings.HasPrefix(line, "event: "):
				kind = strings.TrimPrefix(line, "event: ")
			case strings.HasPrefix(line, "data: "):
				stream.events <- [2]string{kind, strings.TrimPrefix(line, "data: ")}
			}
		}
		close(stream.events)
	}()
	return stream
}

// next waits for the next event of a kind.
func (stream *pluginEvents) next(kind string) map[string]any {
	stream.t.Helper()
	deadline := time.After(10 * time.Second)
	for {
		select {
		case event, ok := <-stream.events:
			if !ok {
				stream.t.Fatal("the stream ended")
			}
			if event[0] != kind {
				continue
			}
			var decoded map[string]any
			if err := json.Unmarshal([]byte(event[1]), &decoded); err != nil {
				stream.t.Fatal(err)
			}
			return decoded
		case <-deadline:
			stream.t.Fatalf("no %s event", kind)
		}
	}
}

// until waits for a listing that satisfies ok: files written one after
// another may be seen half written first.
func (stream *pluginEvents) until(ok func(map[string]any) bool) map[string]any {
	stream.t.Helper()
	for {
		if listing := stream.next("plugins"); ok(listing) {
			return listing
		}
	}
}

func listed(listing map[string]any, name string) map[string]any {
	for _, value := range listing["plugins"].([]any) {
		if current := value.(map[string]any); current["name"] == name && current["active"] == true {
			return current
		}
	}
	return nil
}

// A page follows the plugins: the stream says when one comes, changes or
// goes, and a change of its style alone says so apart from one of its code.
func TestPluginEventsFollowChanges(t *testing.T) {
	h := newHarness(t)
	h.server.watch.interval = 20 * time.Millisecond
	stream := h.pluginEvents("/api/plugins/events")
	if first := stream.next("plugins"); listed(first, "layout") == nil || listed(first, "hello") != nil {
		t.Fatalf("first listing = %v", first)
	}

	directory := filepath.Join(h.userPlugins(), "hello")
	writeFiles(t, directory, map[string]string{
		"plugin.json":   `{"name": "hello", "web": {"script": "web/hello.js", "style": "web/hello.css"}}`,
		"web/hello.js":  "export default () => {};",
		"web/hello.css": ".hello { color: red; }",
	})
	hello := listed(stream.until(func(listing map[string]any) bool { return listed(listing, "hello") != nil }), "hello")

	// Its code changes: a new code version, the same style version.
	time.Sleep(10 * time.Millisecond)
	writeFiles(t, directory, map[string]string{"web/hello.js": "export default () => { /* v2 */ };"})
	changed := listed(stream.until(func(listing map[string]any) bool {
		current := listed(listing, "hello")
		return current != nil && current["code_version"] != hello["code_version"]
	}), "hello")
	if changed["code_version"] == hello["code_version"] || changed["style_version"] != hello["style_version"] || changed["script"] == hello["script"] {
		t.Fatalf("code change: %v, then %v", hello, changed)
	}
	if status, _, body := h.get(changed["script"].(string)); status != http.StatusOK || !strings.Contains(body, "v2") {
		t.Fatalf("new script: %d %q", status, body)
	}

	// Its style changes: the same code version, a new style version.
	time.Sleep(10 * time.Millisecond)
	writeFiles(t, directory, map[string]string{"web/hello.css": ".hello { color: blue; }"})
	restyled := listed(stream.until(func(listing map[string]any) bool {
		current := listed(listing, "hello")
		return current != nil && current["style_version"] != changed["style_version"]
	}), "hello")
	if restyled["code_version"] != changed["code_version"] || restyled["style_version"] == changed["style_version"] {
		t.Fatalf("style change: %v, then %v", changed, restyled)
	}

	// It goes.
	os.RemoveAll(directory)
	stream.until(func(listing map[string]any) bool { return listed(listing, "hello") == nil })

	// Turning a plugin off is a change of the listing too.
	h.do("PUT", "/api/plugins/palette/enabled", `{"enabled": false}`)
	stream.until(func(listing map[string]any) bool { return listed(listing, "palette") == nil })
}

// A server that serves its assets from a checkout follows them: its own
// plugins are live, and a change of the page itself has pages load again.
func TestLiveAssetsAreFollowed(t *testing.T) {
	dir := t.TempDir()
	writeFiles(t, dir, map[string]string{
		"static/index.html":          "<!doctype html><title>live</title>",
		"static/kernel/host.js":      "// kernel",
		"plugins/shell/plugin.json":  `{"name": "shell", "web": {"script": "web/shell.js"}}`,
		"plugins/shell/web/shell.js": "export default () => {};",
	})
	live, err := resolveAssets(dir)
	if err != nil || !live.live() {
		t.Fatalf("assets = %v, %v", live, err)
	}
	h := newHarnessWith(t, func(o *options) { o.assets = live })
	h.server.watch.interval = 20 * time.Millisecond
	if status, _, body := h.get("/"); status != http.StatusOK || !strings.Contains(body, "live") {
		t.Fatalf("page: %d %q", status, body)
	}
	stream := h.pluginEvents("/api/plugins/events")
	first := stream.next("plugins")
	shell := listed(first, "shell")
	if shell == nil || shell["source"] != "builtin" || shell["live"] != true || first["live"] != true || first["builtin_directory"] != filepath.Join(dir, "plugins") {
		t.Fatalf("listing = %v", first)
	}
	time.Sleep(10 * time.Millisecond)
	writeFiles(t, dir, map[string]string{"plugins/shell/web/shell.js": "export default () => { /* v2 */ };"})
	stream.until(func(listing map[string]any) bool {
		current := listed(listing, "shell")
		return current != nil && current["code_version"] != shell["code_version"]
	})
	writeFiles(t, dir, map[string]string{"static/kernel/host.js": "// kernel v2"})
	stream.next("kernel")
}

func TestResolveAssets(t *testing.T) {
	for _, setting := range []string{"", "embedded"} {
		if assets, err := resolveAssets(setting); err != nil || assets.live() {
			t.Errorf("%q: %v, %v", setting, assets, err)
		}
	}
	if _, err := resolveAssets(t.TempDir()); err == nil {
		t.Error("a directory without assets was taken")
	}
	// The package directory is a checkout's: it holds static/ and plugins/.
	if assets, err := resolveAssets("."); err != nil || !assets.live() {
		t.Errorf("checkout: %v, %v", assets, err)
	}
	// A program built from a checkout follows it wherever it is installed,
	// while the checkout is there.
	root, _ := filepath.Abs(filepath.Join("..", ".."))
	defer func(was string) { builtFrom = was }(builtFrom)
	builtFrom = root
	if assets, err := resolveAssets("auto"); err != nil || !assets.live() || assets.dir != filepath.Join(root, "cmd", "kou-conveyor-web") {
		t.Errorf("built from %s: %+v, %v", root, assets, err)
	}
	builtFrom = t.TempDir()
	if assets, err := resolveAssets("auto"); err != nil || (assets.live() && strings.HasPrefix(assets.dir, builtFrom)) {
		t.Errorf("a checkout that went: %+v, %v", assets, err)
	}
	embedded := compiledAssets()
	builtins, problems := embedded.builtins()
	if len(problems) != 0 || len(builtins) < 20 {
		t.Fatalf("compiled-in plugins: %d, %v", len(builtins), problems)
	}
	for _, current := range builtins {
		if current.Directory != "" || current.Files == nil || current.Web == nil || current.Web.Script == "" {
			t.Errorf("%s = %+v", current.Name, current)
		}
		code, style := embedded.versionsOf(current)
		again, _ := embedded.versionsOf(current)
		if code == "" || style == "" || code != again {
			t.Errorf("%s: versions %q %q %q", current.Name, code, style, again)
		}
	}
}
