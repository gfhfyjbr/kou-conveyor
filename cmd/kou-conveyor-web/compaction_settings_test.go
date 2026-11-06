package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestCompactionSettingsHTTPRoundTrip(t *testing.T) {
	h := newHarness(t)
	_, initial := h.do("GET", "/api/settings", "")
	if initial["auto_compact_percent"] != float64(0) {
		t.Fatalf("default settings: %v", initial)
	}
	res, view := h.do("PUT", "/api/settings", `{"provider_type":"","auto_compact_percent":62.5}`)
	if res.StatusCode != http.StatusOK || view["auto_compact_percent"] != 62.5 {
		t.Fatalf("save: %d %v", res.StatusCode, view)
	}
	saved, err := cockpit.LoadSettings(h.server.opt.SettingsFile)
	if err != nil || saved.AutoCompactPercent != 62.5 {
		t.Fatalf("persisted: %+v, %v", saved, err)
	}
	_, view = h.do("GET", "/api/settings", "")
	if view["auto_compact_percent"] != 62.5 {
		t.Fatalf("reload: %v", view)
	}
	for _, value := range []string{"-1", "101", `"NaN"`, `"80"`, "1e999"} {
		res, body := h.do("PUT", "/api/settings", `{"provider_type":"","auto_compact_percent":`+value+`}`)
		if res.StatusCode != http.StatusBadRequest || body["error"] == nil {
			t.Fatalf("invalid %s: %d %v", value, res.StatusCode, body)
		}
		got, err := cockpit.LoadSettings(h.server.opt.SettingsFile)
		if err != nil || got != saved {
			t.Fatalf("invalid input changed settings: %+v, %v", got, err)
		}
	}
	res, view = h.do("PUT", "/api/settings", `{"provider_type":"responses"}`)
	if res.StatusCode != http.StatusOK || view["auto_compact_percent"] != 62.5 {
		t.Fatalf("older client erased percent: %d %v", res.StatusCode, view)
	}
	res, view = h.do("PUT", "/api/settings", `{"provider_type":"responses","auto_compact_percent":0}`)
	if res.StatusCode != http.StatusOK || view["auto_compact_percent"] != float64(0) {
		t.Fatalf("reset: %d %v", res.StatusCode, view)
	}
}

func TestCompactionSettingsWebAndCanvasPropagation(t *testing.T) {
	h := canvasHarness(t)
	t.Setenv(runconfig.AutoCompactEnvironment, "off")
	if res, view := h.do("PUT", "/api/settings", `{"provider_type":"","auto_compact_percent":62.5}`); res.StatusCode != http.StatusOK {
		t.Fatalf("settings: %d %v", res.StatusCode, view)
	}
	started := h.start(`{"prompt":"print-compaction"}`)
	result, _ := h.stream(started["run_id"].(string), "")
	found := false
	for _, entry := range result.entries() {
		if entry.Kind == cockpit.KindAssistant && entry.Text == "auto_compact=62.5%" {
			found = true
		}
	}
	if !found {
		t.Fatalf("web runner did not receive threshold: %+v", result.entries())
	}
	res, applied := h.do("POST", "/api/canvases/percent/ops", `{"create":true,"ops":[{"op":"node.add","node":{"kind":"agent","title":"Percent"}}]}`)
	if res.StatusCode != http.StatusOK {
		t.Fatalf("canvas: %d %v", res.StatusCode, applied)
	}
	agent := applied["ids"].(map[string]any)["0"].(string)
	_, snapshot := h.do("GET", "/api/canvases/percent", "")
	var session string
	for _, raw := range snapshot["doc"].(map[string]any)["nodes"].([]any) {
		node := raw.(map[string]any)
		if node["id"] == agent {
			session = node["runtime"].(map[string]any)["session"].(string)
		}
	}
	if session == "" {
		t.Fatalf("canvas agent has no session: %v", snapshot)
	}
	if res, body := h.do("POST", "/api/canvases/percent/nodes/"+agent+"/send", `{"text":"print-compaction"}`); res.StatusCode != http.StatusOK {
		t.Fatalf("canvas send: %d %v", res.StatusCode, body)
	}
	h.waitFor(session, func(body map[string]any) bool {
		return body["run"] == nil && strings.Contains(texts(body), "auto_compact=62.5%")
	})
}

func TestCompactionSettingsConnectionSwitchClearsOnlyConnection(t *testing.T) {
	h := newHarness(t)
	path := h.server.opt.SettingsFile
	if err := cockpit.SaveSettings(path, cockpit.Settings{API: cockpit.APIMessages, BaseURL: "https://old.example.com", APIKey: "old-private-key", Model: "old-model", AutoCompactPercent: 75}); err != nil {
		t.Fatal(err)
	}
	for _, next := range []cockpit.Settings{
		{API: cockpit.APIGateway, Model: "gpt-6-sol", Protocol: cockpit.APIResponses},
		{API: cockpit.APIGateway},
		{},
	} {
		request, _ := http.NewRequest("POST", "/api/accounts/connection", nil)
		recorder := httptest.NewRecorder()
		h.server.saveConnection(recorder, request, next)
		if recorder.Code != http.StatusOK {
			t.Fatalf("switch %+v: %d %s", next, recorder.Code, recorder.Body.String())
		}
		want := next
		want.AutoCompactPercent = 75
		got, err := cockpit.LoadSettings(path)
		if err != nil || got != want {
			t.Fatalf("switch %+v: saved %+v, %v; want %+v", next, got, err, want)
		}
	}
	if err := os.WriteFile(path, []byte(`{broken`), 0o600); err != nil {
		t.Fatal(err)
	}
	request, _ := http.NewRequest("POST", "/api/accounts/connection", nil)
	recorder := httptest.NewRecorder()
	h.server.saveConnection(recorder, request, cockpit.Settings{})
	if got, err := cockpit.LoadSettings(path); err != nil || got != (cockpit.Settings{}) {
		t.Fatalf("repair broken config: %+v, %v", got, err)
	}
}
