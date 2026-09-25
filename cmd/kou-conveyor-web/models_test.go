package main

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// promptModels are the models the session's prompts ran with, in order.
func promptModels(body map[string]any) string {
	var models []string
	for _, raw := range body["entries"].([]any) {
		entry := raw.(map[string]any)
		if entry["kind"] == cockpit.KindUser {
			model, _ := entry["model"].(string)
			models = append(models, model)
		}
	}
	return strings.Join(models, ",")
}

// Each prompt runs with the model it names, a queued one with the model it
// was queued with, and the session keeps which model answered which.
func TestPromptsRunWithTheirModels(t *testing.T) {
	// A prompt that names no model runs with the environment's.
	t.Setenv("KOU_CONVEYOR_LLM_MODEL", "env-model")
	h := newHarness(t)
	started := h.start(`{"prompt":"hold at the gate","model":"claude-opus-5-5"}`)
	session := started["session_id"].(string)
	res, body := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"then grok","model":"grok-4.7"}`)
	item, _ := body["item"].(map[string]any)
	if res.StatusCode != http.StatusCreated || item["model"] != "grok-4.7" {
		t.Fatalf("enqueue: %d %v", res.StatusCode, body)
	}
	os.WriteFile(filepath.Join(h.server.opt.Workspace, "gate"), nil, 0o600)
	first, _ := h.stream(started["run_id"].(string), "")
	done := first.last()
	if done.Next == nil {
		t.Fatalf("the queued prompt did not run: %+v", done)
	}
	h.stream(done.Next.ID, "")
	body = h.waitFor(session, func(body map[string]any) bool { return body["run"] == nil })
	if got := promptModels(body); got != "claude-opus-5-5,grok-4.7" {
		t.Fatalf("prompts' models = %q", got)
	}
	// Without a model, a prompt runs with the connection's: here the
	// environment's.
	third := h.start(fmt.Sprintf(`{"prompt":"and you","session_id":%q,"resume":true}`, session))
	h.stream(third["run_id"].(string), "")
	body = h.waitFor(session, func(body map[string]any) bool { return body["run"] == nil })
	if got := promptModels(body); got != "claude-opus-5-5,grok-4.7,env-model" {
		t.Fatalf("prompts' models = %q", got)
	}
	if res, _ := h.do("POST", "/api/runs", `{"prompt":"hi","model":"two words"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("a model with a space: %d", res.StatusCode)
	}
	if res, _ := h.do("POST", "/api/sessions/"+session+"/queue", `{"text":"x","model":"two words"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("a queued model with a space: %d", res.StatusCode)
	}
}

// The composer's models are those of the connection: an endpoint's, which
// the server asks again only after a while or when told to, or the
// gateway's, through its publication when another cockpit runs it.
func TestModelsOfTheConnection(t *testing.T) {
	var asked atomic.Int32
	endpoint := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" || r.Header.Get("Authorization") != "Bearer sk-test-endpoint" {
			http.Error(w, "no", http.StatusUnauthorized)
			return
		}
		asked.Add(1)
		fmt.Fprint(w, `{"data":[{"id":"gpt-6-luna"},{"id":"gpt-image-2"},{"id":"gpt-6-astra"}]}`)
	}))
	defer endpoint.Close()
	h := newHarness(t)
	settings := cockpit.Settings{API: cockpit.APIResponses, BaseURL: endpoint.URL + "/v1", APIKey: "sk-test-endpoint", Model: "gpt-6-astra"}
	if err := cockpit.SaveSettings(h.server.opt.SettingsFile, settings); err != nil {
		t.Fatal(err)
	}
	_, catalog := h.do("GET", "/api/models", "")
	models, _ := catalog["models"].([]any)
	if catalog["source"] != "endpoint" || catalog["default"] != "gpt-6-astra" || len(models) != 3 {
		t.Fatalf("catalog = %v", catalog)
	}
	var ids []string
	for _, raw := range models {
		m := raw.(map[string]any)
		ids = append(ids, m["id"].(string))
		if m["provider_name"] != "OpenAI" || m["api"] != "responses" {
			t.Errorf("model = %v", m)
		}
	}
	if strings.Join(ids, ",") != "gpt-6-astra,gpt-6-luna,gpt-image-2" {
		t.Fatalf("models = %v", ids)
	}
	h.do("GET", "/api/models", "")
	if asked.Load() != 1 {
		t.Fatalf("the endpoint was asked %d times, want once", asked.Load())
	}
	h.do("GET", "/api/models?fresh=1", "")
	if asked.Load() != 2 {
		t.Fatalf("a fresh list did not ask the endpoint: %d", asked.Load())
	}

	// Through the gateway another cockpit runs.
	gateway := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `{"data":[{"id":"grok-4.7","owned_by":"xai"},{"id":"claude-opus-5-5","owned_by":"anthropic"}]}`)
	}))
	defer gateway.Close()
	if err := cockpit.PublishGateway(cockpit.GatewayPath(h.server.opt.SettingsFile), cockpit.Gateway{URL: gateway.URL, APIKey: "sk-gw"}); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveSettings(h.server.opt.SettingsFile, cockpit.Settings{API: cockpit.APIGateway, Model: "grok-4.7"}); err != nil {
		t.Fatal(err)
	}
	_, catalog = h.do("GET", "/api/models", "")
	models, _ = catalog["models"].([]any)
	if catalog["source"] != "gateway" || catalog["default"] != "grok-4.7" || len(models) != 2 || models[0].(map[string]any)["id"] != "claude-opus-5-5" {
		t.Fatalf("gateway catalog = %v", catalog)
	}
	if models[0].(map[string]any)["api"] != "messages" || models[1].(map[string]any)["api"] != "responses" {
		t.Fatalf("the models' APIs = %v", models)
	}
}
