package main

import (
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestSettingsNeverRevealTheKey(t *testing.T) {
	h := newHarness(t)
	res, view := h.do("GET", "/api/settings", "")
	if res.StatusCode != http.StatusOK || view["available"] != true || view["api_key_set"] != false || view["provider_type"] != "" {
		t.Fatalf("initial settings: %d %v", res.StatusCode, view)
	}
	defaults := view["defaults"].(map[string]any)["messages"].(map[string]any)
	if defaults["base_url"] != "https://api.anthropic.com" || defaults["key_variable"] != "ANTHROPIC_API_KEY" {
		t.Fatalf("defaults = %v", defaults)
	}

	save := `{"provider_type":"messages","model":"claude-opus-5","api_key":"sk-ant-secret-1234"}`
	req, _ := http.NewRequest("PUT", h.http.URL+"/api/settings", strings.NewReader(save))
	req.Header.Set("Content-Type", "application/json")
	saved, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(saved.Body)
	saved.Body.Close()
	if saved.StatusCode != http.StatusOK || strings.Contains(string(body), "sk-ant-secret") || !strings.Contains(string(body), `"api_key_hint":"…1234"`) {
		t.Fatalf("save: %d %s", saved.StatusCode, body)
	}
	for _, path := range []string{"/api/settings", "/api/config"} {
		res, _ := h.do("GET", path, "")
		data, _ := io.ReadAll(res.Body)
		if strings.Contains(string(data), "sk-ant-secret") {
			t.Fatalf("%s leaks the key: %s", path, data)
		}
	}
	_, config := h.do("GET", "/api/config", "")
	if c := config["connection"].(map[string]any); c["provider"] != "anthropic" || c["key"] != "settings" || config["model"] != "claude-opus-5" {
		t.Fatalf("config = %v", config)
	}
	if info, err := os.Stat(h.server.opt.SettingsFile); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("settings file: %v %v", info, err)
	}

	// Moving the saved key to another endpoint needs the key again.
	res, failed := h.do("PUT", "/api/settings", `{"provider_type":"messages","base_url":"https://collector.example.com"}`)
	if res.StatusCode != http.StatusBadRequest || failed["field"] != "api_key" {
		t.Fatalf("redirected key: %d %v", res.StatusCode, failed)
	}
	if res, _ := h.do("PUT", "/api/settings", `{"provider_type":"grpc"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("unknown type: %d", res.StatusCode)
	}
	if res, _ := h.do("PUT", "/api/settings", `{"provider_type":"messages"}`, "Sec-Fetch-Site", "cross-site"); res.StatusCode != http.StatusForbidden {
		t.Fatalf("cross-site write: %d", res.StatusCode)
	}

	// Runs pick the settings up.
	started := h.start(`{"prompt":"print-env"}`)
	got, _ := h.stream(started["run_id"].(string), "")
	var answer string
	for _, e := range got.entries() {
		if e.Kind == "assistant" {
			answer = e.Text
		}
	}
	if answer != "provider=anthropic base_url= api_key=sk-ant-secret-1234 model=claude-opus-5" {
		t.Fatalf("runner environment: %q", answer)
	}

	if res, view := h.do("PUT", "/api/settings", `{"provider_type":""}`); res.StatusCode != http.StatusOK || view["api_key_set"] != false {
		t.Fatalf("back to the environment: %d %v", res.StatusCode, view)
	}
}

func TestSettingsCheck(t *testing.T) {
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.Header.Get("Authorization") != "Bearer sk-good" {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		fmt.Fprint(w, `{"data":[{"id":"model-a"},{"id":"model-b"}]}`)
	}))
	defer api.Close()
	h := newHarness(t)
	draft := func(key string) string {
		return fmt.Sprintf(`{"provider_type":"responses","base_url":%q,"model":"model-b"%s}`, api.URL+"/v1", key)
	}
	_, check := h.do("POST", "/api/settings/check", draft(`,"api_key":"sk-good"`))
	if check["ok"] != true || len(check["models"].([]any)) != 2 {
		t.Fatalf("good key: %v", check)
	}
	_, check = h.do("POST", "/api/settings/check", draft(`,"api_key":"sk-bad"`))
	if check["ok"] != false || !strings.Contains(check["message"].(string), "rejected") {
		t.Fatalf("bad key: %v", check)
	}
	// A saved key is checked, but only against its own endpoint.
	h.do("PUT", "/api/settings", draft(`,"api_key":"sk-good"`))
	if _, check = h.do("POST", "/api/settings/check", draft("")); check["ok"] != true {
		t.Fatalf("saved key: %v", check)
	}
	_, check = h.do("POST", "/api/settings/check", `{"provider_type":"responses","base_url":"https://collector.example.com/v1"}`)
	if check["ok"] != false || !strings.Contains(check["message"].(string), "Enter the API key") {
		t.Fatalf("saved key sent elsewhere: %v", check)
	}
}

func TestSessionManagement(t *testing.T) {
	h := newHarness(t)
	for _, prompt := range []string{"first question", "second question"} {
		started := h.start(fmt.Sprintf(`{"prompt":%q,"session_id":"session-1"}`, prompt))
		h.stream(started["run_id"].(string), "")
	}
	_, session := h.do("GET", "/api/sessions/session-1", "")
	entries := session["entries"].([]any)
	var second string
	for _, raw := range entries {
		e := raw.(map[string]any)
		if e["kind"] == "user" && e["text"] == "second question" {
			second = strings.TrimPrefix(e["id"].(string), "input:")
		}
	}
	if second == "" || session["interrupted"] != false || session["pinned"] != false {
		t.Fatalf("session = %v", session)
	}

	if res, body := h.do("PATCH", "/api/sessions/session-1", `{"title":"Release prep","pinned":true}`); res.StatusCode != http.StatusOK || body["title"] != "Release prep" || body["pinned"] != true {
		t.Fatalf("patch: %d %v", res.StatusCode, body)
	}
	if _, session := h.do("GET", "/api/sessions/session-1", ""); session["title"] != "Release prep" || session["pinned"] != true {
		t.Fatalf("renamed session = %v", session)
	}
	if res, _ := h.do("PATCH", "/api/sessions/missing", `{"title":"x"}`); res.StatusCode != http.StatusNotFound {
		t.Fatalf("patch missing: %d", res.StatusCode)
	}

	res, branch := h.do("POST", "/api/sessions/session-1/branch", fmt.Sprintf(`{"message_id":%q}`, second))
	if res.StatusCode != http.StatusOK || branch["prompt"] != "second question" || branch["session_id"] == nil {
		t.Fatalf("branch: %d %v", res.StatusCode, branch)
	}
	branchID := branch["session_id"].(string)
	_, branched := h.do("GET", "/api/sessions/"+branchID, "")
	if branched["title"] != "Release prep · branch" || len(branched["entries"].([]any)) != 3 {
		t.Fatalf("branched session = %v", branched)
	}

	req, _ := http.NewRequest("GET", h.http.URL+"/api/sessions/session-1/export", nil)
	exported, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	doc, _ := io.ReadAll(exported.Body)
	exported.Body.Close()
	if exported.Header.Get("Content-Disposition") != `attachment; filename=release-prep.md` ||
		!strings.HasPrefix(exported.Header.Get("Content-Type"), "text/markdown") ||
		!strings.Contains(string(doc), "# Release prep") || !strings.Contains(string(doc), "echo: second question") {
		t.Fatalf("export: %v\n%s", exported.Header, doc)
	}

	started := h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	for path, method := range map[string]string{"/api/sessions/session-1": "DELETE", "/api/sessions/session-1/branch": "POST"} {
		if res, _ := h.do(method, path, `{}`); res.StatusCode != http.StatusConflict {
			t.Fatalf("%s %s while running: %d", method, path, res.StatusCode)
		}
	}
	// Stop the run once the runner has persisted its prompt.
	for deadline := time.Now().Add(10 * time.Second); ; time.Sleep(20 * time.Millisecond) {
		if _, session := h.do("GET", "/api/sessions/session-1", ""); len(session["entries"].([]any)) == 5 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the runner never persisted the prompt")
		}
	}
	h.do("POST", "/api/runs/"+started["run_id"].(string)+"/cancel", "")
	h.stream(started["run_id"].(string), "")
	if _, session := h.do("GET", "/api/sessions/session-1", ""); session["interrupted"] != true {
		t.Fatalf("a stopped run does not read as interrupted: %v", session["entries"])
	}

	if res, _ := h.do("DELETE", "/api/sessions/session-1", ""); res.StatusCode != http.StatusOK {
		t.Fatalf("delete: %d", res.StatusCode)
	}
	if res, _ := h.do("GET", "/api/sessions/session-1", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("deleted session: %d", res.StatusCode)
	}
	if res, _ := h.do("DELETE", "/api/sessions/session-1", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("second delete: %d", res.StatusCode)
	}
	// A tab still showing the deleted session cannot start it over.
	if res, body := h.do("POST", "/api/runs", `{"prompt":"hello?","session_id":"session-1","resume":true}`); res.StatusCode != http.StatusGone {
		t.Fatalf("run on a deleted session: %d %v", res.StatusCode, body)
	}
}

func TestBrokenSettingsCanBeReplaced(t *testing.T) {
	h := newHarness(t)
	os.WriteFile(h.server.opt.SettingsFile, []byte(`{"provider_type":"grpc"}`), 0o600)
	res, view := h.do("GET", "/api/settings", "")
	if res.StatusCode != http.StatusOK || !strings.Contains(fmt.Sprint(view["error"]), "grpc") {
		t.Fatalf("settings: %d %v", res.StatusCode, view)
	}
	if res, body := h.do("POST", "/api/runs", `{"prompt":"hello"}`); res.StatusCode != http.StatusInternalServerError ||
		!strings.Contains(fmt.Sprint(body["error"]), "connection settings") {
		t.Fatalf("run with broken settings: %d %v", res.StatusCode, body)
	}
	if res, view := h.do("PUT", "/api/settings", `{"provider_type":"messages","api_key":"sk-ant-new-1234"}`); res.StatusCode != http.StatusOK || view["api_key_set"] != true {
		t.Fatalf("replacing broken settings: %d %v", res.StatusCode, view)
	}
}

// promptID returns the message ID of the prompt with the given text.
func promptID(t *testing.T, session map[string]any, text string) string {
	t.Helper()
	for _, raw := range session["entries"].([]any) {
		if e := raw.(map[string]any); e["kind"] == "user" && e["text"] == text {
			return strings.TrimPrefix(e["id"].(string), "input:")
		}
	}
	t.Fatalf("no prompt %q in %v", text, session["entries"])
	return ""
}

func texts(session map[string]any) string {
	var texts []string
	for _, raw := range session["entries"].([]any) {
		texts = append(texts, fmt.Sprint(raw.(map[string]any)["text"]))
	}
	return strings.Join(texts, "|")
}

func TestEditingAPromptRewindsTheSession(t *testing.T) {
	h := newHarness(t)
	for _, prompt := range []string{"first question", "second question"} {
		started := h.start(fmt.Sprintf(`{"prompt":%q,"session_id":"session-1"}`, prompt))
		h.stream(started["run_id"].(string), "")
	}
	_, session := h.do("GET", "/api/sessions/session-1", "")
	first, second := promptID(t, session, "first question"), promptID(t, session, "second question")
	if session["renamed"] != false || session["title"] != "first question" {
		t.Fatalf("session = %v", session)
	}

	edit := func(prompt, rewind string) (*http.Response, map[string]any) {
		return h.do("POST", "/api/runs", fmt.Sprintf(`{"prompt":%q,"session_id":"session-1","resume":true,"rewind":%q}`, prompt, rewind))
	}
	res, started := edit("second question, edited", second)
	if res.StatusCode != http.StatusAccepted {
		t.Fatalf("edit: %d %v", res.StatusCode, started)
	}
	got, _ := h.stream(started["run_id"].(string), "")
	// The run starts from the rewound session: its usage no longer counts
	// the answer that went away.
	for _, e := range got.events {
		if e.Type == "status" {
			if e.Usage == nil || e.Usage.Input != 120 {
				t.Fatalf("first status of the edited run = %+v", e)
			}
			break
		}
	}
	_, session = h.do("GET", "/api/sessions/session-1", "")
	if want := "first question|echo: first question|second question, edited|echo: second question, edited"; texts(session) != want {
		t.Fatalf("edited session = %s", texts(session))
	}

	// Editing the first prompt changes the title that comes from it.
	res, started = edit("first question, edited", first)
	if res.StatusCode != http.StatusAccepted {
		t.Fatalf("edit of the first prompt: %d %v", res.StatusCode, started)
	}
	h.stream(started["run_id"].(string), "")
	if _, session = h.do("GET", "/api/sessions/session-1", ""); session["title"] != "first question, edited" || len(session["entries"].([]any)) != 2 {
		t.Fatalf("session after editing its first prompt = %v", session)
	}
	req, _ := http.NewRequest("GET", h.http.URL+"/api/sessions", nil)
	listed, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var sessions []sessionSummary
	json.UnmarshalRead(listed.Body, &sessions)
	listed.Body.Close()
	if len(sessions) != 1 || sessions[0].Title != "first question, edited" {
		t.Fatalf("sessions = %+v", sessions)
	}

	// A prompt that is gone, or no session at all, cannot be edited.
	if res, body := edit("again", second); res.StatusCode != http.StatusConflict || body["code"] != "prompt_gone" {
		t.Fatalf("edit of a prompt that is gone: %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("POST", "/api/runs", `{"prompt":"again","rewind":"x"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("edit without a session: %d", res.StatusCode)
	}
	if res, _ := h.do("POST", "/api/runs", fmt.Sprintf(`{"prompt":"again","session_id":"session-2","rewind":%q}`, first)); res.StatusCode != http.StatusGone {
		t.Fatalf("edit in a missing session: %d", res.StatusCode)
	}
	_, session = h.do("GET", "/api/sessions/session-1", "")
	current := promptID(t, session, "first question, edited")
	running := h.start(`{"prompt":"wait for it","session_id":"session-1"}`)
	if res, body := edit("again", current); res.StatusCode != http.StatusConflict || body["run_id"] != running["run_id"] {
		t.Fatalf("edit while running: %d %v", res.StatusCode, body)
	}
	h.do("POST", "/api/runs/"+running["run_id"].(string)+"/cancel", "")
	h.stream(running["run_id"].(string), "")

	if res, body := h.do("PATCH", "/api/sessions/session-1", `{"title":"Release prep"}`); res.StatusCode != http.StatusOK {
		t.Fatalf("rename: %d %v", res.StatusCode, body)
	}
	if _, session = h.do("GET", "/api/sessions/session-1", ""); session["renamed"] != true {
		t.Fatalf("renamed session = %v", session)
	}
}

func TestEffortIsSharedWithTheTerminal(t *testing.T) {
	h := newHarness(t)
	path := cockpit.PreferencesPath(h.server.opt.SettingsFile)
	if _, prefs := h.do("GET", "/api/preferences", ""); prefs["effort"] != "high" || prefs["saved"] != false {
		t.Fatalf("before any choice: %v", prefs)
	}
	answer := func() string {
		t.Helper()
		started := h.start(`{"prompt":"print-thinking"}`)
		got, _ := h.stream(started["run_id"].(string), "")
		for _, e := range got.entries() {
			if e.Kind == "assistant" {
				return e.Text
			}
		}
		return ""
	}

	if res, body := h.do("PUT", "/api/preferences", `{"effort":"max"}`); res.StatusCode != http.StatusOK || body["effort"] != "max" {
		t.Fatalf("save: %d %v", res.StatusCode, body)
	}
	if p := cockpit.LoadPreferences(path); p.Effort != "max" {
		t.Fatalf("the terminal cockpit reads %+v", p)
	}
	if _, config := h.do("GET", "/api/config", ""); config["thinking"] != "max" || config["effort_saved"] != true {
		t.Fatalf("config = %v", config)
	}
	if got := answer(); got != "thinking=max" {
		t.Fatalf("a run without a level: %q", got)
	}

	// The terminal cockpit chooses another.
	if err := cockpit.SaveEffort(path, "low"); err != nil {
		t.Fatal(err)
	}
	if _, prefs := h.do("GET", "/api/preferences", ""); prefs["effort"] != "low" || prefs["saved"] != true {
		t.Fatalf("after the terminal's choice: %v", prefs)
	}
	if got := answer(); got != "thinking=low" {
		t.Fatalf("a run without a level: %q", got)
	}

	if res, _ := h.do("PUT", "/api/preferences", `{"effort":"huge"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("unknown effort: %d", res.StatusCode)
	}
	if res, _ := h.do("PUT", "/api/preferences", `{"effort":"max"}`, "Sec-Fetch-Site", "cross-site"); res.StatusCode != http.StatusForbidden {
		t.Fatalf("cross-site save: %d", res.StatusCode)
	}
	if p := cockpit.LoadPreferences(path); p.Effort != "low" {
		t.Fatalf("a refused save changed the effort: %+v", p)
	}
}

func TestEachPromptShowsItsChanges(t *testing.T) {
	h := newHarness(t)
	run := func(prompt string) (string, streamed) {
		t.Helper()
		started := h.start(fmt.Sprintf(`{"prompt":%q,"session_id":"session-1","resume":%v}`, prompt, prompt != "touch first"))
		got, _ := h.stream(started["run_id"].(string), "")
		return started["message_id"].(string), got
	}
	first, got := run("touch first")
	// Browsers hear of the change as it happens, before the run is done.
	changesAt, doneAt := -1, -1
	for i, e := range got.events {
		switch {
		case e.Type == "changes" && e.Changes != nil && len(e.Changes.Changed) > 0 && changesAt < 0:
			changesAt = i
			if e.Changes.Changed[0] != "touched.txt" || e.Changes.Message != first {
				t.Fatalf("changes event = %+v", e.Changes)
			}
		case e.Type == "done":
			doneAt = i
		}
	}
	if changesAt < 0 || changesAt > doneAt {
		t.Fatalf("changes event at %d, done at %d", changesAt, doneAt)
	}

	second, _ := run("touch second")
	path := func(message, rest string) string { return "/api/sessions/session-1/changes/" + message + rest }
	_, listed := h.do("GET", path(first, ""), "")
	files, _ := listed["files"].([]any)
	if listed["available"] != true || len(files) != 1 || files[0].(map[string]any)["status"] != "added" {
		t.Fatalf("first prompt's changes = %v", listed)
	}
	_, listed = h.do("GET", path(second, ""), "")
	files, _ = listed["files"].([]any)
	if len(files) != 1 || files[0].(map[string]any)["status"] != "modified" || listed["latest"].([]any)[0] != "touched.txt" {
		t.Fatalf("second prompt's changes = %v", listed)
	}
	_, diff := h.do("GET", path(second, "/diff?path=touched.txt"), "")
	if patch, _ := diff["patch"].(string); !strings.Contains(patch, "+touch second") || strings.Contains(patch, "+touch first") {
		t.Fatalf("second prompt's diff = %v", diff)
	}

	// A prompt with nothing recorded says so; bad requests find nothing.
	if _, listed := h.do("GET", path("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b001", ""), ""); listed["available"] != false || listed["reason"] == nil {
		t.Fatalf("unrecorded prompt = %v", listed)
	}
	if res, _ := h.do("GET", path(second, "/diff"), ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("diff without a path: %d", res.StatusCode)
	}
	if res, _ := h.do("GET", "/api/sessions/..%2Fx/changes/m", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("bad session: %d", res.StatusCode)
	}

	// Deleting the session deletes what it recorded.
	h.do("DELETE", "/api/sessions/session-1", "")
	if _, listed := h.do("GET", path(first, ""), ""); listed["available"] != false {
		t.Fatalf("records outlived the session: %v", listed)
	}
}

// The pinned sessions keep the order they are dragged into, and the others
// go by when the user last wrote to them.
func TestPinnedSessionsAreOrdered(t *testing.T) {
	h := newHarness(t)
	for _, id := range []string{"s-a", "s-b", "s-c"} {
		started := h.start(fmt.Sprintf(`{"prompt":"hello","session_id":%q}`, id))
		h.stream(started["run_id"].(string), "")
	}
	for _, id := range []string{"s-a", "s-b", "s-c"} {
		if res, _ := h.do("PATCH", "/api/sessions/"+id, `{"pinned":true}`); res.StatusCode != http.StatusOK {
			t.Fatalf("pin %s: %d", id, res.StatusCode)
		}
	}
	listed := func() string {
		t.Helper()
		res, err := http.Get(h.http.URL + "/api/sessions")
		if err != nil {
			t.Fatal(err)
		}
		defer res.Body.Close()
		var sessions []struct {
			ID     string `json:"id"`
			Pinned bool   `json:"pinned"`
		}
		if err := json.UnmarshalRead(res.Body, &sessions); err != nil {
			t.Fatal(err)
		}
		var ids []string
		for _, s := range sessions {
			ids = append(ids, s.ID)
		}
		return strings.Join(ids, ",")
	}
	if got := listed(); got != "s-a,s-b,s-c" {
		t.Fatalf("pinned in order = %s", got)
	}
	if res, body := h.do("PUT", "/api/pins", `{"order":["s-c","s-a","s-b"]}`); res.StatusCode != http.StatusOK || body["ok"] != true {
		t.Fatalf("order: %d %v", res.StatusCode, body)
	}
	if got := listed(); got != "s-c,s-a,s-b" {
		t.Fatalf("dragged = %s", got)
	}
	if res, _ := h.do("PUT", "/api/pins", `{"order":"nope"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("bad order: %d", res.StatusCode)
	}
}
