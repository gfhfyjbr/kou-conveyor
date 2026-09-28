package cockpit_test

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

// linkWorkspace is a workspace with a file and a folder to link.
func linkWorkspace(t *testing.T) string {
	t.Helper()
	workspace := t.TempDir()
	if err := os.MkdirAll(filepath.Join(workspace, "cmd"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(workspace, "cmd", "main.go"), []byte("package main\n\nfunc main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	return workspace
}

func startIn(t *testing.T, workspace, sessions, id, prompt string) *cockpit.Job {
	t.Helper()
	job, err := cockpit.Start(t.Context(), cockpit.Options{
		Runner: cockpittest.Runner(t), Workspace: workspace, SessionDir: sessions, Heartbeat: time.Minute,
	}, cockpit.Request{SessionID: id, MessageID: uuid.New().String(), Prompt: prompt, Thinking: "high"})
	if err != nil {
		t.Fatal(err)
	}
	return job
}

func TestPromptsBringTheFilesTheyLink(t *testing.T) {
	workspace, sessions, id := linkWorkspace(t), t.TempDir(), uuid.New().String()
	tr := cockpit.NewTranscript()
	job := startIn(t, workspace, sessions, id, "what is $cmd/main.go:3 in $cmd/? not $HOME")
	drain(t, job, tr)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	prompt := tr.Entries[0]
	want := []cockpit.LinkedFileInfo{
		{Label: "$cmd/main.go:3", Path: "cmd/main.go", From: 3, To: 3, Lines: 3, Size: 29},
		{Label: "$cmd/", Path: "cmd/", Directory: true, From: 1, To: 1, Lines: 1},
	}
	if !reflect.DeepEqual(prompt.Files, want) {
		t.Fatalf("files = %+v, want %+v", prompt.Files, want)
	}
	// The session keeps what the model saw, and so does the transcript read
	// from it.
	files, err := cockpit.PromptFiles(sessions, id, prompt.ID[len("input:"):])
	if err != nil || len(files) != 2 || files[0].Content != "func main() {}" || files[1].Content != "main.go" {
		t.Fatalf("prompt files = %+v, %v", files, err)
	}
	loaded, err := cockpit.LoadSession(sessions, id)
	if err != nil || !reflect.DeepEqual(loaded.Entries[0].Files, want) {
		t.Fatalf("loaded files = %+v, %v", loaded.Entries[0].Files, err)
	}
	if _, err := cockpit.PromptFiles(sessions, id, uuid.New().String()); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("files of a prompt the session lacks: %v", err)
	}
	// The Markdown export says what the model saw of them.
	if export := cockpit.ExportMarkdown("Links", id, loaded, time.Now()); !strings.Contains(export, "\n_Linked files: `$cmd/main.go:3` (line 3 of 3), `$cmd/` (1 entry)_\n") {
		t.Fatalf("export:\n%s", export)
	}
}

func TestLinkedFilesSayWhatTheModelSaw(t *testing.T) {
	for _, test := range []struct {
		file cockpit.LinkedFileInfo
		want string
	}{
		{cockpit.LinkedFileInfo{From: 1, To: 100, Lines: 345}, "lines 1–100 of 345"},
		{cockpit.LinkedFileInfo{From: 7, To: 7, Lines: 9}, "line 7 of 9"},
		{cockpit.LinkedFileInfo{From: 1, To: 12, Lines: 12}, "all 12 lines"},
		{cockpit.LinkedFileInfo{From: 1, To: 1, Size: 3 << 30}, "line 1 · 3072.0 MB"},
		{cockpit.LinkedFileInfo{Lines: 345, Size: 10}, "345 lines · none shown"},
		{cockpit.LinkedFileInfo{}, "empty"},
		{cockpit.LinkedFileInfo{Binary: true, Size: 2048}, "binary · 2 KB"},
		{cockpit.LinkedFileInfo{Error: "no such file or directory"}, "not read: no such file or directory"},
		{cockpit.LinkedFileInfo{Directory: true, From: 1, To: 200, Lines: 250}, "200 of 250 entries"},
		{cockpit.LinkedFileInfo{Directory: true, From: 1, To: 3, Lines: 3}, "3 entries"},
		{cockpit.LinkedFileInfo{Directory: true}, "empty folder"},
	} {
		if got := test.file.Describe(); got != test.want {
			t.Errorf("%+v: %q, want %q", test.file, got, test.want)
		}
	}
}

func TestSteeredMessagesLinkFiles(t *testing.T) {
	workspace := linkWorkspace(t)
	job := startIn(t, workspace, t.TempDir(), uuid.New().String(), "steer")
	tr := cockpit.NewTranscript()
	message := uuid.New().String()
	sent := false
	for line := range job.Lines() {
		if line.Stderr {
			continue
		}
		tr.Apply([]byte(line.Text))
		if running := tr.Running(); !sent && len(running) == 1 && running[0].Tool.State == cockpit.ToolRunning {
			if err := job.Steer(message, "and $cmd/main.go"); err != nil {
				t.Fatal(err)
			}
			sent = true
		}
	}
	if e := tr.Entry("input:" + message); e == nil || !e.Forced || len(e.Files) != 1 || e.Files[0].Path != "cmd/main.go" || e.Files[0].Lines != 3 {
		t.Fatalf("forced prompt = %+v", e)
	}
}

func TestAnOlderRunnerGetsPromptsWithoutFiles(t *testing.T) {
	t.Setenv(cockpittest.NoLinks, "1")
	cockpit.ForgetRunners()
	t.Cleanup(cockpit.ForgetRunners)
	workspace, sessions, id := linkWorkspace(t), t.TempDir(), uuid.New().String()
	tr := cockpit.NewTranscript()
	job := startIn(t, workspace, sessions, id, "what is $cmd/main.go")
	drain(t, job, tr)
	// It would turn files away: the prompt goes as its text.
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	if prompt := tr.Entries[0]; prompt.Text != "what is $cmd/main.go" || prompt.Files != nil {
		t.Fatalf("prompt = %+v", prompt)
	}
}
