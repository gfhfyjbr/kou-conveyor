package main

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// workspaceFile writes a file of the harness's workspace.
func (h *harness) workspaceFile(name, content string) {
	h.Helper()
	path := filepath.Join(h.server.workspaces.startup().Path, filepath.FromSlash(name))
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		h.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		h.Fatal(err)
	}
}

func TestFilesCompleteAReference(t *testing.T) {
	h := newHarness(t)
	h.workspaceFile("cmd/main.go", "package main\n")
	h.workspaceFile("README.md", "# x\n")
	status, _, body := h.fetch("/api/files?q=main")
	var got struct {
		Files []cockpit.FileMatch `json:"files"`
	}
	if err := json.Unmarshal(body, &got); status != http.StatusOK || err != nil {
		t.Fatalf("status %d, %s", status, body)
	}
	if !reflect.DeepEqual(got.Files, []cockpit.FileMatch{{Path: "cmd/main.go"}}) {
		t.Fatalf("files = %s", body)
	}
	// Nothing typed: the top level, folders first; and the limit.
	ws := h.server.workspaces.startup().ID
	status, _, body = h.fetch("/api/w/" + ws + "/files?q=&limit=1")
	if status != http.StatusOK || string(body) != `{"files":[{"path":"cmd/","directory":true}]}` {
		t.Fatalf("status %d, %s", status, body)
	}
	if status, _, body := h.fetch("/api/files?q=zzz"); status != http.StatusOK || string(body) != `{"files":[]}` {
		t.Fatalf("status %d, %s", status, body)
	}
}

func TestLinksSayWhatATextLinks(t *testing.T) {
	h := newHarness(t)
	h.workspaceFile("cmd/main.go", "package main\n")
	res, err := http.Post(h.http.URL+"/api/links", "application/json", strings.NewReader(`{"text":"see $cmd/main.go:2-3 and $cmd/, not $HOME"}`))
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var got struct {
		Links []linkView `json:"links"`
	}
	if err := json.UnmarshalRead(res.Body, &got); res.StatusCode != http.StatusOK || err != nil {
		t.Fatalf("status %d, %v", res.StatusCode, err)
	}
	want := []linkView{
		{Label: "$cmd/main.go:2-3", Path: "cmd/main.go", Size: 13, StartLine: 2, EndLine: 3},
		{Label: "$cmd/", Path: "cmd/", Directory: true},
	}
	if !reflect.DeepEqual(got.Links, want) {
		t.Fatalf("links = %+v, want %+v", got.Links, want)
	}
	if res, _ := h.do("POST", "/api/links", `{"text":"x"}`, "Content-Type", "text/plain"); res.StatusCode != http.StatusUnsupportedMediaType {
		t.Fatalf("a text/plain body: %d", res.StatusCode)
	}
	// A page of another site cannot ask.
	if res, _ := h.do("POST", "/api/links", `{"text":"x"}`, "Sec-Fetch-Site", "cross-site"); res.StatusCode != http.StatusForbidden {
		t.Fatalf("a cross-site request: %d", res.StatusCode)
	}
}

func TestRunsBringTheFilesTheyLink(t *testing.T) {
	h := newHarness(t)
	h.workspaceFile("cmd/main.go", "package main\n\nfunc main() {}\n")
	message := "4a7c1d52-0b3e-4f5a-9c8d-1e2f3a4b5c6e"
	started := h.start(fmt.Sprintf(`{"prompt":"what is $cmd/main.go?","message_id":%q}`, message))
	events, _ := h.stream(started["run_id"].(string), "")
	prompt := events.entries()["input:"+message]
	if prompt == nil || len(prompt.Files) != 1 || prompt.Files[0].Label != "$cmd/main.go" || prompt.Files[0].Lines != 3 || prompt.Files[0].To != 3 {
		t.Fatalf("prompt = %+v", prompt)
	}
	// What the model saw of it.
	session := started["session_id"].(string)
	status, _, body := h.fetch("/api/sessions/" + session + "/files/" + message)
	if status != http.StatusOK || !strings.Contains(string(body), `"label":"$cmd/main.go","path":"cmd/main.go","from":1,"to":3,"lines":3,"size":29,"content":"package main\n\nfunc main() {}"`) {
		t.Fatalf("status %d, %s", status, body)
	}
	for _, path := range []string{
		"/api/sessions/" + session + "/files/4a7c1d52-0b3e-4f5a-9c8d-1e2f3a4b5c6d",
		"/api/sessions/" + session + "/files/not-a-uuid",
		"/api/sessions/../files/" + message,
	} {
		if status, _, body := h.fetch(path); status != http.StatusNotFound {
			t.Errorf("%s: %d %s", path, status, body)
		}
	}
}
