package main

import (
	"context"
	"encoding/json/v2"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func (h *harness) workspaceIDs() []string {
	h.Helper()
	res, err := http.Get(h.http.URL + "/api/workspaces")
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	var list []workspaceView
	if err := jsonRead(res, &list); err != nil {
		h.Fatal(err)
	}
	var ids []string
	for _, ws := range list {
		ids = append(ids, ws.ID)
	}
	return ids
}

func (h *harness) addWorkspace(path string) (int, map[string]any) {
	h.Helper()
	res, body := h.do("POST", "/api/workspaces", fmt.Sprintf(`{"path":%q}`, path))
	return res.StatusCode, body
}

func sessionIDs(t *testing.T, h *harness, path string) []string {
	t.Helper()
	res, err := http.Get(h.http.URL + path)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var list []sessionSummary
	if err := jsonRead(res, &list); err != nil {
		t.Fatal(err)
	}
	var ids []string
	for _, s := range list {
		ids = append(ids, s.ID)
	}
	return ids
}

func TestWorkspacesKeepTheirOwnSessions(t *testing.T) {
	h := newHarness(t)
	startup := h.workspaceIDs()[0]
	folder := t.TempDir()
	status, added := h.addWorkspace(folder)
	if status != http.StatusCreated || added["path"] != folder || added["missing"] == true {
		t.Fatalf("add: %d %v", status, added)
	}
	other := added["id"].(string)
	// The same folder, also through a link, is listed once.
	link := filepath.Join(t.TempDir(), "link")
	os.Symlink(folder, link)
	for _, path := range []string{folder, folder + "/", link} {
		if status, again := h.addWorkspace(path); status != http.StatusOK || again["id"] != other {
			t.Fatalf("add %s again: %d %v", path, status, again)
		}
	}
	if ids := h.workspaceIDs(); !slices.Equal(ids, []string{startup, other}) {
		t.Fatalf("workspaces = %v", ids)
	}

	started := h.start(`{"prompt":"hello there","session_id":"session-1"}`) // legacy route: the startup workspace
	h.stream(started["run_id"].(string), "")
	res, body := h.do("POST", "/api/w/"+other+"/runs", `{"prompt":"over here","session_id":"session-2"}`)
	if res.StatusCode != http.StatusAccepted || body["workspace"] != other {
		t.Fatalf("run in the added workspace: %d %v", res.StatusCode, body)
	}
	h.stream(body["run_id"].(string), "")
	if _, err := os.Stat(cockpit.SessionPath(filepath.Join(folder, ".harness", "sessions"), "session-2")); err != nil {
		t.Fatalf("the session is not in the workspace folder: %v", err)
	}
	if got := sessionIDs(t, h, "/api/w/"+startup+"/sessions"); !slices.Equal(got, []string{"session-1"}) {
		t.Fatalf("startup sessions = %v", got)
	}
	if got := sessionIDs(t, h, "/api/w/"+other+"/sessions"); !slices.Equal(got, []string{"session-2"}) {
		t.Fatalf("added workspace sessions = %v", got)
	}
	if res, _ := h.do("GET", "/api/w/"+other+"/sessions/session-1", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("a session leaked across workspaces: %d", res.StatusCode)
	}
	if _, session := h.do("GET", "/api/w/"+other+"/sessions/session-2", ""); session["workspace"] != other || session["title"] != "over here" {
		t.Fatalf("session = %v", session)
	}

	// One session ID can run in two workspaces at once.
	first := h.start(`{"prompt":"wait here","session_id":"shared"}`)
	res, second := h.do("POST", "/api/w/"+other+"/runs", `{"prompt":"wait there","session_id":"shared"}`)
	if res.StatusCode != http.StatusAccepted {
		t.Fatalf("the same session ID in another workspace: %d %v", res.StatusCode, second)
	}
	if res, _ := h.do("DELETE", "/api/workspaces/"+other, ""); res.StatusCode != http.StatusConflict {
		t.Fatalf("removed a workspace with a run in progress: %d", res.StatusCode)
	}
	for _, run := range []string{first["run_id"].(string), second["run_id"].(string)} {
		h.do("POST", "/api/runs/"+run+"/cancel", "")
		h.stream(run, "")
	}

	for id, want := range map[string]int{other: http.StatusOK, startup: http.StatusConflict, "missing-000000": http.StatusNotFound} {
		if res, _ := h.do("DELETE", "/api/workspaces/"+id, ""); res.StatusCode != want {
			t.Fatalf("remove %s: %d, want %d", id, res.StatusCode, want)
		}
	}
	if ids := h.workspaceIDs(); !slices.Equal(ids, []string{startup}) {
		t.Fatalf("workspaces after removal = %v", ids)
	}
	if res, _ := h.do("GET", "/api/w/"+other+"/sessions", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("a removed workspace still answers: %d", res.StatusCode)
	}
	if _, err := os.Stat(folder); err != nil {
		t.Fatal("removing a workspace touched its folder")
	}
}

func TestAddedWorkspacesPersist(t *testing.T) {
	h := newHarness(t)
	folder := t.TempDir()
	if status, _ := h.addWorkspace(folder); status != http.StatusCreated {
		t.Fatalf("add: %d", status)
	}
	// A later server with the same settings directory offers it again, and a
	// folder that has gone away stays listed as missing until it is removed.
	gone := t.TempDir()
	h.addWorkspace(gone)
	os.Remove(gone)
	again := newServer(context.Background(), h.server.opt, &net.TCPAddr{IP: net.IPv4(127, 0, 0, 1), Port: 8080})
	settings, _ := again.settings()
	var paths []string
	var missing []bool
	for _, ws := range again.workspaces.all() {
		view := again.viewOf(ws, settings)
		paths, missing = append(paths, view.Path), append(missing, view.Missing)
	}
	if len(paths) != 3 || paths[1] != folder || paths[2] != gone || !slices.Equal(missing, []bool{false, false, true}) {
		t.Fatalf("workspaces = %v, missing %v", paths, missing)
	}
	id := again.workspaces.all()[2].ID
	res, body := h.do("POST", "/api/w/"+id+"/runs", `{"prompt":"hello"}`)
	if res.StatusCode != http.StatusConflict || !strings.Contains(fmt.Sprint(body["error"]), "no longer exists") {
		t.Fatalf("run in a missing folder: %d %v", res.StatusCode, body)
	}
}

func TestAddWorkspaceNeedsAFolder(t *testing.T) {
	h := newHarness(t)
	file := filepath.Join(t.TempDir(), "notes.txt")
	os.WriteFile(file, nil, 0o600)
	for _, path := range []string{"", "relative/path", file, filepath.Join(t.TempDir(), "absent"), "/"} {
		if status, body := h.addWorkspace(path); status != http.StatusBadRequest {
			t.Errorf("%q: %d %v", path, status, body)
		}
	}
	home, err := os.UserHomeDir()
	if err != nil {
		t.Skip("no home directory")
	}
	if status, body := h.addWorkspace("~"); status != http.StatusCreated || body["path"] != home || body["display"] != "~" {
		t.Fatalf("~: %d %v", status, body)
	}
}

func TestFolderCompletion(t *testing.T) {
	root := t.TempDir()
	for _, dir := range []string{"alpha", "alps", "beta", ".hidden"} {
		os.Mkdir(filepath.Join(root, dir), 0o700)
	}
	os.WriteFile(filepath.Join(root, "almanac.txt"), nil, 0o600)
	os.Symlink(filepath.Join(root, "beta"), filepath.Join(root, "also-beta"))
	for prefix, want := range map[string][]string{
		root + "/al":  {root + "/alpha", root + "/alps", root + "/also-beta"},
		root + "/":    {root + "/alpha", root + "/alps", root + "/also-beta", root + "/beta"},
		root + "/.h":  {root + "/.hidden"},
		root + "/zzz": {},
		"relative":    {},
	} {
		if got := completeFolders(prefix, 40); !slices.Equal(got, want) {
			t.Errorf("%q: %v, want %v", prefix, got, want)
		}
	}
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		for _, typed := range []string{"~", "~/"} {
			for _, got := range completeFolders(typed, 5) {
				// Folders inside the home directory, still written with ~.
				rest, ok := strings.CutPrefix(got, "~/")
				if info, err := os.Stat(filepath.Join(home, rest)); !ok || strings.Contains(rest, "/") || err != nil || !info.IsDir() {
					t.Errorf("%s completes to %q", typed, got)
				}
			}
		}
	}
	h := newHarness(t)
	res, err := http.Get(h.http.URL + "/api/folders?prefix=" + root + "/be")
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var got []string
	if err := jsonRead(res, &got); err != nil || !slices.Equal(got, []string{root + "/beta"}) {
		t.Fatalf("folders = %v, %v", got, err)
	}
}

func jsonRead(res *http.Response, v any) error { return json.UnmarshalRead(res.Body, v) }
