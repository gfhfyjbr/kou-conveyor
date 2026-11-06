package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// This opt-in fixture runs its own main in an existing generic Electron runtime,
// with temporary settings and profile paths, not a user's browser application.
func TestCompactionBrowserFixture(t *testing.T) {
	browser := os.Getenv("KOU_COMPACTION_ELECTRON")
	out := os.Getenv("KOU_COMPACTION_BROWSER_OUT")
	if browser == "" || out == "" {
		t.Skip("set KOU_COMPACTION_ELECTRON and KOU_COMPACTION_BROWSER_OUT for isolated browser verification")
	}
	node, err := exec.LookPath("node")
	if err != nil {
		t.Fatal(err)
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	h := newHarness(t)
	saved := cockpit.Settings{API: cockpit.APIGateway, Protocol: cockpit.APIMessages, Model: "fixture-model"}
	if err := cockpit.SaveSettings(h.server.opt.SettingsFile, saved); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	mux.Handle("/", h.server.handler())
	mux.HandleFunc("GET /fixture/compaction", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write([]byte(compactionBrowserHTML))
	})
	fixture := httptest.NewServer(mux)
	defer fixture.Close()
	for _, path := range []string{"/kernel/dom.js", "/api/plugins/connection/files/web/compaction.js", "/api/plugins/connection/files/web/connection.css", "/api/plugins/theme/files/web/theme.css", "/api/plugins/sidebar/files/web/sidebar.css"} {
		res, err := http.Get(fixture.URL + path)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		if res.StatusCode != http.StatusOK {
			t.Fatalf("fixture asset %s: %d", path, res.StatusCode)
		}
	}
	cmd := exec.CommandContext(t.Context(), node, "testdata/compaction-browser.mjs", browser, fixture.URL, out, home)
	output, err := cmd.CombinedOutput()
	t.Logf("browser fixture: %s", output)
	if err != nil {
		t.Fatalf("isolated browser: %v", err)
	}
	got, err := cockpit.LoadSettings(h.server.opt.SettingsFile)
	saved.AutoCompactPercent = 62.5
	if err != nil || got != saved {
		t.Fatalf("browser changed connection or did not persist percent: %+v, %v; want %+v", got, err, saved)
	}
}

const compactionBrowserHTML = `<!doctype html>
<html lang="en" data-theme="dark"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Isolated automatic compaction fixture</title>
<link rel="icon" href="data:,">
<link rel="stylesheet" href="/api/plugins/theme/files/web/theme.css">
<link rel="stylesheet" href="/api/plugins/sidebar/files/web/sidebar.css">
<link rel="stylesheet" href="/api/plugins/connection/files/web/connection.css">
<style>body{overflow:auto;padding:16px}main{max-width:420px}.set-section{background:var(--bg-2)}</style>
</head><body><main class="settings-sections"><section class="ins set-section" data-section="compaction"><h2>Automatic compaction</h2><div id="fixture"></div></section></main>
<script type="module">
import { h } from '/kernel/dom.js';
import { compactionSettings } from '/api/plugins/connection/files/web/compaction.js';
window.fixtureFailures = { load: false, save: false };
const api = async (path, options = {}) => {
  if (!options.method && fixtureFailures.load) throw new Error('fixture load unavailable');
  if (options.method === 'PUT' && fixtureFailures.save) throw new Error('fixture save unavailable');
  const response = await fetch(path, { method: options.method || 'GET', headers: { 'Content-Type': 'application/json' }, body: options.body ? JSON.stringify(options.body) : undefined });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || String(response.status));
  return data;
};
window.fixtureReload = () => document.querySelector('#fixture').replaceChildren(compactionSettings({ h, api }));
fixtureReload();
</script></body></html>`
