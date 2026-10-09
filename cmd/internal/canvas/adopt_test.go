package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// userSession makes a session of the user's in the workspace, titled.
func userSession(t *testing.T, te *testEngine, id, title string) {
	t.Helper()
	dir := te.host.ws.SessionDir
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, id+".session.jsonl"), nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.RenameSession(dir, id, title); err != nil {
		t.Fatal(err)
	}
}

func TestAUsersSessionBecomesANodeAndOutlivesIt(t *testing.T) {
	te := newEngine(t)
	dir := te.host.ws.SessionDir
	const session = "0a1b2c3d-0000-4000-8000-000000000001"
	userSession(t, te, session, "Orchestrator")
	id, c := te.newCanvas(t, "Orchestration")

	res := te.ops(t, id, `[{"op":"node.add","node":{"preset":"foreman","session":"`+session+`"}}]`)
	node := res.IDs["0"]
	n := c.node(node)
	if n == nil || n.Runtime.Session != session || !n.Runtime.Adopted || n.Title != "Orchestrator" || n.Access != ScopeAdmin {
		t.Fatalf("the node of the session: %+v", n)
	}
	eventually(t, 5*time.Second, "the session to name its node", func() bool {
		meta := cockpit.LoadMeta(dir, session)
		return meta.Canvas == id && meta.Node == node && meta.Adopted
	})
	if meta := cockpit.LoadMeta(dir, session); meta.Title != "Orchestrator" {
		t.Fatalf("the session's own title goes: %+v", meta)
	}
	sessions, err := cockpit.ListSessions(dir)
	if err != nil || len(sessions) != 1 || !sessions[0].Adopted || sessions[0].Canvas != id {
		t.Fatalf("the session list: %+v, %v", sessions, err)
	}
	// Its runs are given the canvas, and with it the canvas's tools.
	if env := te.RunEnv("w1", session); !slices.Contains(env, "KOU_CANVAS_NODE="+node) || !slices.ContainsFunc(env, func(v string) bool { return strings.HasPrefix(v, "KOU_CANVAS_TOKEN=") }) {
		t.Fatalf("the session's runs are given %q", env)
	}
	// The first message it is sent tells it of the canvas.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+node+"/send", `{"text":"look after the children"}`, "", nil)
	eventually(t, 5*time.Second, "the message", func() bool { return len(te.host.promptsOf(session)) == 1 })
	if text := te.host.promptsOf(session)[0].text; !strings.HasPrefix(text, briefStart) || !strings.HasSuffix(text, "look after the children") {
		t.Fatalf("the first message: %q", text)
	}

	// A session is on one canvas at a time, and only the user puts it there.
	other, _ := te.newCanvas(t, "Other")
	for _, canvas := range []string{id, other} {
		if status, body := te.call(t, "POST", "/api/w/w1/canvases/"+canvas+"/ops", `{"ops":[{"op":"node.add","node":{"session":"`+session+`"}}]}`, ""); status != http.StatusConflict || !strings.Contains(string(body), "one canvas at a time") {
			t.Fatalf("the session put on a canvas again: %d %s", status, body)
		}
	}
	const stranger = "0a1b2c3d-0000-4000-8000-000000000002"
	userSession(t, te, stranger, "Someone else's")
	for _, tc := range []struct {
		name, ops, token string
		want             int
	}{
		{"no such session", `[{"op":"node.add","node":{"session":"0a1b2c3d-0000-4000-8000-00000000ffff"}}]`, "", http.StatusNotFound},
		{"not an ID", `[{"op":"node.add","node":{"session":"../x"}}]`, "", http.StatusBadRequest},
		{"a note", `[{"op":"node.add","node":{"preset":"note","session":"` + stranger + `"}}]`, "", http.StatusBadRequest},
		{"twice in a batch", `[{"op":"node.add","node":{"session":"` + stranger + `"}},{"op":"node.add","node":{"session":"` + stranger + `"}}]`, "", http.StatusBadRequest},
		{"by an agent", `[{"op":"node.add","node":{"session":"` + stranger + `"}}]`, te.token(t, c, node, ScopeAdmin), http.StatusForbidden},
	} {
		path := "/api/w/w1/canvases/" + id + "/ops"
		if tc.token != "" {
			path = "/api/canvas/ops"
		}
		if status, body := te.call(t, "POST", path, `{"ops":`+tc.ops+`}`, tc.token); status != tc.want {
			t.Fatalf("%s: %d %s, want %d", tc.name, status, body, tc.want)
		}
	}

	// Removed — even asked not to keep it — the node leaves the session
	// as it was, the user's again.
	te.ops(t, id, `[{"op":"node.remove","id":"`+node+`","keep":{"session":false},"grace_s":0}]`)
	eventually(t, 5*time.Second, "the session to be let go", func() bool { return cockpit.LoadMeta(dir, session).Canvas == "" })
	if _, err := os.Stat(cockpit.SessionPath(dir, session)); err != nil {
		t.Fatalf("the session went with its node: %v", err)
	}
	if meta := cockpit.LoadMeta(dir, session); meta.Adopted || meta.Title != "Orchestrator" {
		t.Fatalf("the session let go: %+v", meta)
	}
	if env := te.RunEnv("w1", session); env != nil {
		t.Fatalf("a session let go is still given the canvas: %q", env)
	}

	// Let go, it can be put on a canvas again; the canvas deleted, it stays.
	res = te.ops(t, other, `[{"op":"node.add","node":{"session":"`+session+`","title":"Lead"}}]`)
	if n := te.canvas(t, other).node(res.IDs["0"]); n == nil || n.Title != "Lead" || n.Access != ScopeBuild {
		t.Fatalf("the session put on another canvas: %+v", n)
	}
	eventually(t, 5*time.Second, "the session to name its new node", func() bool { return cockpit.LoadMeta(dir, session).Canvas == other })
	te.must(t, http.StatusNoContent, "DELETE", "/api/w/w1/canvases/"+other, "", "", nil)
	if _, err := os.Stat(cockpit.SessionPath(dir, session)); err != nil {
		t.Fatalf("the session went with its canvas: %v", err)
	}
	if meta := cockpit.LoadMeta(dir, session); meta.Canvas != "" || meta.Adopted || meta.Title != "Orchestrator" {
		t.Fatalf("the session of a canvas deleted: %+v", meta)
	}
}
