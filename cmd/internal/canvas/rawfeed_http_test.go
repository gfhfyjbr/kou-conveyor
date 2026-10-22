package canvas

import (
	"encoding/json/v2"
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
	More        bool              `json:"more"`
	Tail        int               `json:"tail"`
	Older       int               `json:"older"`
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

// TestTranscriptCursorsKeepToTheirFilters: a filtered read with after=last
// moves the cursor of its filter only, a tail and a read with since move
// none, and every entry is still found by the reads that ask for it.
func TestTranscriptCursorsKeepToTheirFilters(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Transcript cursors")
	agent := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Worker"}}]`).IDs["0"]
	session, _ := agentTranscriptFile(t, c, agent,
		errorRecord("e1"), searchModelRecord(t, 2, "a1"), errorRecord("e2"), searchModelRecord(t, 4, "a2"))
	path := cockpit.SessionPath(c.ws.SessionDir, session)
	base := "/api/w/w1/canvases/" + id + "/nodes/" + agent + "/transcript?raw=1"
	read := func(query string) transcriptHTTPPage {
		t.Helper()
		var page transcriptHTTPPage
		te.must(t, http.StatusOK, "GET", base+query, "", "", &page)
		return page
	}
	got := func(page transcriptHTTPPage) string {
		var out []string
		for _, entry := range page.Entries {
			out = append(out, entry.Text)
		}
		return strings.Join(out, ",")
	}

	if page := read("&tail=1"); got(page) != "a2" || page.Tail != 1 || page.Older != 3 || page.Cursor != "" {
		t.Fatalf("tail: %+v", page)
	}
	if page := read("&after=last&kinds=assistant"); got(page) != "a1,a2" {
		t.Fatalf("filtered read: %q", got(page))
	}
	// Neither the tail nor the filtered read moved the cursor of the reads
	// of everything.
	if page := read("&after=last"); got(page) != "e1,a1,e2,a2" {
		t.Fatalf("unfiltered read after a tail and a filtered read: %q", got(page))
	}
	if page := read("&after=last&kinds=assistant"); got(page) != "" {
		t.Fatalf("filtered read again: %q", got(page))
	}

	appendAgentTranscript(t, path, errorRecord("e3"))
	if page := read("&after=last&since=2099-01-01T00:00:00Z"); got(page) != "" {
		t.Fatalf("read since a later time: %q", got(page))
	}
	if page := read("&after=last"); got(page) != "e3" {
		t.Fatalf("read after a read with since: %q, want the entry since did not show", got(page))
	}
	if status, body := te.call(t, "GET", base+"&tail=2&after=0", "", ""); status != http.StatusBadRequest {
		t.Errorf("tail with after: %d %s, want %d", status, body, http.StatusBadRequest)
	}
}

// TestTranscriptAnswersStayWithinTheBudget: a transcript too big for one
// answer is given in pages that say there is more.
func TestTranscriptAnswersStayWithinTheBudget(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Transcript budget")
	agent := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Worker"}}]`).IDs["0"]
	var records []any
	for index := range 40 {
		records = append(records, searchModelRecord(t, uint64(index+1), strings.Repeat(strconv.Itoa(index%10), 3000)))
	}
	agentTranscriptFile(t, c, agent, records...)
	base := "/api/w/w1/canvases/" + id + "/nodes/" + agent + "/transcript?raw=1&limit=500"
	after, total, pages := "0", 0, 0
	for {
		status, body := te.call(t, "GET", base+"&after="+after, "", "")
		if status != http.StatusOK {
			t.Fatalf("page %d: %d %s", pages, status, body)
		}
		if len(body) > transcriptBudget+4096 {
			t.Fatalf("page %d is %d bytes, over the budget", pages, len(body))
		}
		var page transcriptHTTPPage
		if err := json.Unmarshal(body, &page); err != nil {
			t.Fatal(err)
		}
		pages++
		total += len(page.Entries)
		if !page.More {
			break
		}
		after = page.Next
	}
	if total != len(records) || pages < 4 {
		t.Fatalf("%d entries in %d pages, want all %d in 4 or more", total, pages, len(records))
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
