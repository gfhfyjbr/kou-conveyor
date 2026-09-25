package cockpit_test

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	turns "github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// fakeGateway serves a model list as the accounts gateway does, and
// publishes itself beside a settings file.
func fakeGateway(t *testing.T, settings string) *httptest.Server {
	t.Helper()
	gateway := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" || r.Header.Get("Authorization") != "Bearer sk-gateway" {
			http.Error(w, "no", http.StatusUnauthorized)
			return
		}
		fmt.Fprint(w, `{"object":"list","data":[
			{"id":"grok-4.7","object":"model","owned_by":"xai","created":1789948800},
			{"id":"claude-sonnet-5","object":"model","owned_by":"anthropic","created":1782777600},
			{"id":"grok-imagine-video","object":"model","owned_by":"xai","created":1735689600},
			{"id":"claude-opus-5-5","object":"model","owned_by":"anthropic","created":1790035200},
			{"id":"gpt-6-astra","object":"model","owned_by":"openai","created":1780000000}
		]}`)
	}))
	t.Cleanup(gateway.Close)
	if err := cockpit.PublishGateway(cockpit.GatewayPath(settings), cockpit.Gateway{URL: gateway.URL, APIKey: "sk-gateway"}); err != nil {
		t.Fatal(err)
	}
	return gateway
}

// Each prompt runs with a model of its own. Through the gateway the API
// follows the model: a session goes from Claude, over the Messages API, to
// Grok, over the Responses API, and back.
func TestStartRunsEachPromptWithItsModel(t *testing.T) {
	dir := t.TempDir()
	settings := filepath.Join(t.TempDir(), "settings.json")
	gateway := fakeGateway(t, settings)
	// The protocol chosen for the settings' model stays with that model.
	if err := cockpit.SaveSettings(settings, cockpit.Settings{API: cockpit.APIGateway, Model: "claude-opus-5-5", Protocol: cockpit.APIResponses}); err != nil {
		t.Fatal(err)
	}
	o := cockpit.Options{Runner: cockpittest.Runner(t), Workspace: t.TempDir(), SessionDir: dir, SettingsFile: settings}
	tr := cockpit.NewTranscript()
	run := func(model string) string {
		t.Helper()
		id := newID()
		job, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: id, Prompt: "print-env", Model: model})
		if err != nil {
			t.Fatal(err)
		}
		tr.SubmitModel(id, "print-env", model, time.Now())
		drain(t, job, tr)
		return tr.Entries[len(tr.Entries)-1].Text
	}
	for _, step := range []struct{ model, want, answeredBy string }{
		{"", "provider=openai base_url=" + gateway.URL + "/v1 api_key=sk-gateway model=claude-opus-5-5", "claude-opus-5-5"},
		{"grok-4.7", "provider=openai base_url=" + gateway.URL + "/v1 api_key=sk-gateway model=grok-4.7", "grok-4.7"},
		{"claude-sonnet-5", "provider=anthropic base_url=" + gateway.URL + " api_key=sk-gateway model=claude-sonnet-5", "claude-sonnet-5"},
	} {
		if got := run(step.model); got != step.want {
			t.Fatalf("model %q: %s\nwant %s", step.model, got, step.want)
		}
		if got := tr.LastModel(); got != step.answeredBy {
			t.Fatalf("model %q: the prompt says %q answered it", step.model, got)
		}
	}
	// The session file keeps which model answered which prompt.
	loaded, err := cockpit.LoadSession(dir, "s")
	if err != nil {
		t.Fatal(err)
	}
	var models []string
	for _, e := range loaded.Entries {
		if e.Kind == cockpit.KindUser {
			models = append(models, e.Model)
		}
	}
	if strings.Join(models, ",") != "claude-opus-5-5,grok-4.7,claude-sonnet-5" {
		t.Fatalf("prompts' models = %v", models)
	}
	if _, err := cockpit.Start(t.Context(), o, cockpit.Request{SessionID: "s", MessageID: newID(), Prompt: "hi", Model: "two words"}); err == nil {
		t.Fatal("a model ID with a space was handed to the runner")
	}
}

func TestSettingsForModel(t *testing.T) {
	g := cockpit.Gateway{URL: "http://127.0.0.1:8318", APIKey: "sk"}
	pinned := cockpit.Settings{API: cockpit.APIGateway, Model: "claude-opus-5-5", Protocol: cockpit.APIResponses}
	if got := pinned.ForModel(""); got != pinned {
		t.Errorf("no model changed the settings: %+v", got)
	}
	if got := pinned.ForModel(" claude-opus-5-5 "); got != pinned {
		t.Errorf("the settings' own model dropped their protocol: %+v", got)
	}
	if got := pinned.ForModel("claude-sonnet-5").Through(g); got.API != cockpit.APIMessages || got.Model != "claude-sonnet-5" || got.BaseURL != g.URL {
		t.Errorf("another Claude model = %+v, want the Messages API", got)
	}
	if got := pinned.ForModel("gemini-3.5-flash").Through(g); got.API != cockpit.APIResponses || got.BaseURL != g.URL+"/v1" {
		t.Errorf("a Gemini model = %+v, want the Responses API", got)
	}
	direct := cockpit.Settings{API: cockpit.APIMessages, BaseURL: "https://api.anthropic.com", APIKey: "sk-ant", Model: "claude-opus-5"}
	if got := direct.ForModel("claude-haiku-4-5"); got.API != cockpit.APIMessages || got.APIKey != "sk-ant" || got.Model != "claude-haiku-4-5" {
		t.Errorf("a direct connection = %+v, want its endpoint with the model", got)
	}
}

func TestListModelsThroughTheGateway(t *testing.T) {
	settings := filepath.Join(t.TempDir(), "settings.json")
	fakeGateway(t, settings)
	s := cockpit.Settings{API: cockpit.APIGateway, Model: "claude-opus-5-5"}
	catalog := cockpit.ListModels(t.Context(), "", settings, s, func(string) string { return "" })
	if catalog.Error != "" || catalog.Source != "gateway" || catalog.Default != "claude-opus-5-5" {
		t.Fatalf("catalog = %+v", catalog)
	}
	var order []string
	for _, m := range catalog.Models {
		order = append(order, m.ID)
	}
	// By provider, the newest first; media last.
	if got := strings.Join(order, ","); got != "claude-opus-5-5,claude-sonnet-5,gpt-6-astra,grok-4.7,grok-imagine-video" {
		t.Fatalf("order = %s", got)
	}
	grok, _ := catalog.Find("grok-4.7")
	if grok.ProviderName != "xAI" || grok.API != cockpit.APIResponses || grok.Media {
		t.Errorf("grok = %+v", grok)
	}
	claude, _ := catalog.Find("claude-sonnet-5")
	if claude.ProviderName != "Anthropic" || claude.API != cockpit.APIMessages {
		t.Errorf("claude = %+v", claude)
	}
	if video, _ := catalog.Find("grok-imagine-video"); !video.Media {
		t.Errorf("a video model is not marked media: %+v", video)
	}
	// Without the gateway, the catalog says so and runs still take any ID.
	off := cockpit.ListModels(t.Context(), "", filepath.Join(t.TempDir(), "settings.json"), s, func(string) string { return "" })
	if off.Error == "" || off.Source != "gateway" || len(off.Models) != 0 {
		t.Errorf("without a gateway = %+v", off)
	}
}

func TestDescribeModel(t *testing.T) {
	for id, want := range map[string]string{
		"anthropic/claude-3.5-haiku": "Anthropic", "gpt-5.6-sol": "OpenAI", "o3-mini": "OpenAI", "gemini-3.5-flash": "Google",
		"kimi-k3": "Moonshot", "deepseek-v4": "DeepSeek", "qwen3-coder": "Qwen", "llama-4": "Meta", "mystery": "Other",
	} {
		if got := cockpit.DescribeModel(cockpit.ModelInfo{ID: id}, "").ProviderName; got != want {
			t.Errorf("%s: provider %q, want %q", id, got, want)
		}
	}
	if got := cockpit.DescribeModel(cockpit.ModelInfo{ID: "mystery"}, "openai").ProviderName; got != "OpenAI" {
		t.Errorf("the endpoint's provider is the fallback: %q", got)
	}
	for id, media := range map[string]bool{
		"gpt-image-2": true, "grok-imagine-image": true, "gemini-3.1-flash-image": true, "text-embedding-3-large": true,
		"claude-opus-5-5": false, "gpt-6-astra": false, "gemini-3-pro-preview": false, "kimi-k2.7-code": false,
	} {
		if got := cockpit.DescribeModel(cockpit.ModelInfo{ID: id}, "").Media; got != media {
			t.Errorf("%s: media %v, want %v", id, got, media)
		}
	}
}

// A prompt shows the model that answered it: the one it was sent to until
// the runner records its turn's model. Prompts delivered while the agent
// works are answered by the run's model; one the runner never took is
// answered by none.
func TestTranscriptRecordsEachPromptsModel(t *testing.T) {
	tr := cockpit.NewTranscript()
	apply := func(item sessionstore.Item) {
		t.Helper()
		line := fmt.Appendf(nil, `{"type":"item","data":{"Item":%s}}`, mustJSON(t, item))
		if _, err := tr.Apply(line); err != nil {
			t.Fatal(err)
		}
	}
	sequence := sessionstore.Sequence(0)
	item := func(kind sessionstore.ItemKind, data any) sessionstore.Item {
		sequence++
		return sessionstore.Item{Sequence: sequence, RecordedAt: time.Now().UTC(), Kind: kind, Data: data}
	}
	input := func(id string, delivery inbox.Delivery) sessionstore.Item {
		return item(sessionstore.ItemInput, inbox.Input{ID: inbox.ID(id), Kind: inbox.InputExternal, Payload: []byte(`"text"`), Delivery: delivery})
	}
	first := newID()
	tr.SubmitModel(first, "text", "claude-opus-5-5", time.Now())
	if got := tr.Entry("input:" + first).Model; got != "claude-opus-5-5" {
		t.Fatalf("pending prompt shows %q", got)
	}
	apply(input(first, ""))
	// The runner went with another model: that is what the prompt shows.
	apply(item(sessionstore.ItemTurn, turns.Turn{ID: "t1", Model: "claude-sonnet-5"}))
	if got := tr.Entry("input:" + first).Model; got != "claude-sonnet-5" {
		t.Fatalf("answered prompt shows %q", got)
	}
	forced := newID()
	apply(input(forced, inbox.DeliverAfterTools))
	apply(item(sessionstore.ItemTurn, turns.Turn{ID: "t2", PreviousTurnID: "t1", Model: "claude-sonnet-5"}))
	if got := tr.Entry("input:" + forced).Model; got != "claude-sonnet-5" {
		t.Fatalf("forced prompt shows %q", got)
	}
	lost := newID()
	tr.Submit(lost, "never taken", time.Now())
	tr.Finish(fmt.Errorf("the runner died"), false, time.Now())
	next := newID()
	tr.SubmitModel(next, "text", "grok-4.7", time.Now())
	apply(input(next, ""))
	apply(item(sessionstore.ItemTurn, turns.Turn{ID: "t3", PreviousTurnID: "t2", Model: "grok-4.7"}))
	if got := tr.Entry("input:" + lost).Model; got != "" {
		t.Fatalf("a prompt the runner never took shows %q", got)
	}
	if tr.LastModel() != "grok-4.7" {
		t.Fatalf("last model = %q", tr.LastModel())
	}
	// A turn of a runner that does not record models leaves the guess.
	old := newID()
	tr.SubmitModel(old, "text", "gpt-6-astra", time.Now())
	apply(input(old, ""))
	apply(item(sessionstore.ItemTurn, turns.Turn{ID: "t4", PreviousTurnID: "t3"}))
	if got := tr.Entry("input:" + old).Model; got != "gpt-6-astra" {
		t.Fatalf("prompt of an older runner shows %q", got)
	}
}

func TestQueuedPromptsKeepTheirModel(t *testing.T) {
	var q cockpit.Queue
	a := q.Add("a", "grok-4.7")
	b := q.Add("b", "")
	if !q.SetModel(b.ID, "claude-opus-5-5") {
		t.Fatal("a queued prompt's model did not change")
	}
	q.Force(a.ID)
	if q.SetModel(a.ID, "gpt-6-astra") {
		t.Fatal("a forced prompt's model changed: the running agent has it")
	}
	got, _, _ := q.Get(a.ID)
	next, _, _ := q.Get(b.ID)
	if got.Model != "grok-4.7" || next.Model != "claude-opus-5-5" {
		t.Fatalf("models = %q, %q", got.Model, next.Model)
	}
}

func mustJSON(t *testing.T, v any) []byte {
	t.Helper()
	data, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return data
}
