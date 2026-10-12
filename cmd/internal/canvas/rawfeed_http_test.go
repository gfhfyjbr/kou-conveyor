package canvas

import (
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

type transcriptHTTPPage struct {
	Node        string            `json:"node"`
	Session     string            `json:"session"`
	Cursor      string            `json:"cursor"`
	Next        string            `json:"next"`
	CursorReset bool              `json:"cursor_reset"`
	Entries     []transcriptEntry `json:"entries"`
}

type searchHTTPResult struct {
	Node    string `json:"node"`
	Session string `json:"session"`
	Query   string `json:"query"`
	Text    string `json:"text"`
}

func agentTranscriptFile(t *testing.T, c *Canvas, node string, records ...any) (string, int64) {
	t.Helper()
	session := sessionOf(t, c, node)
	path := cockpit.SessionPath(c.ws.SessionDir, session)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	var end int64
	for _, record := range records {
		end = writeSessionRecord(t, file, record)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	return session, end
}

func appendAgentTranscript(t *testing.T, path string, records ...any) int64 {
	t.Helper()
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	var end int64
	for _, record := range records {
		end = writeSessionRecord(t, file, record)
	}
	return end
}

func searchModelRecord(t *testing.T, sequence uint64, text string) map[string]any {
	t.Helper()
	return itemRecord(t, sequence, time.Date(2026, 10, 12, 10, 0, 0, 0, time.UTC), sessionstore.ItemModelResponse,
		sessionstore.ModelResponse{Response: llm.Response{Output: []llm.Item{
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text}},
		}}})
}

func TestWorkspaceRawTranscriptCursorAndSearchRoutes(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Transcript routes")
	agent := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Worker"}}]`).IDs["0"]
	session, firstEnd := agentTranscriptFile(t, c, agent, errorRecord("first"))
	path := cockpit.SessionPath(c.ws.SessionDir, session)
	secondEnd := appendAgentTranscript(t, path, errorRecord("second"))

	var first transcriptHTTPPage
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+agent+"/transcript?raw=1&after=last&limit=1", "", "", &first)
	if first.Node != agent || first.Session != session || first.Cursor != "0" || first.Next != strconv.FormatInt(firstEnd, 10) || len(first.Entries) != 1 || first.Entries[0].Text != "first" {
		t.Fatalf("first raw page: %+v", first)
	}

	// "last" is a durable reader cursor, not just an alias for the current
	// file size: the next request starts at the first page's boundary.
	var second transcriptHTTPPage
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+agent+"/transcript?raw=1&after=last&limit=10", "", "", &second)
	if second.Cursor != strconv.FormatInt(firstEnd, 10) || second.Next != strconv.FormatInt(secondEnd, 10) || len(second.Entries) != 1 || second.Entries[0].Text != "second" {
		t.Fatalf("second raw page: %+v", second)
	}

	thirdEnd := appendAgentTranscript(t, path, searchModelRecord(t, 3, "search needle"))
	var third transcriptHTTPPage
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+agent+"/transcript?raw=1&after="+strconv.FormatInt(secondEnd, 10), "", "", &third)
	if third.Next != strconv.FormatInt(thirdEnd, 10) || len(third.Entries) != 1 || third.Entries[0].Kind != "assistant" || third.Entries[0].Text != "search needle" {
		t.Fatalf("third raw page: %+v", third)
	}

	var search searchHTTPResult
	te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+agent+"/search?q=needle&limit=5", "", "", &search)
	if search.Node != agent || search.Session != session || search.Query != "needle" || !strings.Contains(search.Text, "search needle") {
		t.Fatalf("search result: %+v", search)
	}

	for _, tc := range []struct {
		path string
		want int
	}{
		{"/api/w/w1/canvases/" + id + "/nodes/" + agent + "/transcript?raw=1&after=not-a-cursor", http.StatusBadRequest},
		{"/api/w/w1/canvases/" + id + "/nodes/" + agent + "/transcript?raw=1&limit=0", http.StatusBadRequest},
		{"/api/w/w1/canvases/" + id + "/nodes/" + agent + "/search", http.StatusBadRequest},
		{"/api/w/w1/canvases/" + id + "/nodes/" + agent + "/search?q=(", http.StatusBadRequest},
		{"/api/w/w1/canvases/" + id + "/nodes/" + agent + "/search?q=needle&limit=0", http.StatusBadRequest},
	} {
		if status, body := te.call(t, "GET", tc.path, "", ""); status != tc.want {
			t.Errorf("GET %s: %d %s, want %d", tc.path, status, body, tc.want)
		}
	}
}

func TestAgentTranscriptRoutesRespectObserveRelations(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Transcript access")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"node.add","node":{"kind":"agent","title":"Stranger"}}
	]`)
	lead, stranger := res.IDs["0"], res.IDs["1"]
	leadSession, _ := agentTranscriptFile(t, c, lead, searchModelRecord(t, 1, "lead transcript"))
	agentTranscriptFile(t, c, stranger, searchModelRecord(t, 1, "stranger transcript"))

	build := te.token(t, c, lead, ScopeBuild)
	var made Result
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"agent","title":"Child"}}]}`, build, &made)
	child := made.IDs["0"]
	childSession, _ := agentTranscriptFile(t, c, child, searchModelRecord(t, 1, "child transcript"))
	observe := te.token(t, c, lead, ScopeObserve)

	var own transcriptHTTPPage
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/self/transcript?after=0", "", observe, &own)
	if own.Node != lead || own.Session != leadSession || len(own.Entries) != 1 {
		t.Fatalf("self transcript: %+v", own)
	}

	var childPage transcriptHTTPPage
	te.must(t, http.StatusOK, "GET", "/api/canvas/nodes/"+child+"/transcript?after=0", "", observe, &childPage)
	if childPage.Node != child || childPage.Session != childSession || len(childPage.Entries) != 1 {
		t.Fatalf("child transcript: %+v", childPage)
	}

	for _, path := range []string{
		"/api/canvas/nodes/" + stranger + "/transcript?after=0",
		"/api/canvas/nodes/" + stranger + "/search?q=transcript",
	} {
		if status, body := te.call(t, "GET", path, "", observe); status != http.StatusForbidden {
			t.Errorf("unrelated node %s: %d %s, want %d", path, status, body, http.StatusForbidden)
		}
	}
	if status, body := te.call(t, "GET", "/api/canvas/nodes/self/transcript?after=0", "", te.token(t, c, lead, ScopeNone)); status != http.StatusForbidden {
		t.Errorf("observe route with no scope: %d %s, want %d", status, body, http.StatusForbidden)
	}
}
