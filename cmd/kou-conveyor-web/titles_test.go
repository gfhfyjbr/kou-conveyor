package main

import (
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// titleGateway stands for the accounts gateway, which titles sessions over
// the Responses API with what title answers to a request; it keeps the
// models asked and the requests.
func titleGateway(t *testing.T, settingsFile string, title func(request string) string) func() []string {
	t.Helper()
	var mu sync.Mutex
	var asked []string
	gateway := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/responses" || r.Header.Get("Authorization") != "Bearer sk-gw" {
			http.NotFound(w, r)
			return
		}
		data, _ := io.ReadAll(r.Body)
		var body struct {
			Model string `json:"model"`
		}
		json.Unmarshal(data, &body)
		mu.Lock()
		asked = append(asked, body.Model+" "+string(data))
		mu.Unlock()
		answer, _ := json.Marshal(title(string(data)))
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(w, `data: {"type":"response.completed","response":{"id":"r","status":"completed","output":[{"id":"m","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":%s}]}]}}`+"\n\n", answer)
	}))
	t.Cleanup(gateway.Close)
	if err := cockpit.PublishGateway(cockpit.GatewayPath(settingsFile), cockpit.Gateway{URL: gateway.URL, APIKey: "sk-gw"}); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveSettings(settingsFile, cockpit.Settings{API: cockpit.APIGateway, Model: "grok-4.7"}); err != nil {
		t.Fatal(err)
	}
	return func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), asked...)
	}
}

// listed is the session list's entry of a session.
func (h *harness) listed(id string) sessionSummary {
	h.Helper()
	res, err := http.Get(h.http.URL + "/api/sessions")
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	var sessions []sessionSummary
	if err := json.UnmarshalRead(res.Body, &sessions); err != nil {
		h.Fatal(err)
	}
	for _, s := range sessions {
		if s.ID == id {
			return s
		}
	}
	h.Fatalf("session %s is not listed: %+v", id, sessions)
	return sessionSummary{}
}

// A model titles a new session after its first prompt — the one the
// preferences name, else the prompt's: the browsers that follow the run see
// the title come, and the session list and the session have it. The next
// prompts title nothing, and a title of the user's wins over the model's.
func TestNewSessionsAreTitledAfterTheirFirstPrompt(t *testing.T) {
	h := newHarness(t)
	asked := titleGateway(t, h.server.opt.SettingsFile, func(request string) string {
		if strings.Contains(request, "launch party") {
			return "Launch party"
		}
		return `"Release checklist."`
	})
	if _, prefs := h.do("GET", "/api/preferences", ""); prefs["auto_titles"] != false || prefs["title_model"] != "" {
		t.Fatalf("the harness's preferences = %v", prefs)
	}
	res, prefs := h.do("PUT", "/api/preferences", `{"auto_titles":true,"title_model":" gpt-6-luna "}`)
	if res.StatusCode != http.StatusOK || prefs["auto_titles"] != true || prefs["title_model"] != "gpt-6-luna" {
		t.Fatalf("turning titles on: %d %v", res.StatusCode, prefs)
	}
	if p := cockpit.LoadPreferences(cockpit.PreferencesPath(h.server.opt.SettingsFile)); p.NoAutoTitles || p.TitleModel != "gpt-6-luna" {
		t.Fatalf("the terminal cockpit reads %+v", p)
	}

	// The run waits at the gate until the title has come: those who follow
	// it see the title come while it runs.
	started := h.start(`{"prompt":"make a checklist for the release behind the gate","model":"grok-4.7"}`)
	session := started["session_id"].(string)
	body := h.waitFor(session, func(body map[string]any) bool { return body["title"] == "Release checklist" })
	if body["renamed"] != false {
		t.Fatalf("the model's title reads as the user's: %v", body)
	}
	os.WriteFile(filepath.Join(h.server.opt.Workspace, "gate"), nil, 0o600)
	run, _ := h.stream(started["run_id"].(string), "")
	titled := false
	for _, e := range run.events {
		titled = titled || e.Type == "title" && e.Text == "Release checklist"
	}
	if !titled {
		t.Fatalf("no title event: %+v", run.events)
	}
	if got := asked(); len(got) != 1 || !strings.HasPrefix(got[0], "gpt-6-luna ") || !strings.Contains(got[0], "make a checklist for the release") {
		t.Fatalf("the gateway was asked %q", got)
	}
	if s := h.listed(session); s.Title != "Release checklist" || !s.AutoTitled {
		t.Fatalf("listed = %+v", s)
	}

	// The next prompt titles nothing.
	next := h.start(fmt.Sprintf(`{"prompt":"and then?","session_id":%q,"resume":true}`, session))
	h.stream(next["run_id"].(string), "")
	if got := asked(); len(got) != 1 {
		t.Fatalf("the next prompt asked for a title: %q", got)
	}

	// The user's title wins; taking it back brings the model's back.
	if res, body := h.do("PATCH", "/api/sessions/"+session, `{"title":"Mine"}`); res.StatusCode != http.StatusOK || body["title"] != "Mine" || body["renamed"] != true {
		t.Fatalf("rename: %d %v", res.StatusCode, body)
	}
	if s := h.listed(session); s.Title != "Mine" || s.AutoTitled {
		t.Fatalf("listed after rename = %+v", s)
	}
	if res, body := h.do("PATCH", "/api/sessions/"+session, `{"title":""}`); res.StatusCode != http.StatusOK || body["title"] != "Release checklist" || body["renamed"] != false {
		t.Fatalf("reset: %d %v", res.StatusCode, body)
	}

	// Editing the first prompt titles the session again, by what it asks
	// now: the title made of the prompt replaced is dropped at once.
	_, body = h.do("GET", "/api/sessions/"+session, "")
	edited := h.start(fmt.Sprintf(`{"prompt":"plan the launch party","session_id":%q,"rewind":%q}`,
		session, promptID(t, body, "make a checklist for the release behind the gate")))
	h.stream(edited["run_id"].(string), "")
	h.waitFor(session, func(body map[string]any) bool { return body["title"] == "Launch party" })
	if got := asked(); len(got) != 2 || !strings.HasPrefix(got[1], "gpt-6-luna ") || !strings.Contains(got[1], "plan the launch party") {
		t.Fatalf("the gateway was asked %q", got)
	}

	// Without a title model, the model the first prompt runs with titles.
	if res, prefs := h.do("PUT", "/api/preferences", `{"title_model":""}`); res.StatusCode != http.StatusOK || prefs["title_model"] != "" {
		t.Fatalf("no title model: %d %v", res.StatusCode, prefs)
	}
	second := h.start(`{"prompt":"another session","model":"grok-4.7"}`)
	h.stream(second["run_id"].(string), "")
	h.waitFor(second["session_id"].(string), func(body map[string]any) bool { return body["title"] == "Release checklist" })
	if got := asked(); len(got) != 3 || !strings.HasPrefix(got[2], "grok-4.7 ") {
		t.Fatalf("the gateway was asked %q", got)
	}

	// Turned off, titles are not made.
	if res, prefs := h.do("PUT", "/api/preferences", `{"auto_titles":false}`); res.StatusCode != http.StatusOK || prefs["auto_titles"] != false {
		t.Fatalf("turning titles off: %d %v", res.StatusCode, prefs)
	}
	third := h.start(`{"prompt":"an untitled one","model":"grok-4.7"}`)
	h.stream(third["run_id"].(string), "")
	if body := h.waitFor(third["session_id"].(string), func(body map[string]any) bool { return body["run"] == nil }); body["title"] != "an untitled one" {
		t.Fatalf("an untitled session's title = %v", body["title"])
	}
	if got := asked(); len(got) != 3 {
		t.Fatalf("titles turned off asked the gateway: %q", got)
	}
	for _, bad := range []string{`{"title_model":"two words"}`, `{"auto_titles":"yes"}`} {
		if res, _ := h.do("PUT", "/api/preferences", bad); res.StatusCode != http.StatusBadRequest {
			t.Errorf("%s: %d", bad, res.StatusCode)
		}
	}
}
