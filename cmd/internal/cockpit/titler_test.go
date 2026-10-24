package cockpit_test

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

// A session is shown under the title a model made of its first prompt until
// the user gives it one of their own; taking theirs back brings the model's
// again, and without either the session is shown under its first prompt.
func TestAutoTitleIsShownUntilTheUsersOwn(t *testing.T) {
	dir := t.TempDir()
	session(t, dir, "s", "please fix the flaky login test")
	shown := func(want string, auto bool) {
		t.Helper()
		list, _ := cockpit.ListSessions(dir)
		if len(list) != 1 || list[0].Title != want || list[0].AutoTitled != auto {
			t.Fatalf("list = %+v, want %q (auto %v)", list, want, auto)
		}
		tr, _ := cockpit.LoadSession(dir, "s")
		if got := cockpit.SessionTitle(dir, "s", tr); got != want {
			t.Fatalf("session title = %q, want %q", got, want)
		}
		if got := cockpit.ListedTitle(dir, "s"); got != want {
			t.Fatalf("listed title = %q, want %q", got, want)
		}
	}
	if err := cockpit.SetAutoTitle(dir, "s", " Flaky\nlogin test "); err != nil {
		t.Fatal(err)
	}
	shown("Flaky login test", true)
	if err := cockpit.RenameSession(dir, "s", "Mine"); err != nil {
		t.Fatal(err)
	}
	shown("Mine", false)
	if err := cockpit.RenameSession(dir, "s", ""); err != nil {
		t.Fatal(err)
	}
	shown("Flaky login test", true)
	if err := cockpit.SetAutoTitle(dir, "s", ""); err != nil {
		t.Fatal(err)
	}
	shown("please fix the flaky login test", false)
	if _, err := os.Stat(filepath.Join(dir, ".meta", "s.json")); err == nil {
		t.Fatal("empty metadata was kept")
	}

	// The title may come before the runner has made the session's file.
	if err := cockpit.SetAutoTitle(dir, "fresh", "Early title"); err != nil {
		t.Fatal(err)
	}
	if got := cockpit.LoadMeta(dir, "fresh"); got.AutoTitle != "Early title" || got.ShownTitle() != "Early title" {
		t.Fatalf("meta = %+v", got)
	}
	if err := cockpit.SetAutoTitle(dir, "bad/id", "x"); err == nil {
		t.Fatal("titled an invalid session")
	}
}

// New sessions are titled by default, by the model their first prompt runs
// with; the preferences may name another, or turn titles off.
func TestTitlePreferences(t *testing.T) {
	path := filepath.Join(t.TempDir(), "preferences.json")
	if p := cockpit.LoadPreferences(path); p.NoAutoTitles || p.TitleModel != "" {
		t.Fatalf("defaults = %+v", p)
	}
	if err := cockpit.SaveEffort(path, "max"); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveTitleModel(path, "gpt-6-luna"); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveAutoTitles(path, false); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); p.TitleModel != "gpt-6-luna" || !p.NoAutoTitles || p.Effort != "max" {
		t.Fatalf("saved = %+v", p)
	}
	if err := cockpit.SaveTitleModel(path, "two words"); err == nil {
		t.Fatal("saved a model with a space")
	}
	if err := cockpit.SaveAutoTitles(path, true); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveTitleModel(path, ""); err != nil {
		t.Fatal(err)
	}
	if p := cockpit.LoadPreferences(path); p.TitleModel != "" || p.NoAutoTitles || p.Effort != "max" {
		t.Fatalf("after reset = %+v", p)
	}
	os.WriteFile(path, []byte(`{"title_model":"bad model"}`), 0o600)
	if p := cockpit.LoadPreferences(path); p.TitleModel != "" {
		t.Fatalf("a broken title model was read: %+v", p)
	}
}

// titleRequest is what a title's request asked the model.
type titleRequest struct {
	path, model, body string
}

// titleGateway stands for the accounts gateway: Claude models answer over
// the Messages API, the others over the Responses API, where a model that
// takes no reasoning effort refuses a request that asks for one.
func titleGateway(t *testing.T, settings string) func() []titleRequest {
	t.Helper()
	var mu sync.Mutex
	var requests []titleRequest
	gateway := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		var body struct {
			Model string `json:"model"`
		}
		json.Unmarshal(data, &body)
		mu.Lock()
		requests = append(requests, titleRequest{r.URL.Path, body.Model, string(data)})
		mu.Unlock()
		switch r.URL.Path {
		case "/v1/responses":
			if r.Header.Get("Authorization") != "Bearer sk-gateway" {
				http.Error(w, "no", http.StatusUnauthorized)
				return
			}
			if strings.Contains(string(data), `"reasoning":`) {
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(http.StatusBadRequest)
				fmt.Fprint(w, `{"error":{"message":"reasoning.effort is not supported with this model","type":"invalid_request_error","code":"unsupported_parameter"}}`)
				return
			}
			w.Header().Set("Content-Type", "text/event-stream")
			fmt.Fprint(w, `data: {"type":"response.completed","response":{"id":"r","status":"completed","output":[{"id":"m","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"\"Починить флаки-тест логина.\"\nЗатем я…"}]}]}}`+"\n\n")
		case "/v1/messages":
			w.Header().Set("Content-Type", "text/event-stream")
			for _, event := range []string{
				`{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-5","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":1}}}`,
				`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Title: **Release checklist**"}}`,
				`{"type":"content_block_stop","index":0}`,
				`{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}`,
				`{"type":"message_stop"}`,
			} {
				var head struct {
					Type string `json:"type"`
				}
				json.Unmarshal([]byte(event), &head)
				fmt.Fprintf(w, "event: %s\ndata: %s\n\n", head.Type, event)
			}
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(gateway.Close)
	if err := cockpit.PublishGateway(cockpit.GatewayPath(settings), cockpit.Gateway{URL: gateway.URL, APIKey: "sk-gateway"}); err != nil {
		t.Fatal(err)
	}
	return func() []titleRequest {
		mu.Lock()
		defer mu.Unlock()
		return append([]titleRequest(nil), requests...)
	}
}

// A title is asked of the model over the connection a run of it takes:
// through the gateway, a Claude model over the Messages API, another over
// the Responses API — asked again without an effort when it takes none.
func TestGenerateTitleThroughTheGateway(t *testing.T) {
	settings := filepath.Join(t.TempDir(), "settings.json")
	requests := titleGateway(t, settings)
	if err := cockpit.SaveSettings(settings, cockpit.Settings{API: cockpit.APIGateway, Model: "grok-4.7"}); err != nil {
		t.Fatal(err)
	}
	o := cockpit.Options{SettingsFile: settings, Workspace: t.TempDir()}

	title, err := cockpit.GenerateTitle(t.Context(), o, "gpt-6-luna", "почини флаки тест логина")
	if err != nil || title != "Починить флаки-тест логина" {
		t.Fatalf("title = %q, %v", title, err)
	}
	asked := requests()
	if len(asked) != 2 || asked[0].path != "/v1/responses" || asked[1].path != "/v1/responses" {
		t.Fatalf("requests = %+v", asked)
	}
	for _, r := range asked {
		if r.model != "gpt-6-luna" || !strings.Contains(r.body, "почини флаки тест логина") || !strings.Contains(r.body, "You title chat sessions") {
			t.Fatalf("request = %+v", r)
		}
	}
	if strings.Contains(asked[1].body, `"reasoning":`) {
		t.Fatalf("asked again with an effort: %s", asked[1].body)
	}

	// Without a model of its own, the title is the settings' model's.
	title, err = cockpit.GenerateTitle(t.Context(), o, "claude-sonnet-5", "make a release checklist")
	if err != nil || title != "Release checklist" {
		t.Fatalf("title = %q, %v", title, err)
	}
	if asked = requests(); len(asked) != 3 || asked[2].path != "/v1/messages" || asked[2].model != "claude-sonnet-5" {
		t.Fatalf("requests = %+v", asked)
	}
	if _, err := cockpit.GenerateTitle(t.Context(), o, "grok-4.7", " \n "); err == nil {
		t.Fatal("titled an empty prompt")
	}
	// A provider the environment configures, of another API, makes none.
	if _, err := cockpit.GenerateTitle(t.Context(), cockpit.Options{Provider: "ollama", Workspace: t.TempDir()}, "llama", "hello"); err == nil {
		t.Fatal("titled through ollama")
	}
	if len(requests()) != 3 {
		t.Fatalf("requests = %+v", requests())
	}
}
