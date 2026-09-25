package main

import (
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestAccountsGatewayOff(t *testing.T) {
	h := newHarness(t)
	res, body := h.do("GET", "/api/accounts", "")
	if res.StatusCode != http.StatusOK {
		t.Fatalf("accounts = %d", res.StatusCode)
	}
	gateway, _ := body["gateway"].(map[string]any)
	if gateway["enabled"] != false || gateway["state"] != "off" {
		t.Errorf("gateway = %v", gateway)
	}
	if list, _ := body["accounts"].([]any); len(list) != 0 {
		t.Errorf("accounts = %v", list)
	}
	if providers, _ := body["providers"].([]any); len(providers) < 5 {
		t.Errorf("providers = %v", providers)
	}
	if res, body := h.do("POST", "/api/sign-ins", `{"provider":"claude"}`); res.StatusCode != http.StatusServiceUnavailable || !strings.Contains(body["error"].(string), "-accounts") {
		t.Errorf("sign-in without a gateway = %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("GET", "/api/accounts?range=1y", ""); res.StatusCode != http.StatusBadRequest {
		t.Errorf("an unknown range = %d", res.StatusCode)
	}
	_, config := h.do("GET", "/api/config", "")
	if accounts, _ := config["accounts"].(map[string]any); accounts["state"] != "off" {
		t.Errorf("config = %v", config["accounts"])
	}
}

// TestAccountsGateway runs the gateway inside the server. The gateway keeps
// process-wide state, so this is the package's only test that starts one.
func TestAccountsGateway(t *testing.T) {
	// CLIProxyAPI replaces its configuration while it serves requests,
	// without a lock (its server reads the pointer as the watcher swaps it),
	// which the race detector reports at random. make test runs this test
	// again without the detector.
	if raceDetector {
		t.Skip("CLIProxyAPI swaps its configuration without a lock; run without -race")
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	address := listener.Addr().String()
	listener.Close()
	dir := t.TempDir()
	// Registered before the server's, this runs after it stopped.
	var settingsFile string
	t.Cleanup(func() {
		if _, err := cockpit.LoadGateway(cockpit.GatewayPath(settingsFile)); !errors.Is(err, cockpit.ErrGatewayOff) {
			t.Errorf("the gateway is still published after the server stopped: %v", err)
		}
	})
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasPrefix(r.Header.Get("Authorization"), "Bearer sk-upstream") {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"object":"list","data":[{"id":"fake-model"},{"id":"other-model"}]}`)
	}))
	defer upstream.Close()
	h := newHarnessWith(t, func(o *options) { o.accounts, o.accountsDir = address, dir })
	settingsFile = h.server.opt.SettingsFile

	var body map[string]any
	for deadline := time.Now().Add(20 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		_, body = h.do("GET", "/api/accounts", "")
		if gateway, _ := body["gateway"].(map[string]any); gateway["state"] != "starting" {
			break
		}
	}
	gateway, _ := body["gateway"].(map[string]any)
	if gateway["state"] != "running" || gateway["url"] != "http://"+address || gateway["enabled"] != true {
		t.Fatalf("gateway = %v", gateway)
	}
	key := h.server.gateway.APIKey()
	if key == "" {
		t.Fatal("the gateway has no key")
	}
	// Nothing that reaches a browser holds the key or a token.
	secret := func(t *testing.T, path string) {
		t.Helper()
		res, err := http.Get(h.http.URL + path)
		if err != nil {
			t.Fatal(err)
		}
		data, _ := io.ReadAll(res.Body)
		res.Body.Close()
		for _, s := range []string{key, "sk-ant-oat01-test", "sk-ant-ort01-test"} {
			if strings.Contains(string(data), s) {
				t.Errorf("%s shows a secret: %s", path, data)
			}
		}
	}

	name := "claude-someone@example.com.json"
	credential := `{"type":"claude","email":"someone@example.com","access_token":"sk-ant-oat01-test","refresh_token":"sk-ant-ort01-test","expired":"2099-01-01T00:00:00Z"}`
	for _, bad := range []struct{ name, content string }{{"x.json", "not json"}, {"../x.json", credential}, {"x.txt", credential}} {
		if res, _ := h.do("POST", "/api/accounts/import", `{"name":`+quote(bad.name)+`,"content":`+quote(bad.content)+`}`); res.StatusCode != http.StatusBadRequest {
			t.Errorf("import %q = %d", bad.name, res.StatusCode)
		}
	}
	if res, body := h.do("POST", "/api/accounts/import", `{"name":`+quote(name)+`,"content":`+quote(credential)+`}`); res.StatusCode != http.StatusOK {
		t.Fatalf("import = %d %v", res.StatusCode, body)
	}
	find := func() map[string]any {
		_, body := h.do("GET", "/api/accounts?range=7d", "")
		list, _ := body["accounts"].([]any)
		for _, item := range list {
			if a, _ := item.(map[string]any); a["name"] == name {
				return a
			}
		}
		return nil
	}
	var account map[string]any
	for deadline := time.Now().Add(10 * time.Second); account == nil && time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		account = find()
	}
	if account == nil || account["provider"] != "claude" || account["state"] != "ready" || account["label"] != "someone@example.com" {
		t.Fatalf("account = %v", account)
	}
	secret(t, "/api/accounts")
	secret(t, "/api/config")

	path := "/api/accounts/" + url.PathEscape(name)
	if res, body := h.do("PATCH", path, `{"disabled":true}`); res.StatusCode != http.StatusOK {
		t.Fatalf("disable = %d %v", res.StatusCode, body)
	}
	if a := find(); a["state"] != "disabled" {
		t.Errorf("disabled account = %v", a["state"])
	}
	if res, _ := h.do("PATCH", path, `{}`); res.StatusCode != http.StatusBadRequest {
		t.Errorf("an empty change = %d", res.StatusCode)
	}
	if res, _ := h.do("PATCH", "/api/accounts/nobody.json", `{"disabled":true}`); res.StatusCode != http.StatusNotFound {
		t.Errorf("an unknown account = %d", res.StatusCode)
	}

	t.Run("connection", func(t *testing.T) {
		// The server publishes the gateway for runs, of either cockpit.
		published, err := cockpit.LoadGateway(cockpit.GatewayPath(h.server.opt.SettingsFile))
		if err != nil || published.URL != "http://"+address || published.APIKey != key {
			t.Fatalf("published = %+v, %v", published, err)
		}
		if res, _ := h.do("PUT", "/api/connection", `{"mode":"gateway"}`); res.StatusCode != http.StatusBadRequest {
			t.Errorf("the gateway without a model = %d", res.StatusCode)
		}
		if res, _ := h.do("PUT", "/api/connection", `{"mode":"telepathy"}`); res.StatusCode != http.StatusBadRequest {
			t.Errorf("an unknown mode = %d", res.StatusCode)
		}
		res, body := h.do("PUT", "/api/connection", `{"mode":"gateway","model":"claude-opus-5-5"}`)
		if res.StatusCode != http.StatusOK {
			t.Fatalf("connection = %d %v", res.StatusCode, body)
		}
		saved, _ := cockpit.LoadSettings(h.server.opt.SettingsFile)
		if saved != (cockpit.Settings{API: cockpit.APIGateway, Model: "claude-opus-5-5"}) {
			t.Errorf("settings = %+v", saved)
		}
		connection, _ := body["connection"].(map[string]any)
		if connection["mode"] != "gateway" || connection["api"] != "messages" || connection["model"] != "claude-opus-5-5" {
			t.Errorf("connection = %v", connection)
		}
		if gateway, _ := body["gateway"].(map[string]any); gateway["use"] == nil {
			t.Errorf("the gateway does not know runs use it: %v", gateway)
		}
		// Runs resolve the settings against the published gateway.
		resolved, err := cockpit.ResolveGateway(t.Context(), h.server.opt.SettingsFile, saved)
		if err != nil || resolved.BaseURL != "http://"+address || resolved.APIKey != key || resolved.API != cockpit.APIMessages {
			t.Errorf("resolved = %+v, %v", resolved, err)
		}
		secret(t, "/api/accounts")
		secret(t, "/api/config")

		// A direct connection moves into the gateway as an endpoint.
		direct := cockpit.Settings{API: cockpit.APIResponses, BaseURL: upstream.URL + "/v1", APIKey: "sk-upstream-direct-0001", Model: "fake-model"}
		if err := cockpit.SaveSettings(h.server.opt.SettingsFile, direct); err != nil {
			t.Fatal(err)
		}
		_, body = h.do("GET", "/api/accounts", "")
		if connection, _ := body["connection"].(map[string]any); connection["mode"] != "direct" || connection["key_hint"] != "…0001" {
			t.Errorf("direct = %v", connection)
		}
		res, body = h.do("POST", "/api/connection/adopt", "")
		if res.StatusCode != http.StatusOK {
			t.Fatalf("adopt = %d %v", res.StatusCode, body)
		}
		saved, _ = cockpit.LoadSettings(h.server.opt.SettingsFile)
		if saved != (cockpit.Settings{API: cockpit.APIGateway, Model: "fake-model", Protocol: cockpit.APIResponses}) {
			t.Errorf("adopted settings = %+v", saved)
		}
		_, body = h.do("GET", "/api/accounts", "")
		endpoints, _ := body["endpoints"].([]any)
		if len(endpoints) != 1 {
			t.Fatalf("endpoints = %v", endpoints)
		}
		adopted, _ := endpoints[0].(map[string]any)
		models, _ := adopted["models"].([]any)
		if adopted["kind"] != "openai" || adopted["key_hint"] != "…0001" || len(models) != 2 {
			t.Errorf("adopted = %v", adopted)
		}
		if res, _ := h.do("POST", "/api/connection/adopt", ""); res.StatusCode != http.StatusConflict {
			t.Errorf("nothing left to adopt = %d", res.StatusCode)
		}
		if res, _ := h.do("PUT", "/api/connection", `{"mode":"environment"}`); res.StatusCode != http.StatusOK {
			t.Fatal(res.StatusCode)
		}
		if saved, _ := cockpit.LoadSettings(h.server.opt.SettingsFile); saved != (cockpit.Settings{}) {
			t.Errorf("environment settings = %+v", saved)
		}
		if res, _ := h.do("DELETE", "/api/endpoints/"+adopted["id"].(string), ""); res.StatusCode != http.StatusNoContent {
			t.Errorf("remove the adopted endpoint = %d", res.StatusCode)
		}
	})

	t.Run("endpoints", func(t *testing.T) {
		res, body := h.do("POST", "/api/endpoints/probe", `{"kind":"openai-compatible","base_url":`+quote(upstream.URL+"/v1")+`,"api_key":"sk-upstream-2"}`)
		if models, _ := body["models"].([]any); res.StatusCode != http.StatusOK || len(models) != 2 {
			t.Fatalf("probe = %d %v", res.StatusCode, body)
		}
		if res, _ := h.do("POST", "/api/endpoints", `{"kind":"openai-compatible","name":"bad name"}`); res.StatusCode != http.StatusBadRequest {
			t.Errorf("a bad endpoint = %d", res.StatusCode)
		}
		res, body = h.do("POST", "/api/endpoints", `{"kind":"openai-compatible","name":"router","base_url":`+quote(upstream.URL+"/v1")+
			`,"api_key":"sk-upstream-endpoint-0002","models":[{"name":"fake-model"}]}`)
		if res.StatusCode != http.StatusOK || body["name"] != "router" || body["key_hint"] != "…0002" {
			t.Fatalf("add = %d %v", res.StatusCode, body)
		}
		id := body["id"].(string)
		secretOf := func(path string) {
			t.Helper()
			res, _ := http.Get(h.http.URL + path)
			data, _ := io.ReadAll(res.Body)
			res.Body.Close()
			if strings.Contains(string(data), "sk-upstream-endpoint-0002") {
				t.Errorf("%s shows the endpoint's key", path)
			}
		}
		secretOf("/api/accounts")
		if res, body := h.do("PUT", "/api/endpoints/"+id, `{"name":"router","base_url":`+quote(upstream.URL+"/v1")+`,"models":[{"name":"fake-model"},{"name":"other-model"}]}`); res.StatusCode != http.StatusOK || body["id"] != id {
			t.Errorf("update keeping the key = %d %v", res.StatusCode, body)
		}
		if res, _ := h.do("PATCH", "/api/endpoints/"+id, `{"disabled":true}`); res.StatusCode != http.StatusOK {
			t.Errorf("disable = %d", res.StatusCode)
		}
		_, body = h.do("GET", "/api/accounts", "")
		list, _ := body["endpoints"].([]any)
		if len(list) != 1 || list[0].(map[string]any)["disabled"] != true || len(list[0].(map[string]any)["models"].([]any)) != 2 {
			t.Errorf("endpoints = %v", list)
		}
		if res, _ := h.do("DELETE", "/api/endpoints/"+id, ""); res.StatusCode != http.StatusNoContent {
			t.Errorf("remove = %d", res.StatusCode)
		}
		if res, _ := h.do("DELETE", "/api/endpoints/"+id, ""); res.StatusCode != http.StatusNotFound {
			t.Errorf("remove again = %d", res.StatusCode)
		}
	})

	if res, _ := h.do("GET", "/api/sign-ins/someone-elses", ""); res.StatusCode != http.StatusNotFound {
		t.Errorf("an unknown sign-in = %d", res.StatusCode)
	}
	if res, _ := h.do("POST", "/api/sign-ins", `{"provider":"myspace"}`); res.StatusCode != http.StatusBadRequest {
		t.Errorf("an unknown provider = %d", res.StatusCode)
	}
	if res, _ := h.do("DELETE", path, ""); res.StatusCode != http.StatusNoContent {
		t.Fatalf("remove = %d", res.StatusCode)
	}
	if a := find(); a != nil {
		t.Errorf("removed, still listed: %v", a)
	}
	if _, err := os.Stat(filepath.Join(dir, "auths", name)); !os.IsNotExist(err) {
		t.Errorf("the credential is still there: %v", err)
	}
	if res, _ := h.do("POST", "/api/accounts/import", `{"name":"x.json","content":"{}"}`, "Origin", "https://evil.example"); res.StatusCode != http.StatusForbidden {
		t.Errorf("a cross-site import = %d", res.StatusCode)
	}
}

func quote(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"', '\\':
			b.WriteByte('\\')
			b.WriteRune(r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}
