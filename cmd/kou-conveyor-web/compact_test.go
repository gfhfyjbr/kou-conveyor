package main

import (
	"io"
	"net/http"
	"strings"
	"testing"
)

func TestCompactionRunStreamsTheSummary(t *testing.T) {
	h := newHarness(t)
	started := h.start(`{"prompt":"first question"}`)
	sessionID := started["session_id"].(string)
	h.stream(started["run_id"].(string), "")

	compaction := h.start(`{"compact":true,"instructions":"the release","session_id":"` + sessionID + `","resume":true}`)
	if compaction["compact"] != true || compaction["message_id"] != "" {
		t.Fatalf("start = %#v", compaction)
	}
	got, status := h.stream(compaction["run_id"].(string), "")
	if status != http.StatusOK || got.last().Type != "done" || got.last().Error != "" {
		t.Fatalf("status %d, events %#v", status, got.events)
	}
	if got.events[0].Type != "status" || got.events[0].Activity != "Compacting context" {
		t.Fatalf("first event = %#v", got.events[0])
	}
	var notices int
	for _, entry := range got.entries() {
		if entry.Kind == "user" {
			t.Fatalf("a compaction published a prompt: %#v", entry)
		}
		if entry.Kind == "notice" && entry.Text == "Context compacted from 152k tokens" && entry.Detail == "Summary of the session, focused on the release" {
			notices++
		}
	}
	if notices != 1 {
		t.Fatalf("entries = %#v", got.entries())
	}
	_, session := h.do("GET", "/api/sessions/"+sessionID, "")
	if session["interrupted"] != false || session["title"] != "first question" || len(session["entries"].([]any)) != 3 {
		t.Fatalf("session = %#v", session)
	}
}

func TestCompactionRequestsAreValidated(t *testing.T) {
	h := newHarness(t)
	for body, want := range map[string]int{
		`{"compact":true}`: http.StatusBadRequest,
		`{"compact":true,"session_id":"s-1","prompt":"hi"}`:  http.StatusBadRequest,
		`{"prompt":"hi","instructions":"focus"}`:             http.StatusBadRequest,
		`{"compact":true,"session_id":"missing-session"}`:    http.StatusGone,
		`{"compact":true,"session_id":"s-1","rewind":"abc"}`: http.StatusBadRequest,
	} {
		if res, decoded := h.do("POST", "/api/runs", body); res.StatusCode != want {
			t.Errorf("%s: %d %v, want %d", body, res.StatusCode, decoded, want)
		}
	}
}

func TestComposerOffersCommands(t *testing.T) {
	h := newHarness(t)
	// The page is the plugin host; the composer, its commands and the
	// commands themselves are built-in plugins.
	for path, wants := range map[string][]string{
		"/":               {`src="/kernel/host.js"`},
		"/kernel/host.js": {"function contributions(", "hook:${name}", "commands: Object.freeze({"},
		"/api/plugins/commands/files/web/commands.js": {"function suggestions(", "cockpit.hooks.tap('composer.submit'", "cockpit.hooks.tap('composer.key'"},
		"/api/plugins/composer/files/web/composer.js": {"id: 'prompt'", "cockpit.hooks.first('composer.key', event)", "cockpit.ui.slot('composer.above', above)"},
		"/api/plugins/session/files/web/session.js":   {"name: 'compact'", "cockpit.commands.register({ ...command"},
		"/api/plugins/plugins/files/web/plugins.js":   {"cockpit.inspector.register({ id: 'plugins'"},
		"/api/plugins/help/files/web/help.js":         {"id: 'sheet-commands'"},
	} {
		res, err := http.Get(h.http.URL + path)
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(res.Body)
		res.Body.Close()
		if res.StatusCode != http.StatusOK {
			t.Errorf("%s: %d", path, res.StatusCode)
			continue
		}
		for _, want := range wants {
			if !strings.Contains(string(body), want) {
				t.Errorf("%s lacks %q", path, want)
			}
		}
	}
}
