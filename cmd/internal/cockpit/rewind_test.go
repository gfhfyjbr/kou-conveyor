package cockpit_test

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	harness "github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// resumable fails the test unless the session store the runner resumes
// sessions with accepts the session.
func resumable(t *testing.T, dir, id string) {
	t.Helper()
	store, err := localfile.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Resume(t.Context(), harness.ID(id)); err != nil {
		t.Fatalf("the runner cannot resume %s: %v", id, err)
	}
}

func kinds(t *testing.T, dir, id string) string {
	t.Helper()
	tr, err := cockpit.LoadSession(dir, id)
	if err != nil {
		t.Fatal(err)
	}
	var kinds []string
	for _, e := range tr.Entries {
		kinds = append(kinds, e.Kind)
	}
	return strings.Join(kinds, ",")
}

func TestRewindSessionBeforeAPrompt(t *testing.T) {
	dir := t.TempDir()
	ids := session(t, dir, "rewound", "first question", "use a tool", "third question")
	output := filepath.Join(dir, "operations", "rewound", "operation-1")
	os.MkdirAll(output, 0o700)
	os.WriteFile(filepath.Join(output, "out"), []byte("hi\n"), 0o600)

	before, err := cockpit.LoadSessionBefore(dir, "rewound", ids[1])
	if err != nil {
		t.Fatal(err)
	}
	if err := cockpit.RewindSession(dir, "rewound", ids[1]); err != nil {
		t.Fatal(err)
	}
	if got := prompts(t, dir, "rewound"); strings.Join(got, "|") != "first question|echo: first question" {
		t.Fatalf("rewound history = %q", got)
	}
	resumable(t, dir, "rewound")
	after, _ := cockpit.LoadSession(dir, "rewound")
	if len(before.Entries) != 2 || len(after.Entries) != 2 || before.Size != after.Size || before.Usage != after.Usage {
		t.Fatalf("LoadSessionBefore = %+v, the rewound session = %+v", before, after)
	}
	if _, err := os.Stat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("the output of a removed command survived: %v", err)
	}

	// The edited prompt runs where the old one was, and can be edited again.
	edited := session(t, dir, "rewound", "a tool, edited")
	if got := prompts(t, dir, "rewound"); strings.Join(got, "|") != "first question|echo: first question|a tool, edited|echo: a tool, edited" {
		t.Fatalf("history after the edited prompt = %q", got)
	}
	resumable(t, dir, "rewound")
	if err := cockpit.RewindSession(dir, "rewound", edited[0]); err != nil {
		t.Fatal(err)
	}
	resumable(t, dir, "rewound")

	// A prompt the session no longer holds leaves it alone.
	data, _ := os.ReadFile(cockpit.SessionPath(dir, "rewound"))
	for _, gone := range []string{ids[2], "not-a-prompt"} {
		if err := cockpit.RewindSession(dir, "rewound", gone); !errors.Is(err, cockpit.ErrPromptGone) {
			t.Fatalf("rewind to a missing prompt: %v", err)
		}
		if _, err := cockpit.LoadSessionBefore(dir, "rewound", gone); !errors.Is(err, cockpit.ErrPromptGone) {
			t.Fatalf("load before a missing prompt: %v", err)
		}
	}
	if again, _ := os.ReadFile(cockpit.SessionPath(dir, "rewound")); !bytes.Equal(data, again) {
		t.Fatal("a failed rewind changed the session")
	}
	if err := cockpit.RewindSession(dir, "missing", ids[0]); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("rewind of a missing session: %v", err)
	}
}

func TestRewindBeforeTheFirstPromptStartsOver(t *testing.T) {
	dir := t.TempDir()
	ids := session(t, dir, "restart", "first question", "second question")
	if list, _ := cockpit.ListSessions(dir); len(list) != 1 || list[0].Title != "first question" {
		t.Fatalf("sessions = %+v", list)
	}
	if err := cockpit.RewindSession(dir, "restart", ids[0]); err != nil {
		t.Fatal(err)
	}
	if got := kinds(t, dir, "restart"); got != "" {
		t.Fatalf("entries after rewinding to the start = %s", got)
	}
	resumable(t, dir, "restart")
	// The title was the first prompt, which is gone.
	if list, _ := cockpit.ListSessions(dir); len(list) != 1 || list[0].Title != "" {
		t.Fatalf("sessions after the rewind = %+v", list)
	}
	session(t, dir, "restart", "a better first question")
	if list, _ := cockpit.ListSessions(dir); list[0].Title != "a better first question" {
		t.Fatalf("title after the edited first prompt = %q", list[0].Title)
	}
}

// A branch holds the history it was made from as inherited history, without
// the operations of its tool calls. Rewinding into that history must keep it
// inherited, or the runner could not resume the branch.
func TestRewindIntoInheritedHistory(t *testing.T) {
	dir := t.TempDir()
	parent := session(t, dir, "parent", "use a tool", "second question", "third question")
	branch, err := cockpit.BranchSession(t.Context(), dir, "parent", parent[2])
	if err != nil {
		t.Fatal(err)
	}
	id := branch.SessionID
	own := session(t, dir, id, "branch question", "another branch question")

	if err := cockpit.RewindSession(dir, id, own[1]); err != nil {
		t.Fatal(err)
	}
	resumable(t, dir, id)
	if got := kinds(t, dir, id); got != "user,tool,assistant,user,assistant,notice,user,assistant" {
		t.Fatalf("rewound within the branch's own history: %s", got)
	}

	if err := cockpit.RewindSession(dir, id, parent[1]); err != nil {
		t.Fatal(err)
	}
	resumable(t, dir, id)
	if got := kinds(t, dir, id); got != "user,tool,assistant,notice" {
		t.Fatalf("rewound into inherited history: %s", got)
	}
	session(t, dir, id, "second question, edited")
	resumable(t, dir, id)
	if got := prompts(t, dir, id); strings.Join(got, "|") != "use a tool|echo: use a tool|second question, edited|echo: second question, edited" {
		t.Fatalf("history after the edited prompt = %q", got)
	}

	// Before the very first prompt nothing is left to inherit.
	if err := cockpit.RewindSession(dir, id, parent[0]); err != nil {
		t.Fatal(err)
	}
	resumable(t, dir, id)
	if got := kinds(t, dir, id); got != "" {
		t.Fatalf("rewound to the start of a branch: %s", got)
	}
}

func TestStartRewindsBeforeRunning(t *testing.T) {
	dir := t.TempDir()
	ids := session(t, dir, "edited", "first question", "second question")
	o := cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: dir, Heartbeat: time.Minute}
	job, err := cockpit.Start(t.Context(), o, cockpit.Request{
		SessionID: "edited", MessageID: uuid.New().String(), Prompt: "second question, edited", Rewind: ids[1],
	})
	if err != nil {
		t.Fatal(err)
	}
	drain(t, job, cockpit.NewTranscript())
	if got := prompts(t, dir, "edited"); strings.Join(got, "|") != "first question|echo: first question|second question, edited|echo: second question, edited" {
		t.Fatalf("history = %q", got)
	}
	// The session is left alone when the prompt is gone or the session is.
	for _, request := range []cockpit.Request{
		{SessionID: "edited", Rewind: ids[1]},
		{SessionID: "missing", Rewind: ids[0]},
	} {
		request.MessageID, request.Prompt = uuid.New().String(), "again"
		_, err := cockpit.Start(t.Context(), o, request)
		if want := map[string]error{"edited": cockpit.ErrPromptGone, "missing": cockpit.ErrSessionGone}[request.SessionID]; !errors.Is(err, want) {
			t.Fatalf("rewinding %s: %v, want %v", request.SessionID, err, want)
		}
	}
	if got := prompts(t, dir, "edited"); len(got) != 4 {
		t.Fatalf("a refused rewind changed the session: %q", got)
	}
	if _, err := os.Stat(cockpit.SessionPath(dir, "missing")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("a rewind created a session")
	}
}
