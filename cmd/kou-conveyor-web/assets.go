package main

import (
	"embed"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// The browser cockpit is a small plugin host (static/) and plugins: the
// page is empty until plugins build it. Its own plugins (plugins/) are
// built in like the harness's core, and a user's or a workspace's plugin of
// the same name replaces one. Both are compiled into the program; a server
// that runs from a checkout serves them from the checkout instead, and open
// pages take up what changes there at once, as they do for any plugin.

//go:embed static
var staticFiles embed.FS

//go:embed plugins
var builtinFiles embed.FS

// Values of -assets.
const (
	assetsAuto     = "auto"
	assetsEmbedded = "embedded"
)

// builtFrom is the checkout the program was built from, when its build says
// so (install.sh and make build do: -ldflags "-X main.builtFrom=<dir>"). A
// program installed elsewhere then still follows the checkout's page and
// built-in plugins, as long as the checkout is there.
var builtFrom string

// assets are where the page and the built-in plugins come from.
type assets struct {
	// dir is the directory they are read from, which holds static/ and
	// plugins/; empty for those compiled in.
	dir    string
	static fs.FS

	once     sync.Once
	compiled []plugin.Plugin
	problems []error
	versions sync.Map // name → [2]string, for plugins compiled in
}

// resolveAssets finds the assets -assets names: "embedded" for those
// compiled in, a directory that holds static/ and plugins/, or "auto" for
// the checkout the server runs from when it runs from one, else those
// compiled in. An empty setting is "embedded".
func resolveAssets(setting string) (*assets, error) {
	switch setting {
	case "", assetsEmbedded:
		return compiledAssets(), nil
	case assetsAuto:
		if dir := findCheckout(); dir != "" {
			return liveAssets(dir), nil
		}
		return compiledAssets(), nil
	}
	dir, err := filepath.Abs(setting)
	if err != nil {
		return nil, err
	}
	if !holdsAssets(dir) {
		return nil, fmt.Errorf("-assets: %s holds no static/index.html and plugins/", dir)
	}
	return liveAssets(dir), nil
}

func compiledAssets() *assets {
	static, err := fs.Sub(staticFiles, "static")
	if err != nil {
		panic(err)
	}
	return &assets{static: static}
}

func liveAssets(dir string) *assets {
	return &assets{dir: dir, static: os.DirFS(filepath.Join(dir, "static"))}
}

// findCheckout looks for cmd/kou-conveyor-web in the checkout the program
// was built from, beside the executable and in the directory above it (bin/
// of a checkout). Not in the working directory: the server is started in
// workspaces, and a workspace's files are the workspace's to run only once
// it is trusted — assets found there would be the page's own code.
func findCheckout() string {
	var bases []string
	if builtFrom != "" {
		bases = append(bases, builtFrom)
	}
	if exe, err := os.Executable(); err == nil {
		if resolved, err := filepath.EvalSymlinks(exe); err == nil {
			exe = resolved
		}
		bases = append(bases, filepath.Dir(exe), filepath.Dir(filepath.Dir(exe)))
	}
	for _, base := range bases {
		for _, dir := range []string{filepath.Join(base, "cmd", "kou-conveyor-web"), base} {
			if holdsAssets(dir) {
				return dir
			}
		}
	}
	return ""
}

func holdsAssets(dir string) bool {
	index, err := os.Stat(filepath.Join(dir, "static", "index.html"))
	if err != nil || index.IsDir() {
		return false
	}
	plugins, err := os.Stat(filepath.Join(dir, "plugins"))
	return err == nil && plugins.IsDir()
}

// live reports whether the assets are read from disk as they change.
func (a *assets) live() bool { return a.dir != "" }

func (a *assets) staticDirectory() string {
	if a.dir == "" {
		return ""
	}
	return filepath.Join(a.dir, "static")
}

func (a *assets) pluginDirectory() string {
	if a.dir == "" {
		return ""
	}
	return filepath.Join(a.dir, "plugins")
}

// builtins reads the built-in plugins: those compiled in once, those on
// disk every time, as they may have changed.
func (a *assets) builtins() ([]plugin.Plugin, []error) {
	if a.dir != "" {
		return plugin.ReadDirectory(a.pluginDirectory(), plugin.SourceBuiltin)
	}
	a.once.Do(func() {
		sub, err := fs.Sub(builtinFiles, "plugins")
		if err != nil {
			a.problems = []error{err}
			return
		}
		a.compiled, a.problems = plugin.ReadDirectoryFS(sub, plugin.SourceBuiltin)
	})
	return a.compiled, a.problems
}

// versionsOf fingerprints a plugin's code and style; those of plugins
// compiled in never change, and are taken once.
func (a *assets) versionsOf(current plugin.Plugin) (string, string) {
	if current.Directory != "" || current.Files == nil {
		return current.Versions()
	}
	if cached, ok := a.versions.Load(current.Name); ok {
		pair := cached.([2]string)
		return pair[0], pair[1]
	}
	code, style := current.Versions()
	a.versions.Store(current.Name, [2]string{code, style})
	return code, style
}

// describe says where the assets come from, for the startup banner.
func (a *assets) describe() string {
	if a.dir == "" {
		return "compiled in"
	}
	return display(a.dir) + " (live: edits show in open pages at once)"
}
