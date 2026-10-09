package main

import (
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// workspaceFiles writes files into the harness's workspace.
func workspaceFiles(t *testing.T, h *harness, files map[string]string) string {
	t.Helper()
	root := h.server.workspaces.startup().Path
	for name, content := range files {
		full := filepath.Join(root, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(full), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"}, args...)...)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func names(entries []any) []string {
	var out []string
	for _, e := range entries {
		entry := e.(map[string]any)
		name := entry["name"].(string)
		if entry["dir"] == true {
			name += "/"
		}
		if entry["ignored"] == true {
			name += " (ignored)"
		}
		if g, ok := entry["git"].(string); ok {
			name += " " + g
		}
		if entry["changed"] == true {
			name += " *"
		}
		out = append(out, name)
	}
	return out
}

func TestTreeListsFoldersFirstWithWhatGitSees(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	h := newHarness(t)
	root := workspaceFiles(t, h, map[string]string{
		"b.go": "package b\n", "a10.txt": "ten\n", "a2.txt": "two\n", "src/main.go": "package main\n",
		"build/out.bin": "x", ".gitignore": "build/\n",
	})
	git(t, root, "init", "-q")
	git(t, root, "add", ".gitignore", "b.go", "src/main.go")
	git(t, root, "commit", "-q", "-m", "first")
	os.WriteFile(filepath.Join(root, "src", "main.go"), []byte("package main // changed\n"), 0o600)
	time.Sleep(gitStatusFresh / 10)

	res, body := h.do("GET", "/api/tree", "")
	if res.StatusCode != http.StatusOK {
		t.Fatalf("tree: %d %v", res.StatusCode, body)
	}
	got := strings.Join(names(body["entries"].([]any)), ", ")
	want := "build/ (ignored), src/ *, .gitignore, a2.txt ?, a10.txt ?, b.go"
	if got != want {
		t.Fatalf("tree = %s\nwant   %s", got, want)
	}
	_, body = h.do("GET", "/api/tree?path=src", "")
	if got := strings.Join(names(body["entries"].([]any)), ", "); got != "main.go M" {
		t.Fatalf("src = %s", got)
	}
	for _, bad := range []string{"../", "..%2Fetc", "/etc/../.."} {
		if res, _ := h.do("GET", "/api/tree?path="+bad, ""); res.StatusCode == http.StatusOK {
			// A path is taken from the workspace's root, whatever it says.
			if _, again := h.do("GET", "/api/tree?path="+bad, ""); again["path"] != "." {
				t.Fatalf("%s listed %v", bad, again["path"])
			}
		}
	}
	if res, _ := h.do("GET", "/api/tree?path=nope", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("a missing folder: %d", res.StatusCode)
	}
}

func TestFileComesHighlighted(t *testing.T) {
	h := newHarness(t)
	workspaceFiles(t, h, map[string]string{
		"main.go":   "package main\n\nfunc main() { println(\"hi\") }\n",
		"blob.bin":  "\x00\x01\x02",
		"logo.png":  "\x89PNG\r\n\x1a\n",
		"notes.txt": "plain words\n",
	})
	res, body := h.do("GET", "/api/file?path=main.go", "")
	if res.StatusCode != http.StatusOK || body["language"] != "Go" || body["complete"] != true || !strings.Contains(body["text"].(string), "println") {
		t.Fatalf("file: %d %v", res.StatusCode, body)
	}
	if runs := body["runs"].([]any); len(runs) < 10 || len(runs)%2 != 0 {
		t.Fatalf("runs = %v", runs)
	}
	if classes := body["classes"].([]any); classes[0] != "" || len(classes) < 20 {
		t.Fatalf("classes = %v", classes)
	}
	// The whole highlighting, by version.
	version := body["version"].(string)
	if res, tokens := h.do("GET", "/api/file?path=main.go&tokens=1&version="+version, ""); res.StatusCode != http.StatusOK || tokens["complete"] != true {
		t.Fatalf("tokens: %d %v", res.StatusCode, tokens)
	}
	if res, _ := h.do("GET", "/api/file?path=main.go&tokens=1&version=old", ""); res.StatusCode != http.StatusConflict {
		t.Fatalf("an old version: %d", res.StatusCode)
	}
	if _, body := h.do("GET", "/api/file?path=blob.bin", ""); body["binary"] != true || body["text"] != nil {
		t.Fatalf("binary = %v", body)
	}
	if _, body := h.do("GET", "/api/file?path=logo.png", ""); body["image"] != "image/png" {
		t.Fatalf("picture = %v", body)
	}
	if res, _ := h.do("GET", "/api/raw?path=logo.png", ""); res.StatusCode != http.StatusOK || res.Header.Get("Content-Type") != "image/png" {
		t.Fatalf("raw picture: %d %s", res.StatusCode, res.Header.Get("Content-Type"))
	}
	if res, _ := h.do("GET", "/api/raw?path=notes.txt", ""); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("raw text: %d", res.StatusCode)
	}
	if _, body := h.do("GET", "/api/file?path=notes.txt", ""); body["language"] != "" || body["text"] != "plain words\n" {
		t.Fatalf("plain = %v", body)
	}
}

func TestFileStaysInTheWorkspace(t *testing.T) {
	h := newHarness(t)
	root := workspaceFiles(t, h, map[string]string{"inside.txt": "in\n"})
	outside := filepath.Join(t.TempDir(), "secret.txt")
	os.WriteFile(outside, []byte("secret\n"), 0o600)
	if err := os.Symlink(outside, filepath.Join(root, "link.txt")); err != nil {
		t.Fatal(err)
	}
	if res, body := h.do("GET", "/api/file?path=link.txt", ""); res.StatusCode == http.StatusOK {
		t.Fatalf("a link out of the workspace was followed: %v", body)
	}
	if res, _ := h.do("GET", "/api/file?path=../../etc/hosts", ""); res.StatusCode == http.StatusOK {
		// The path is the workspace's etc/hosts, which is not there.
		t.Fatal("a path out of the workspace was read")
	}
	_, body := h.do("GET", "/api/tree", "")
	for _, e := range body["entries"].([]any) {
		entry := e.(map[string]any)
		if entry["name"] == "link.txt" && (entry["link"] != true || entry["broken"] != true) {
			t.Fatalf("link = %v", entry)
		}
	}
}

func TestNaturalCompare(t *testing.T) {
	list := []string{"file10", "file2", "file1", "a", "file02b"}
	for i := range list {
		for j := i + 1; j < len(list); j++ {
			if naturalCompare(list[j], list[i]) < 0 {
				list[i], list[j] = list[j], list[i]
			}
		}
	}
	if got := strings.Join(list, " "); got != "a file1 file2 file02b file10" {
		t.Fatalf("order = %s", got)
	}
}
