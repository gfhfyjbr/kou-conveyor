package main

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// queueOf reads a queue as the API returns it: "text" for a queued prompt,
// "!text" for a forced one, then "(paused)".
func queueOf(t *testing.T, value any) string {
	t.Helper()
	queue, _ := value.(map[string]any)
	var parts []string
	items, _ := queue["items"].([]any)
	for _, raw := range items {
		item := raw.(map[string]any)
		text := item["text"].(string)
		if forced, _ := item["forced"].(bool); forced {
			text = "!" + text
		}
		parts = append(parts, text)
	}
	if paused, _ := queue["paused"].(bool); paused {
		parts = append(parts, "(paused)")
	}
	return strings.Join(parts, " ")
}

func eventQueue(q *cockpit.Queue) string {
	var parts []string
	for _, item := range q.Items {
		text := item.Text
		if item.Forced {
			text = "!" + text
		}
		parts = append(parts, text)
	}
	if q.Paused {
		parts = append(parts, "(paused)")
	}
	return strings.Join(parts, " ")
}

// waitFor polls the session until ready says so.
func (h *harness) waitFor(session string, ready func(map[string]any) bool) map[string]any {
	h.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if res, body := h.do("GET", "/api/sessions/"+session, ""); res.StatusCode == http.StatusOK && ready(body) {
			return body
		}
		time.Sleep(20 * time.Millisecond)
	}
	h.Fatal("timed out waiting for the session")
	return nil
}

// entryTexts is what a browser shows of a run: entry events are snapshots,
// the last one of each entry counts, in the order entries first came.
func entryTexts(s streamed) string {
	var order []string
	last := map[string]*cockpit.Entry{}
	for _, e := range s.events {
		if e.Type != "entry" || e.Entry.Kind == cockpit.KindTool {
			continue
		}
		if last[e.Entry.ID] == nil {
			order = append(order, e.Entry.ID)
		}
		last[e.Entry.ID] = e.Entry
	}
	var parts []string
	for _, id := range order {
		text := last[id].Text
		if last[id].Forced {
			text = "!" + text
		}
		parts = append(parts, text)
	}
	return strings.Join(parts, "|")
}

func TestQueuedPromptRunsWhenTheRunEnds(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"hold at the gate"}`)
	session := started["session_id"].(string)
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"  then say hi  "}`)
	if res.StatusCode != http.StatusCreated || queueOf(t, body["queue"]) != "then say hi" {
		t.Fatalf("enqueue: %d %v", res.StatusCode, body)
	}
	// Every tab that opens the session sees what waits.
	_, view := h.do("GET", "/api/sessions/"+session, "")
	if queueOf(t, view["queue"]) != "then say hi" {
		t.Fatalf("session queue = %v", view["queue"])
	}
	os.WriteFile(filepath.Join(h.server.opt.Workspace, "gate"), nil, 0o600)
	first, _ := h.stream(started["run_id"].(string), "")
	done := first.last()
	if done.Type != "done" || done.Next == nil || done.Queue == nil || len(done.Queue.Items) != 0 {
		t.Fatalf("done = %+v", done)
	}
	second, _ := h.stream(done.Next.ID, "")
	if got := entryTexts(second); got != "then say hi|echo: then say hi" {
		t.Fatalf("the queued run streamed %q", got)
	}
	if second.last().Next != nil {
		t.Fatal("an empty queue started another run")
	}
}

func TestForcedPromptGoesToTheRunningAgent(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"steer the tests"}`)
	session := started["session_id"].(string)
	h.waitFor(session, func(body map[string]any) bool {
		for _, raw := range body["entries"].([]any) {
			if tool, _ := raw.(map[string]any)["tool"].(map[string]any); tool != nil && tool["state"] == cockpit.ToolRunning {
				return true
			}
		}
		return false
	})
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"use pnpm","force":true}`)
	if res.StatusCode != http.StatusCreated || queueOf(t, body["queue"]) != "!use pnpm" || body["note"] != "" {
		t.Fatalf("force: %d %v", res.StatusCode, body)
	}
	events, _ := h.stream(started["run_id"].(string), "")
	if got := entryTexts(events); got != "steer the tests|!use pnpm|steered: use pnpm" {
		t.Fatalf("the run streamed %q", got)
	}
	var queues []string
	for _, e := range events.events {
		if e.Type == "queue" {
			queues = append(queues, eventQueue(e.Queue))
		}
	}
	// Forced, then delivered.
	if strings.Join(queues, ",") != "!use pnpm," {
		t.Fatalf("queue events = %q", queues)
	}
	if done := events.last(); done.Next != nil || len(done.Queue.Items) != 0 {
		t.Fatalf("done = %+v", done)
	}
}

func TestStoppedRunPausesTheQueue(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"wait for a signal"}`)
	session, run := started["session_id"].(string), started["run_id"].(string)
	h.waitFor(session, func(body map[string]any) bool { return len(body["entries"].([]any)) > 0 })
	h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"second"}`)
	// The agent never reads this one: the run is stopped first.
	_, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"first","force":true}`)
	if queueOf(t, body["queue"]) != "!first second" {
		t.Fatalf("queue = %v", body["queue"])
	}
	// A forced prompt is the agent's: it can be neither edited nor dropped.
	forced := "/api/sessions/" + session + "/queue/" + body["item"].(map[string]any)["id"].(string)
	if res, _ := h.do("DELETE", forced, ""); res.StatusCode != http.StatusConflict {
		t.Fatalf("drop a forced prompt: %d", res.StatusCode)
	}
	if res, _ := h.do("PATCH", forced, `{"text":"changed"}`); res.StatusCode != http.StatusConflict {
		t.Fatalf("edit a forced prompt: %d", res.StatusCode)
	}
	h.do("POST", "/api/runs/"+run+"/cancel", "")
	events, _ := h.stream(run, "")
	done := events.last()
	if !done.Stopped || done.Next != nil || eventQueue(done.Queue) != "first second (paused)" {
		t.Fatalf("done = %+v, queue %q", done, eventQueue(done.Queue))
	}
	// Paused, a new prompt waits too.
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"third"}`)
	if res.StatusCode != http.StatusCreated || queueOf(t, body["queue"]) != "first second third (paused)" {
		t.Fatalf("enqueue while paused: %d %v", res.StatusCode, body)
	}
	// Resuming runs the queue, one prompt after another.
	res, body = h.do("POST", "/api/sessions/"+session+"/queue/resume", "")
	if res.StatusCode != http.StatusAccepted {
		t.Fatalf("resume: %d %v", res.StatusCode, body)
	}
	var texts []string
	next := body["run"].(map[string]any)["id"].(string)
	for next != "" {
		events, _ := h.stream(next, "")
		texts = append(texts, entryTexts(events))
		next = ""
		if done := events.last(); done.Next != nil {
			next = done.Next.ID
		}
	}
	if got := strings.Join(texts, " / "); got != "first|echo: first / second|echo: second / third|echo: third" {
		t.Fatalf("runs = %s", got)
	}
}

func TestQueueEditsMovesAndDrops(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"hold at the gate"}`)
	session := started["session_id"].(string)
	path := "/api/sessions/" + session + "/queue"
	ids := map[string]string{}
	for _, text := range []string{"a", "b", "c"} {
		_, body := h.do("POST", path, fmt.Sprintf(`{"text":%q}`, text))
		ids[text] = body["item"].(map[string]any)["id"].(string)
	}
	steps := []struct {
		method, item, body, want string
		status                   int
	}{
		{"PATCH", "b", `{"text":"B"}`, "a B c", http.StatusOK},
		{"PATCH", "c", `{"position":0}`, "c a B", http.StatusOK},
		{"DELETE", "a", ``, "c B", http.StatusOK},
		{"PATCH", "b", `{"text":"   "}`, "", http.StatusBadRequest},
		{"PATCH", "b", `{"text":"B","extra":1}`, "", http.StatusBadRequest},
		{"DELETE", "a", ``, "", http.StatusNotFound},
		{"POST", "pause", ``, "c B (paused)", http.StatusOK},
		{"POST", "resume", ``, "c B", http.StatusOK},
	}
	for _, step := range steps {
		url := path + "/" + ids[step.item]
		if step.method == "POST" {
			url = path + "/" + step.item
		}
		res, body := h.do(step.method, url, step.body)
		if res.StatusCode != step.status || step.want != "" && queueOf(t, body["queue"]) != step.want {
			t.Fatalf("%s %s %s: %d %v, want %d %q", step.method, step.item, step.body, res.StatusCode, body, step.status, step.want)
		}
	}
	if res, body := h.do("DELETE", path, ""); res.StatusCode != http.StatusOK || queueOf(t, body["queue"]) != "" {
		t.Fatalf("clear: %d %v", res.StatusCode, body)
	}
	os.WriteFile(filepath.Join(h.server.opt.Workspace, "gate"), nil, 0o600)
	if done := func() event { s, _ := h.stream(started["run_id"].(string), ""); return s.last() }(); done.Next != nil {
		t.Fatalf("a cleared queue ran %+v", done.Next)
	}
}

func TestQueueWithoutARunRunsAtOnce(t *testing.T) {
	h := newHarness(t)
	session := "2f6f2f53-6b3c-4a8c-9a38-5d9d5c0d6a11"
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"hello"}`)
	if res.StatusCode != http.StatusAccepted || body["run"] == nil {
		t.Fatalf("enqueue without a run: %d %v", res.StatusCode, body)
	}
	events, _ := h.stream(body["run"].(map[string]any)["id"].(string), "")
	if got := entryTexts(events); got != "hello|echo: hello" {
		t.Fatalf("the run streamed %q", got)
	}
	if res, _ := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":""}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("an empty prompt was queued: %d", res.StatusCode)
	}
}
