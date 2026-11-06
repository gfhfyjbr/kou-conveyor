//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"encoding/json/v2"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func shutdownJSON(t *testing.T, s *shutdownServer, method, path, body string, status int, into any) {
	t.Helper()
	req, err := http.NewRequest(method, s.url+path, strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	data, _ := io.ReadAll(res.Body)
	if res.StatusCode != status {
		t.Fatalf("%s %s: %s %s", method, path, res.Status, data)
	}
	if into != nil {
		if err := json.Unmarshal(data, into); err != nil {
			t.Fatal(err)
		}
	}
}

func TestGatewayFrontendAdoptionAndStateSurviveRestart(t *testing.T) {
	o := shutdownFixture(t)
	direct := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"data":[{"id":"direct-fixture-model"}]}`)
	}))
	defer direct.Close()
	if err := cockpit.SaveSettings(o.SettingsFile, cockpit.Settings{API: cockpit.APIResponses, BaseURL: direct.URL + "/v1", APIKey: "sk-direct-fixture", Model: "direct-fixture-model"}); err != nil {
		t.Fatal(err)
	}
	first := startShutdownServer(t, o.Workspace, o.accounts)
	var adopted struct {
		Connection connectionView `json:"connection"`
	}
	shutdownJSON(t, first, "POST", "/api/connection/adopt", "", http.StatusOK, &adopted)
	if adopted.Connection.Mode != "gateway" || adopted.Connection.Model != "direct-fixture-model" {
		t.Fatalf("front-end adoption envelope: %+v", adopted)
	}
	settings, err := cockpit.LoadSettings(o.SettingsFile)
	if err != nil || settings.API != cockpit.APIGateway {
		t.Fatalf("adoption did not update this front-end: %+v %v", settings, err)
	}
	shutdownJSON(t, first, "POST", "/api/context-windows", `{"model":"fake-model","tokens":64000}`, http.StatusOK, nil)
	shutdownJSON(t, first, "POST", "/api/cache-ttls", `{"model":"fake-model","ttl":"1h"}`, http.StatusOK, nil)
	run := startShutdownRun(t, first, o.Workspace, "wait request-model")
	d, err := loadGatewayHost(o.accountsDir)
	if err != nil {
		t.Fatal(err)
	}
	first.interrupt(t)
	// Record usage in the single owning Host while no cockpit is present.
	req, _ := http.NewRequest("POST", d.Gateway.URL+"/v1/chat/completions", strings.NewReader(`{"model":"fake-model","messages":[{"role":"user","content":"hello"}]}`))
	req.Header.Set("Authorization", "Bearer "+d.Gateway.APIKey)
	req.Header.Set("Content-Type", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	io.Copy(io.Discard, res.Body)
	res.Body.Close()
	if res.StatusCode != http.StatusOK {
		t.Fatalf("offline model transport: %s", res.Status)
	}
	second := startShutdownServer(t, o.Workspace, o.accounts)
	next, err := loadGatewayHost(o.accountsDir)
	if err != nil || next.Gateway != d.Gateway {
		t.Fatalf("restart replaced the live host: %+v %v", next, err)
	}
	var overview struct {
		Connection connectionView   `json:"connection"`
		Gateway    gatewayView      `json:"gateway"`
		Models     []accounts.Model `json:"models"`
	}
	shutdownJSON(t, second, "GET", "/api/accounts", "", http.StatusOK, &overview)
	if overview.Connection.Mode != "gateway" || !overview.Gateway.Enabled || len(overview.Models) == 0 {
		t.Fatalf("accounts state/envelope: %+v", overview)
	}
	var models struct {
		Models []cockpit.ModelInfo `json:"models"`
	}
	shutdownJSON(t, second, "GET", "/api/accounts/models", "", http.StatusOK, &models)
	found := false
	for _, m := range models.Models {
		if m.ID == "fake-model" && m.Context == 64000 {
			found = true
		}
	}
	if !found {
		t.Fatalf("process-local registry/window was lost: %+v", models)
	}
	var listing accounts.ContextListing
	shutdownJSON(t, second, "GET", "/api/context-windows", "", http.StatusOK, &listing)
	found = false
	for _, m := range listing.Models {
		if m.ID == "fake-model" && m.Window == 64000 && m.TTL == "1h" {
			found = true
		}
	}
	if !found {
		t.Fatalf("model overrides were lost: %+v", listing)
	}
	waitFor(t, func() bool {
		var report accounts.UsageReport
		shutdownJSON(t, second, "GET", "/api/usage?range=24h", "", http.StatusOK, &report)
		return report.Totals.Requests > 0
	})
	var busy struct {
		RunID string `json:"run_id"`
	}
	shutdownJSON(t, second, "POST", "/api/runs", `{"session_id":"`+run.session+`","prompt":"must not start"}`, http.StatusConflict, &busy)
	shutdownJSON(t, second, "POST", "/api/runs/"+busy.RunID+"/cancel", "", http.StatusAccepted, nil)
	waitFor(t, func() bool { return !cockpit.SessionBusy(o.SessionDir, run.session) })
	second.interrupt(t)
	waitGatewayGone(t, newGatewayPeer(d, nil))
	if data, err := os.ReadFile(o.SettingsFile); err != nil || strings.Contains(string(data), d.Token) {
		t.Fatal("the helper's private token escaped into connection settings")
	}
}
