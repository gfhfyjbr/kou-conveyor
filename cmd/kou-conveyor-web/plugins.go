package main

import (
	"bytes"
	"encoding/json/v2"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// The browser cockpit loads the web part of every active plugin of the
// workspace in view: its module and its style sheet, served from the
// plugin's files. The page itself is made of plugins, so the listing holds
// the cockpit's own built-in plugins too. Plugins that are not active, a
// workspace's while the workspace is not trusted among them, serve nothing.
//
// A plugin's files are served under a version, a fingerprint of the files:
// a browser keeps a module it imported for as long as the page lives, so a
// changed plugin has to come from new addresses — its modules' own imports
// included — to be loaded anew. The listing names the current addresses.

type pluginView struct {
	Name         string           `json:"name"`
	Version      string           `json:"version,omitzero"`
	Description  string           `json:"description,omitzero"`
	Source       plugin.Source    `json:"source"`
	Directory    string           `json:"directory,omitzero"`
	Active       bool             `json:"active"`
	Reason       string           `json:"reason,omitzero"`
	Tools        []pluginToolView `json:"tools"`
	Commands     []plugin.Command `json:"commands"`
	Skills       bool             `json:"skills,omitzero"`
	Instructions bool             `json:"instructions,omitzero"`
	Script       string           `json:"script,omitzero"`
	Style        string           `json:"style,omitzero"`
	// CodeVersion and StyleVersion fingerprint the plugin's files: a page
	// loads the plugin again when the first changes, and swaps its style
	// sheet when only the second does.
	CodeVersion  string   `json:"code_version,omitzero"`
	StyleVersion string   `json:"style_version,omitzero"`
	After        []string `json:"after,omitzero"`
	// Live is set for a plugin read from disk, whose changes show at once.
	Live bool `json:"live,omitzero"`
}

type pluginToolView struct {
	Name        string `json:"name"`
	Description string `json:"description"`
}

// plugins finds the plugins of a workspace, the cockpit's own among them.
func (s *server) plugins(ws *workspace) plugin.Found {
	builtins, problems := s.assets.builtins()
	found := cockpit.PluginsWith(s.opt.SettingsFile, ws.Path, builtins)
	found.Errors = append(append([]error(nil), problems...), found.Errors...)
	return found
}

// pluginSources are the files and directories a workspace's plugins come
// from, to watch.
func (s *server) pluginSources(ws *workspace) []string {
	options := cockpit.PluginOptions(s.opt.SettingsFile, ws.Path, nil)
	sources := plugin.Sources(options)
	if dir := s.assets.pluginDirectory(); dir != "" {
		sources = append(sources, dir)
	}
	return sources
}

func (s *server) pluginsOf(ws *workspace) map[string]any {
	found := s.plugins(ws)
	views := make([]pluginView, 0, len(found.Plugins))
	workspacePlugins := 0
	for _, current := range found.Plugins {
		if current.Source == plugin.SourceWorkspace {
			workspacePlugins++
		}
		view := pluginView{
			Name: current.Name, Version: current.Version, Description: current.Description, Source: current.Source,
			Directory: current.Directory, Active: current.Active, Reason: current.Reason,
			Tools: []pluginToolView{}, Commands: current.Commands, Skills: current.Skills != "", Instructions: current.Instructions != "",
			Live: current.Directory != "",
		}
		if view.Commands == nil {
			view.Commands = []plugin.Command{}
		}
		for _, definition := range current.Tools {
			view.Tools = append(view.Tools, pluginToolView{Name: definition.Name, Description: definition.Description})
		}
		if current.Active && (current.Web != nil || len(current.Commands) > 0) {
			view.CodeVersion, view.StyleVersion = s.assets.versionsOf(current)
		}
		if current.Active && current.Web != nil {
			view.Script = pluginFileURL(ws, current, view.CodeVersion, current.Web.Script)
			view.Style = pluginFileURL(ws, current, view.StyleVersion, current.Web.Style)
			view.After = current.Web.After
		}
		views = append(views, view)
	}
	errs := make([]string, 0, len(found.Errors))
	for _, err := range found.Errors {
		errs = append(errs, err.Error())
	}
	listing := map[string]any{
		"workspace": ws.ID, "plugins": views, "errors": errs, "trusted": found.Trusted, "workspace_plugins": workspacePlugins,
		"workspace_directory": plugin.WorkspaceDirectory(ws.Path), "user_directory": plugin.UserDirectory(cockpit.PluginDirectory(s.opt.SettingsFile)),
		"live": s.assets.live(),
		// server tells pages this run of the server from the last: one that
		// sees another after reconnecting knows it restarted.
		"server": map[string]any{"instance": s.instance, "rebuild": s.rebuild != nil, "started_at": s.started, "build": buildLabel()},
	}
	if dir := s.assets.pluginDirectory(); dir != "" {
		listing["builtin_directory"] = dir
	}
	return listing
}

// pluginFileURL is where the browser fetches a file of a plugin, under the
// version of the files, so a changed file comes from a new address.
func pluginFileURL(ws *workspace, current plugin.Plugin, version, relative string) string {
	if relative == "" {
		return ""
	}
	escaped := make([]string, 0)
	for _, part := range strings.Split(path.Clean(filepath.ToSlash(relative)), "/") {
		escaped = append(escaped, url.PathEscape(part))
	}
	if version == "" {
		version = "0"
	}
	return fmt.Sprintf("/api/w/%s/plugins/%s/v/%s/%s", url.PathEscape(ws.ID), url.PathEscape(current.Name), url.PathEscape(version), strings.Join(escaped, "/"))
}

func (s *server) handlePlugins(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	writeJSON(w, http.StatusOK, s.pluginsOf(ws))
}

// handleTrustPlugins records whether the workspace's plugins may run.
func (s *server) handleTrustPlugins(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	var body struct {
		Trusted *bool `json:"trusted"`
	}
	if err := json.UnmarshalRead(io.LimitReader(r.Body, 4<<10), &body); err != nil || body.Trusted == nil {
		writeError(w, http.StatusBadRequest, `expected {"trusted": true or false}`)
		return
	}
	if err := cockpit.TrustPlugins(s.opt.SettingsFile, ws.Path, *body.Trusted); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.watch.poke()
	writeJSON(w, http.StatusOK, s.pluginsOf(ws))
}

// handleEnablePlugin turns a plugin on or off for every workspace.
func (s *server) handleEnablePlugin(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	var body struct {
		Enabled *bool `json:"enabled"`
	}
	if err := json.UnmarshalRead(io.LimitReader(r.Body, 4<<10), &body); err != nil || body.Enabled == nil {
		writeError(w, http.StatusBadRequest, `expected {"enabled": true or false}`)
		return
	}
	if err := cockpit.DisablePlugin(s.opt.SettingsFile, r.PathValue("name"), !*body.Enabled); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.watch.poke()
	writeJSON(w, http.StatusOK, s.pluginsOf(ws))
}

// Files a plugin may serve, by extension.
var pluginFileTypes = map[string]string{
	".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8",
	".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
	".gif": "image/gif", ".webp": "image/webp", ".woff": "font/woff", ".woff2": "font/woff2", ".txt": "text/plain; charset=utf-8",
	".html": "text/plain; charset=utf-8", ".md": "text/plain; charset=utf-8",
}

// handlePluginFile serves a file of an active plugin: one of the kinds
// above, inside the plugin's files, and not hidden. The version in the
// address only tells the browser apart what changed; the file served is
// the one there now.
func (s *server) handlePluginFile(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	name, relative := r.PathValue("name"), r.PathValue("path")
	kind, allowed := pluginFileTypes[strings.ToLower(path.Ext(relative))]
	if !allowed || !fs.ValidPath(relative) || strings.HasPrefix(relative, ".") || strings.Contains(relative, "/.") {
		writeError(w, http.StatusNotFound, "not found")
		return
	}
	var owner *plugin.Plugin
	for _, current := range s.plugins(ws).Active() {
		if current.Name == name && (current.Directory != "" || current.Files != nil) {
			owner = &current
			break
		}
	}
	if owner == nil {
		writeError(w, http.StatusNotFound, "no active plugin "+name)
		return
	}
	content, modified, err := readPluginFile(*owner, relative)
	if err != nil {
		writeError(w, http.StatusNotFound, "not found")
		return
	}
	w.Header().Set("Content-Type", kind)
	w.Header().Set("Cache-Control", "no-cache")
	http.ServeContent(w, r, path.Base(relative), modified, content)
}

// readPluginFile opens a file of a plugin: on disk through a root, which
// keeps the path, symbolic links included, inside the plugin; compiled in,
// from its files.
func readPluginFile(owner plugin.Plugin, relative string) (io.ReadSeeker, time.Time, error) {
	if owner.Directory != "" {
		root, err := os.OpenRoot(owner.Directory)
		if err != nil {
			return nil, time.Time{}, err
		}
		defer root.Close()
		file, err := root.Open(filepath.FromSlash(relative))
		if err != nil {
			return nil, time.Time{}, err
		}
		defer file.Close()
		info, err := file.Stat()
		if err != nil || info.IsDir() {
			return nil, time.Time{}, fs.ErrNotExist
		}
		content, err := io.ReadAll(io.LimitReader(file, 32<<20))
		if err != nil {
			return nil, time.Time{}, err
		}
		return bytes.NewReader(content), info.ModTime(), nil
	}
	info, err := fs.Stat(owner.Files, relative)
	if err != nil || info.IsDir() {
		return nil, time.Time{}, fs.ErrNotExist
	}
	content, err := fs.ReadFile(owner.Files, relative)
	if err != nil {
		return nil, time.Time{}, err
	}
	return bytes.NewReader(content), info.ModTime(), nil
}

// buildLabel names the build that runs: the start of the fingerprint of the
// Go code it was built from, when it built itself, else the installed one.
func buildLabel() string {
	if len(sourceFingerprint) >= 7 {
		return sourceFingerprint[:7]
	}
	return "installed"
}
