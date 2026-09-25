package plugin

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"testing/fstest"
	"time"
)

func TestReadFSLoadsACompiledInPlugin(t *testing.T) {
	files := fstest.MapFS{
		"shell/plugin.json":     {Data: []byte(`{"name": "shell", "prompt": "prompt.md", "web": {"script": "web/shell.js", "style": "web/shell.css", "after": ["theme"]}}`)},
		"shell/prompt.md":       {Data: []byte("  Be brief.  ")},
		"shell/web/shell.js":    {Data: []byte("export default () => {};")},
		"shell/web/shell.css":   {Data: []byte(".shell {}")},
		"broken/plugin.json":    {Data: []byte(`{"name": "broken", "web": {"script": "web/missing.js"}}`)},
		"not-a-plugin/README":   {Data: []byte("no manifest")},
		"bad-after/plugin.json": {Data: []byte(`{"name": "bad-after", "web": {"after": ["Not A Name"]}}`)},
	}
	plugins, problems := ReadDirectoryFS(files, SourceBuiltin)
	if len(plugins) != 1 || len(problems) != 2 {
		t.Fatalf("plugins = %v, problems = %v", plugins, problems)
	}
	shell := plugins[0]
	if shell.Name != "shell" || shell.Directory != "" || shell.Files == nil || shell.Instructions != "Be brief." || shell.Web.After[0] != "theme" {
		t.Fatalf("shell = %+v", shell)
	}
	if _, err := shell.Resolve("web/shell.js"); err == nil {
		t.Fatal("a compiled-in plugin resolved a path on disk")
	}
	if info, err := shell.Stat("web/shell.js"); err != nil || info.IsDir() {
		t.Fatalf("stat = %v, %v", info, err)
	}
	if _, err := shell.Stat("../escape"); err == nil {
		t.Fatal("stat left the plugin")
	}
	joined := problems[0].Error() + problems[1].Error()
	if !strings.Contains(joined, "web/missing.js") || !strings.Contains(joined, "not a plugin name") {
		t.Fatalf("problems = %v", problems)
	}
}

func TestVersionsFollowCodeAndStyleApart(t *testing.T) {
	directory := writePlugin(t, filepath.Join(t.TempDir(), "look"), `{"name": "look", "web": {"script": "web/look.js", "style": "web/look.css"}}`,
		"web/look.js", "export default () => {};", "web/look.css", ".look {}")
	read := func() (string, string) {
		loaded, err := Read(directory, SourceUser)
		if err != nil {
			t.Fatal(err)
		}
		return loaded.Versions()
	}
	code, style := read()
	later := time.Now().Add(time.Second)
	os.WriteFile(filepath.Join(directory, "web", "look.css"), []byte(".look { color: red }"), 0o644)
	os.Chtimes(filepath.Join(directory, "web", "look.css"), later, later)
	code2, style2 := read()
	if code2 != code || style2 == style {
		t.Fatalf("a style change: %s %s → %s %s", code, style, code2, style2)
	}
	os.WriteFile(filepath.Join(directory, "web", "lib.js"), []byte("export const x = 1;"), 0o644)
	code3, style3 := read()
	if code3 == code2 || style3 != style2 {
		t.Fatalf("a new module: %s %s → %s %s", code2, style2, code3, style3)
	}

	// Compiled in, the content counts.
	files := fstest.MapFS{"plugin.json": {Data: []byte(`{"name": "x"}`)}, "a.js": {Data: []byte("1")}}
	compiled, err := ReadFS(files, "x", SourceBuiltin)
	if err != nil {
		t.Fatal(err)
	}
	first, _ := compiled.Versions()
	files["a.js"] = &fstest.MapFile{Data: []byte("2")}
	if second, _ := compiled.Versions(); second == first {
		t.Fatal("a changed compiled-in file kept its version")
	}
}

func TestFingerprintSeesChangesAndFollowsLinks(t *testing.T) {
	root := t.TempDir()
	plugins := filepath.Join(root, "plugins")
	writePlugin(t, filepath.Join(plugins, "a"), `{"name": "a"}`, "a.js", "1")
	// A plugin being worked on elsewhere, linked in.
	elsewhere := writePlugin(t, filepath.Join(root, "work", "b"), `{"name": "b"}`, "b.js", "1")
	if err := os.Symlink(elsewhere, filepath.Join(plugins, "b")); err != nil {
		t.Skip("no symbolic links:", err)
	}
	settings := filepath.Join(root, "plugins.json")
	before := Fingerprint(plugins, settings)
	if Fingerprint(plugins, settings) != before {
		t.Fatal("the fingerprint is not stable")
	}
	steps := []struct {
		name   string
		change func()
		same   bool
	}{
		{"a file of a plugin", func() { os.WriteFile(filepath.Join(plugins, "a", "a.js"), []byte("22"), 0o644) }, false},
		{"a file of a linked plugin", func() { os.WriteFile(filepath.Join(elsewhere, "b.js"), []byte("22"), 0o644) }, false},
		{"a new plugin", func() { writePlugin(t, filepath.Join(plugins, "c"), `{"name": "c"}`) }, false},
		{"plugins.json", func() { os.WriteFile(settings, []byte(`{}`), 0o644) }, false},
		{"a hidden file", func() { os.WriteFile(filepath.Join(plugins, "a", ".swap"), []byte("x"), 0o644) }, true},
		{"node_modules", func() { writePlugin(t, filepath.Join(plugins, "a", "node_modules", "dep"), `{}`) }, true},
		{"a plugin removed", func() { os.RemoveAll(filepath.Join(plugins, "c")) }, false},
	}
	for _, step := range steps {
		last := Fingerprint(plugins, settings)
		time.Sleep(5 * time.Millisecond)
		step.change()
		if now := Fingerprint(plugins, settings); (now == last) != step.same {
			t.Errorf("%s: changed %v, want %v", step.name, now != last, !step.same)
		}
	}
}

func TestWatchCallsOnChange(t *testing.T) {
	directory := t.TempDir()
	var changes atomic.Int32
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan struct{})
	go func() {
		Watch(ctx, 10*time.Millisecond, func() []string { return []string{directory} }, func() { changes.Add(1) })
		close(done)
	}()
	time.Sleep(30 * time.Millisecond)
	os.WriteFile(filepath.Join(directory, "x"), []byte("1"), 0o644)
	deadline := time.Now().Add(2 * time.Second)
	for changes.Load() == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	<-done
	if changes.Load() == 0 {
		t.Fatal("no change seen")
	}
}

func TestDiscoverTakesTheProgramsBuiltins(t *testing.T) {
	config := t.TempDir()
	builtin := Plugin{Manifest: Manifest{Name: "layout", Web: &Web{Script: "web/layout.js"}}, Source: SourceBuiltin}
	found := Discover(Options{ConfigDirectory: config, Builtins: []Plugin{builtin}})
	if len(found.Plugins) != 2 || found.Plugins[1].Name != "layout" || !found.Plugins[1].Active {
		t.Fatalf("plugins = %+v", found.Plugins)
	}
	// A user plugin of the same name replaces it.
	writePlugin(t, filepath.Join(UserDirectory(config), "layout"), `{"name": "layout", "web": {"script": "layout.js"}}`, "layout.js", "")
	found = Discover(Options{ConfigDirectory: config, Builtins: []Plugin{builtin}})
	if len(found.Plugins) != 3 || found.Plugins[1].Active || found.Plugins[1].Reason != "replaced by the user plugin of the same name" || !found.Plugins[2].Active {
		t.Fatalf("plugins = %+v", found.Plugins)
	}
	sources := Sources(Options{ConfigDirectory: config, Workspace: "/w", Builtins: []Plugin{{Manifest: Manifest{Name: "x"}, Directory: "/checkout/plugins/x"}}})
	want := []string{"/checkout/plugins/x", UserDirectory(config), SettingsPath(config), WorkspaceDirectory("/w")}
	if strings.Join(sources, " ") != strings.Join(want, " ") {
		t.Fatalf("sources = %v, want %v", sources, want)
	}
}
