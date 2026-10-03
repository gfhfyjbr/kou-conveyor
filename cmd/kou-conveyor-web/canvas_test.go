package main

import (
	"net/http"
	"strings"
	"testing"
	"time"
)

// canvasHarness starts a server that runs the canvases, its engine's
// links and launch files in a cache of the test's own.
func canvasHarness(t *testing.T) *harness {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CACHE_HOME", home+"/.cache")
	t.Setenv("XDG_CONFIG_HOME", home+"/.config")
	return newHarnessWith(t, func(o *options) { o.canvas = true })
}

// With -canvas on, the pages hear so from /api/config; a canvas made there
// lists with what it holds, and goes. Off, there are no canvases to find.
func TestCanvasesOnAndOff(t *testing.T) {
	off := newHarness(t)
	if _, config := off.do("GET", "/api/config", ""); config["canvas"] != false {
		t.Fatalf("config with the canvases off: %v", config)
	}
	if res, _ := off.do("GET", "/api/canvases", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("canvases off: %d", res.StatusCode)
	}

	h := canvasHarness(t)
	if _, config := h.do("GET", "/api/config", ""); config["canvas"] != true {
		t.Fatalf("config: %v", config)
	}
	res, made := h.do("POST", "/api/canvases", `{"title":"Board"}`)
	summary, _ := made["canvas"].(map[string]any)
	id, _ := summary["id"].(string)
	if res.StatusCode != http.StatusOK || id == "" || summary["title"] != "Board" {
		t.Fatalf("create: %d %v", res.StatusCode, made)
	}
	res, applied := h.do("POST", "/api/canvases/"+id+"/ops", `{"ops":[
		{"op":"node.add","node":{"preset":"note","title":"Readme","config":{"text":"hello"}}},
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("ops: %d %v", res.StatusCode, applied)
	}
	_, listed := h.do("GET", "/api/canvases", "")
	list, _ := listed["canvases"].([]any)
	if len(list) != 1 {
		t.Fatalf("list: %v", listed)
	}
	if first, _ := list[0].(map[string]any); first["nodes"] != float64(2) || first["agents"] != float64(1) {
		t.Fatalf("summary: %v", first)
	}
	// The workspace's own address answers too.
	ws := h.server.workspaces.startup().ID
	if res, _ := h.do("GET", "/api/w/"+ws+"/canvases/"+id, ""); res.StatusCode != http.StatusOK {
		t.Fatalf("the canvas by its workspace: %d", res.StatusCode)
	}
	if res, _ := h.do("DELETE", "/api/canvases/"+id, ""); res.StatusCode != http.StatusNoContent {
		t.Fatalf("delete: %d", res.StatusCode)
	}
	if _, listed := h.do("GET", "/api/canvases", ""); len(listed["canvases"].([]any)) != 0 {
		t.Fatalf("after the delete: %v", listed)
	}
}

// A kou agent node is a session of the workspace: what is sent to it, or
// comes along an edge, runs as a prompt of that session, whose answer the
// node puts out; and the session names its canvas and its node.
func TestCanvasAgentNode(t *testing.T) {
	h := canvasHarness(t)
	res, applied := h.do("POST", "/api/canvases/board/ops", `{"create":true,"title":"Board","ops":[
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}},
		{"op":"node.add","node":{"preset":"manual","title":"Go"}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("ops: %d %v", res.StatusCode, applied)
	}
	ids, _ := applied["ids"].(map[string]any)
	agent, _ := ids["0"].(string)
	manual, _ := ids["1"].(string)
	if agent == "" || manual == "" {
		t.Fatalf("ids: %v", applied)
	}
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

	// answer waits for the agent's answer to hold what it was asked.
	answer := func(want string) string {
		t.Helper()
		deadline := time.Now().Add(15 * time.Second)
		for {
			_, read := h.do("GET", "/api/canvases/board/nodes/"+agent+"/read?what=answer", "")
			text, _ := read["text"].(string)
			if strings.Contains(text, want) {
				return text
			}
			if time.Now().After(deadline) {
				t.Fatalf("no answer with %q: %v", want, read)
			}
			time.Sleep(25 * time.Millisecond)
		}
	}

	if res, sent := h.do("POST", "/api/canvases/board/nodes/"+agent+"/send", `{"text":"hello there"}`); res.StatusCode != http.StatusOK {
		t.Fatalf("send: %d %v", res.StatusCode, sent)
	}
	if text := answer("hello there"); !strings.HasPrefix(text, "echo: ") {
		t.Fatalf("answer: %q", text)
	}
	_, view := h.do("GET", "/api/sessions/"+session, "")
	if view["canvas"] != "board" || view["node"] != agent {
		t.Fatalf("the session does not name its canvas: %v", view)
	}

	// What the manual source fires goes along its edge, with where it
	// comes from.
	if res, wired := h.do("POST", "/api/canvases/board/ops", `{"ops":[{"op":"edge.add","edge":{"from":{"node":"`+manual+`","port":"out"},"to":{"node":"`+agent+`","port":"in"}}}]}`); res.StatusCode != http.StatusOK {
		t.Fatalf("edge: %d %v", res.StatusCode, wired)
	}
	if res, fired := h.do("POST", "/api/canvases/board/nodes/"+manual+"/fire", `{"text":"ping from the button"}`); res.StatusCode != http.StatusOK {
		t.Fatalf("fire: %d %v", res.StatusCode, fired)
	}
	if text := answer("ping from the button"); !strings.Contains(text, "«Go»") {
		t.Fatalf("the prompt did not say where it came from: %q", text)
	}
}
