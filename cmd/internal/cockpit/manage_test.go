package cockpit_test

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
)

// session runs prompts one after another on one session and returns the
// session's message IDs in order.
func session(t *testing.T, dir, id string, prompts ...string) []string {
	t.Helper()
	var ids []string
	for _, prompt := range prompts {
		tr := cockpit.NewTranscript()
		job := start(t, t.Context(), dir, id, prompt)
		drain(t, job, tr)
		if err := job.Err(); err != nil {
			t.Fatalf("%q: %v", prompt, err)
		}
		for _, e := range tr.Entries {
			if e.Kind == cockpit.KindUser {
				ids = append(ids, strings.TrimPrefix(e.ID, "input:"))
			}
		}
	}
	return ids
}

func prompts(t *testing.T, dir, id string) []string {
	t.Helper()
	tr, err := cockpit.LoadSession(dir, id)
	if err != nil {
		t.Fatal(err)
	}
	var texts []string
	for _, e := range tr.Entries {
		switch e.Kind {
		case cockpit.KindUser, cockpit.KindAssistant:
			texts = append(texts, e.Text)
		}
	}
	return texts
}

func TestBranchSessionBeforeAPrompt(t *testing.T) {
	dir := t.TempDir()
	ids := session(t, dir, "parent", "first question", "second question")

	branch, err := cockpit.BranchSession(t.Context(), dir, "parent", ids[1])
	if err != nil {
		t.Fatal(err)
	}
	if branch.SessionID == "" || branch.Prompt != "second question" {
		t.Fatalf("branch = %+v", branch)
	}
	if got := prompts(t, dir, branch.SessionID); strings.Join(got, "|") != "first question|echo: first question" {
		t.Fatalf("branch history = %q", got)
	}
	// The branch is a session like any other: it runs and branches again.
	session(t, dir, branch.SessionID, "another second question")
	if got := prompts(t, dir, branch.SessionID); len(got) != 4 || got[2] != "another second question" {
		t.Fatalf("continued branch = %q", got)
	}
	if again, err := cockpit.BranchSession(t.Context(), dir, branch.SessionID, ""); err != nil || len(prompts(t, dir, again.SessionID)) != 4 {
		t.Fatalf("branch of a branch = %+v, %v", again, err)
	}
	list, err := cockpit.ListSessions(dir)
	if err != nil || len(list) != 3 {
		t.Fatalf("sessions = %+v, %v", list, err)
	}
	for _, info := range list {
		if info.ID == branch.SessionID && info.Title != "first question · branch" {
			t.Errorf("branch title = %q", info.Title)
		}
	}
}

func TestBranchSessionBeforeTheFirstPromptStartsOver(t *testing.T) {
	dir := t.TempDir()
	ids := session(t, dir, "parent", "only question")
	branch, err := cockpit.BranchSession(t.Context(), dir, "parent", ids[0])
	if err != nil || branch.SessionID != "" || branch.Prompt != "only question" {
		t.Fatalf("branch = %+v, %v", branch, err)
	}
	if _, err := cockpit.BranchSession(t.Context(), dir, "parent", "8a4f7c1e-2f7c-4e0e-9d8a-000000000000"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("unknown prompt: %v", err)
	}
}

func TestBranchingARunningSessionIsRefused(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("no cross-process session locks on windows")
	}
	dir := t.TempDir()
	session(t, dir, "parent", "question")
	unlock, err := cockpit.LockSession(dir, "parent")
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	if _, err := cockpit.BranchSession(t.Context(), dir, "parent", ""); !errors.Is(err, cockpit.ErrBranchRunning) {
		t.Fatalf("err = %v", err)
	}
	if err := cockpit.DeleteSession(dir, "parent"); !errors.Is(err, cockpit.ErrSessionBusy) {
		t.Fatalf("deleted a running session: %v", err)
	}
}

func TestRenamePinAndDelete(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "older", "older question")
	time.Sleep(10 * time.Millisecond)
	session(t, dir, "newer", "newer question")

	if err := cockpit.RenameSession(dir, "older", "  Release\nchecklist "); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.PinSession(dir, "older", true); err != nil {
		t.Fatal(err)
	}
	list, _ := cockpit.ListSessions(dir)
	if len(list) != 2 || list[0].ID != "older" || !list[0].Pinned || list[0].Title != "Release checklist" {
		t.Fatalf("list = %+v", list)
	}
	tr, _ := cockpit.LoadSession(dir, "older")
	if title := cockpit.SessionTitle(dir, "older", tr); title != "Release checklist" {
		t.Fatalf("title = %q", title)
	}
	if err := cockpit.RenameSession(dir, "older", ""); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.PinSession(dir, "older", false); err != nil {
		t.Fatal(err)
	}
	list, _ = cockpit.ListSessions(dir)
	if list[0].ID != "newer" || list[1].Title != "older question" {
		t.Fatalf("list after reset = %+v", list)
	}
	if _, err := os.Stat(filepath.Join(dir, ".meta", "older.json")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("empty metadata was kept: %v", err)
	}

	if err := cockpit.RenameSession(dir, "missing", "x"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("renamed a missing session: %v", err)
	}
	cockpit.PinSession(dir, "newer", true)
	output := filepath.Join(dir, "operations", "newer", "operation-1")
	os.MkdirAll(output, 0o700)
	os.WriteFile(filepath.Join(output, "stdout"), []byte("tool output"), 0o600)
	if err := cockpit.DeleteSession(dir, "newer"); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "operations", "newer")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("tool output survived its session: %v", err)
	}
	list, _ = cockpit.ListSessions(dir)
	if len(list) != 1 || list[0].ID != "older" {
		t.Fatalf("list after delete = %+v", list)
	}
	if _, err := os.Stat(filepath.Join(dir, ".meta", "newer.json")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("metadata survived its session")
	}
	if err := cockpit.DeleteSession(dir, "newer"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("second delete: %v", err)
	}
}

func TestFindSessionByPrefix(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "abc-123", "one")
	session(t, dir, "abd-456", "two")
	for prefix, want := range map[string]string{"abc": "abc-123", "abd-456": "abd-456"} {
		if got, err := cockpit.FindSession(dir, prefix); err != nil || got != want {
			t.Errorf("FindSession(%q) = %q, %v", prefix, got, err)
		}
	}
	if _, err := cockpit.FindSession(dir, "ab"); err == nil || !strings.Contains(err.Error(), "2 sessions") {
		t.Errorf("ambiguous prefix: %v", err)
	}
	if _, err := cockpit.FindSession(dir, "zzz"); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("unknown prefix: %v", err)
	}
}

func TestInterruptedAndExport(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "done", "use a tool please")
	tr, _ := cockpit.LoadSession(dir, "done")
	if tr.Interrupted() {
		t.Fatal("a finished run reads as interrupted")
	}
	doc := cockpit.ExportMarkdown("Tool run", "done", tr, time.Date(2026, 9, 23, 12, 0, 0, 0, time.UTC))
	for _, want := range []string{"# Tool run\n", "## 1. You", "use a tool please", "**Bash** `echo hi` — done", "```text\nhi\n```", "**Agent**", "echo: use a tool please"} {
		if !strings.Contains(doc, want) {
			t.Errorf("export lacks %q:\n%s", want, doc)
		}
	}
	if name := cockpit.ExportName("Fix the `flaky` test, then ship!", "done"); name != "fix-the-flaky-test-then-ship.md" {
		t.Errorf("name = %q", name)
	}
	if name := cockpit.ExportName("Привет", "0123456789"); name != "session-01234567.md" {
		t.Errorf("name = %q", name)
	}

	job := start(t, t.Context(), dir, "stopped", "wait for me")
	<-job.Lines()
	job.Cancel()
	for range job.Lines() {
	}
	tr, _ = cockpit.LoadSession(dir, "stopped")
	if !tr.Interrupted() {
		t.Fatal("an unanswered prompt does not read as interrupted")
	}
}

func TestStartAppliesSavedSettings(t *testing.T) {
	dir := t.TempDir()
	settings := filepath.Join(t.TempDir(), "settings.json")
	t.Setenv("KOU_CONVEYOR_LLM_BASE_URL", "https://leak.example.com")
	answer := func(o cockpit.Options) string {
		t.Helper()
		o.Runner, o.Workspace, o.SessionDir = cockpittest.Runner(t), t.TempDir(), dir
		job, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: newID(), Prompt: "print-env"})
		if err != nil {
			t.Fatal(err)
		}
		tr := cockpit.NewTranscript()
		drain(t, job, tr)
		return tr.Entries[len(tr.Entries)-1].Text
	}
	if got := answer(cockpit.Options{SettingsFile: settings}); !strings.Contains(got, "base_url=https://leak.example.com") {
		t.Fatalf("without settings: %s", got)
	}
	if err := cockpit.SaveSettings(settings, cockpit.Settings{API: cockpit.APIMessages, APIKey: "sk-ant-test", Model: "claude-opus-5"}); err != nil {
		t.Fatal(err)
	}
	if got := answer(cockpit.Options{SettingsFile: settings}); got != "provider=anthropic base_url= api_key=sk-ant-test model=claude-opus-5" {
		t.Fatalf("with settings: %s", got)
	}
	if got := answer(cockpit.Options{SettingsFile: settings, Provider: "ollama"}); !strings.HasPrefix(got, "provider=ollama base_url=https://leak.example.com") {
		t.Fatalf("a provider flag wins: %s", got)
	}
	os.WriteFile(settings, []byte("{broken"), 0o600)
	if _, err := cockpit.Start(t.Context(), cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: dir, SettingsFile: settings},
		cockpit.Request{SessionID: "s", MessageID: newID(), Prompt: "hi"}); err == nil {
		t.Fatal("started with unreadable settings")
	}
}

func newID() string { return uuid.New().String() }

func TestMetadataFromDiskIsCleaned(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "s", "question")
	os.MkdirAll(filepath.Join(dir, ".meta"), 0o700)
	os.WriteFile(filepath.Join(dir, ".meta", "s.json"), []byte(`{"title":"\u001b]0;owned\u0007 title\u001b[2J"}`), 0o600)
	if title := cockpit.LoadMeta(dir, "s").Title; strings.ContainsAny(title, "\x1b\x07") || title != "title" {
		t.Fatalf("title = %q", title)
	}
	if list, _ := cockpit.ListSessions(dir); strings.ContainsAny(list[0].Title, "\x1b\x07") {
		t.Fatalf("listed title = %q", list[0].Title)
	}
}

func TestRunningADeletedSessionFails(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "s", "question")
	if err := cockpit.DeleteSession(dir, "s"); err != nil {
		t.Fatal(err)
	}
	o := cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: dir}
	if _, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: newID(), Prompt: "again", Resume: true}); !errors.Is(err, cockpit.ErrSessionGone) {
		t.Fatalf("err = %v", err)
	}
	if _, err := os.Stat(cockpit.SessionPath(dir, "s")); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("the deleted session came back")
	}
	// A new session under a chosen ID is not a resume.
	job, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: newID(), Prompt: "fresh start"})
	if err != nil {
		t.Fatal(err)
	}
	drain(t, job, cockpit.NewTranscript())
}
