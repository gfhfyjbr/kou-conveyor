package main

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestCanvasModelsReuseThePickerCatalogAndCache(t *testing.T) {
	var calls atomic.Int32
	var unavailable atomic.Bool
	endpoint := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if unavailable.Load() {
			http.NotFound(w, r)
			return
		}
		fmt.Fprint(w, `{"data":[{"id":"sol/claude-alias"},{"id":"gpt-image-2"}]}`)
	}))
	defer endpoint.Close()
	h := canvasHarness(t)
	settings := cockpit.Settings{API: cockpit.APIResponses, BaseURL: endpoint.URL + "/v1", APIKey: "sk-private", Model: "sol/claude-alias"}
	if err := cockpit.SaveSettings(h.server.opt.SettingsFile, settings); err != nil {
		t.Fatal(err)
	}
	res, made := h.do("POST", "/api/canvases/models/ops", `{"create":true,"ops":[{"op":"node.add","node":{"kind":"agent"}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("canvas: %d %v", res.StatusCode, made)
	}
	_, snapshot := h.do("GET", "/api/canvases/models", "")
	node := snapshot["doc"].(map[string]any)["nodes"].([]any)[0].(map[string]any)
	session := node["runtime"].(map[string]any)["session"].(string)
	token := ""
	for _, value := range h.server.canvas.RunEnv(h.server.workspaces.startup().ID, session) {
		if value, ok := strings.CutPrefix(value, "KOU_CANVAS_TOKEN="); ok {
			token = value
		}
	}
	if token == "" {
		t.Fatal("the canvas agent has no token")
	}
	if res, picker := h.do("GET", "/api/models", ""); res.StatusCode != http.StatusOK || len(picker["models"].([]any)) != 2 {
		t.Fatalf("picker: %d %v", res.StatusCode, picker)
	}
	before := calls.Load()
	for _, query := range []string{"", "?fresh=1"} {
		res, listed := h.do("GET", "/api/canvas/models"+query, "", "Authorization", "Bearer "+token)
		models, _ := listed["models"].([]any)
		if res.StatusCode != http.StatusOK || listed["source"] != "endpoint" || listed["default"] != "sol/claude-alias" || len(models) != 1 || models[0].(map[string]any)["id"] != "sol/claude-alias" {
			t.Fatalf("canvas models: %d %v", res.StatusCode, listed)
		}
		want := before
		if query != "" {
			want++
		}
		if count := calls.Load(); count != want {
			t.Fatalf("%s: upstream model requests = %d, want %d", query, count, want)
		}
	}
	// A direct endpoint's 404 error names its private URL in the picker;
	// the canvas's error must not repeat that connection detail.
	unavailable.Store(true)
	res, listed := h.do("GET", "/api/canvas/models?fresh=1", "", "Authorization", "Bearer "+token)
	message, _ := listed["error"].(string)
	if res.StatusCode != http.StatusOK || message == "" || strings.Contains(message, endpoint.URL) || strings.Contains(message, "sk-private") {
		t.Fatalf("unavailable canvas catalog: %d %v", res.StatusCode, listed)
	}
}

func TestGatewayAliasesKeepTheirNamesAndRoutes(t *testing.T) {
	model := modelInfo(accounts.Model{ID: "sol/claude-alias", Name: "Claude display name", OwnedBy: "anthropic",
		Context: 200000, Via: []string{"sol"}, Clients: []string{"registry-client"}})
	if model.ID != "sol/claude-alias" || model.Name != "Claude display name" || model.ProviderName != "Anthropic" || strings.Join(model.Via, ",") != "sol" {
		t.Fatalf("gateway alias projection: %+v", model)
	}
	body, err := json.Marshal(model)
	if err != nil || strings.Contains(string(body), "registry-client") || strings.Contains(string(body), "clients") {
		t.Fatalf("raw account IDs in the catalog: %s, %v", body, err)
	}
}
