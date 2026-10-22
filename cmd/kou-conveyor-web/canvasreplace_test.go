package main

import (
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// A message to a kou agent node with replace takes the place of its
// sender's last prompt to the agent, as a prompt edited in the session's
// page does: the session goes back to before that prompt, and the new one
// runs from there — with the node's brief again when the session is left
// without a prompt, and without it otherwise.
func TestACanvasPromptReplacedRewindsTheAgentsSession(t *testing.T) {
	h := canvasHarness(t)
	res, applied := h.do("POST", "/api/canvases/board/ops", `{"create":true,"title":"Board","ops":[
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("ops: %d %v", res.StatusCode, applied)
	}
	ids, _ := applied["ids"].(map[string]any)
	agent, _ := ids["0"].(string)
	_, snapshot := h.do("GET", "/api/canvases/board", "")
	session := ""
	doc, _ := snapshot["doc"].(map[string]any)
	nodes, _ := doc["nodes"].([]any)
	for _, n := range nodes {
		if n, _ := n.(map[string]any); n["id"] == agent {
			runtime, _ := n["runtime"].(map[string]any)
			session, _ = runtime["session"].(string)
		}
	}
	if session == "" {
		t.Fatalf("the agent has no session: %v", snapshot)
	}
	of := func(body map[string]any, kind string) []string {
		var out []string
		entries, _ := body["entries"].([]any)
		for _, raw := range entries {
			if e, _ := raw.(map[string]any); e["kind"] == kind {
				text, _ := e["text"].(string)
				out = append(out, text)
			}
		}
		return out
	}
	// idle waits for the session to have run the prompts that end with
	// want, and answered each; it returns the prompts.
	idle := func(want ...string) []string {
		t.Helper()
		body := h.waitFor(session, func(b map[string]any) bool {
			prompts := of(b, cockpit.KindUser)
			return b["run"] == nil && len(prompts) == len(want) && len(of(b, cockpit.KindAssistant)) == len(want) &&
				strings.HasSuffix(prompts[len(prompts)-1], want[len(want)-1])
		})
		prompts := of(body, cockpit.KindUser)
		for i, p := range prompts {
			if !strings.HasSuffix(p, want[i]) {
				t.Fatalf("the prompts %q, not %q", prompts, want)
			}
		}
		return prompts
	}
	send := func(body string) {
		t.Helper()
		// The run before may hold the session a moment after it shows done.
		deadline := time.Now().Add(10 * time.Second)
		for {
			res, sent := h.do("POST", "/api/canvases/board/nodes/"+agent+"/send", body)
			if res.StatusCode == http.StatusOK {
				return
			}
			if msg, _ := sent["error"].(string); res.StatusCode != http.StatusConflict || !strings.Contains(msg, "is at work") || time.Now().After(deadline) {
				t.Fatalf("send %s: %d %v", body, res.StatusCode, sent)
			}
			time.Sleep(20 * time.Millisecond)
		}
	}

	// The only prompt replaced, the session is left with none: the new
	// one comes with the brief, as the first did.
	send(`{"text":"the first ask"}`)
	if first := idle("the first ask")[0]; !strings.HasPrefix(first, briefOpening) {
		t.Fatalf("the first prompt: %q", first)
	}
	send(`{"text":"the first ask, worded otherwise","replace":true}`)
	if first := idle("the first ask, worded otherwise")[0]; !strings.HasPrefix(first, briefOpening) {
		t.Fatalf("the replacing first prompt: %q", first)
	}

	// A later prompt replaced, the one before stays as it was, and the new
	// one comes without the brief.
	send(`{"text":"the second ask"}`)
	idle("the first ask, worded otherwise", "the second ask")
	send(`{"text":"the second ask, worded otherwise","replace":true}`)
	prompts := idle("the first ask, worded otherwise", "the second ask, worded otherwise")
	if strings.Contains(prompts[1], briefOpening) {
		t.Fatalf("the replacing prompt was briefed again: %q", prompts[1])
	}
	// Its answer is the node's, as any.
	deadline := time.Now().Add(10 * time.Second)
	for {
		_, read := h.do("GET", "/api/canvases/board/nodes/"+agent+"/read?what=answer", "")
		if text, _ := read["text"].(string); strings.Contains(text, "the second ask, worded otherwise") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("the node's answer: %v", read)
		}
		time.Sleep(25 * time.Millisecond)
	}
}
