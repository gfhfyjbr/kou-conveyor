package cockpit

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/agentrunner"
)

func TestAPIDefaultsMatchTheRunner(t *testing.T) {
	providers := agentrunner.DefaultProviders()
	for api, defaults := range apiDefaults {
		index := slices.IndexFunc(providers, func(p agentrunner.Provider) bool { return p.Name == defaults.Provider })
		if index < 0 {
			t.Fatalf("%s: the runner has no provider %q", api, defaults.Provider)
		}
		p := providers[index]
		if p.BaseURL != defaults.BaseURL || p.DefaultModel != defaults.Model || p.APIKeyEnvironment != defaults.KeyVariable {
			t.Errorf("%s: cockpit %+v, runner %+v", api, defaults, p)
		}
	}
}

func TestSettingsRoundTripPrivately(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config", "kou-conveyor", "settings.json")
	if s, err := LoadSettings(path); err != nil || s != (Settings{}) {
		t.Fatalf("missing file = %+v, %v", s, err)
	}
	want := Settings{API: APIMessages, BaseURL: "https://gateway.example.com/anthropic", APIKey: "sk-ant-secret-1234", Model: "claude-opus-5"}
	if err := SaveSettings(path, Settings{API: " Messages ", BaseURL: want.BaseURL + "/", APIKey: " " + want.APIKey + " ", Model: want.Model}); err != nil {
		t.Fatal(err)
	}
	got, err := LoadSettings(path)
	if err != nil || got != want {
		t.Fatalf("loaded %+v, %v", got, err)
	}
	if runtime.GOOS != "windows" {
		for file, mode := range map[string]os.FileMode{path: 0o600, filepath.Dir(path): 0o700} {
			if info, err := os.Stat(file); err != nil || info.Mode().Perm() != mode {
				t.Errorf("%s mode = %v, %v", file, info.Mode().Perm(), err)
			}
		}
	}
	if entries, _ := os.ReadDir(filepath.Dir(path)); len(entries) != 1 {
		t.Errorf("temporary files left behind: %v", entries)
	}
	if err := os.WriteFile(path, []byte(`{"provider_type":"grpc"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadSettings(path); err == nil {
		t.Fatal("loaded an unknown provider type")
	}
}

func TestSettingsValidation(t *testing.T) {
	for _, s := range []Settings{
		{API: "grpc"},
		{BaseURL: "https://example.com"},
		{API: APIResponses, BaseURL: "ftp://example.com"},
		{API: APIResponses, BaseURL: "https://user:pass@example.com"},
		{API: APIResponses, BaseURL: "example.com"},
		{API: APIResponses, APIKey: "two words"},
		{API: APIResponses, APIKey: "line\nbreak"},
		{API: APIMessages, Model: "claude opus"},
	} {
		if err := s.Validate(); err == nil {
			t.Errorf("%+v is valid", s)
		}
	}
	for _, s := range []Settings{{}, {API: APIResponses}, {API: APIMessages, BaseURL: "http://localhost:4000", APIKey: "k", Model: "anthropic/claude-opus-5"}} {
		if err := s.Validate(); err != nil {
			t.Errorf("%+v: %v", s, err)
		}
	}
}

func TestUpdateKeepsTheKeyOnlyForItsEndpoint(t *testing.T) {
	saved := Settings{API: APIMessages, APIKey: "sk-ant-secret-1234"}
	key := func(k string) *string { return &k }

	next, err := saved.Update(SettingsUpdate{API: APIMessages, Model: "claude-sonnet-5"})
	if err != nil || next.APIKey != saved.APIKey || next.Model != "claude-sonnet-5" {
		t.Fatalf("same endpoint: %+v, %v", next, err)
	}
	// Spelling out the default endpoint is still the same endpoint.
	if next, err := saved.Update(SettingsUpdate{API: APIMessages, BaseURL: "https://API.anthropic.com/"}); err != nil || next.APIKey != saved.APIKey {
		t.Fatalf("default endpoint: %+v, %v", next, err)
	}
	for _, moved := range []SettingsUpdate{
		{API: APIMessages, BaseURL: "https://attacker.example.com"},
		{API: APIResponses},
	} {
		if _, err := saved.Update(moved); !errors.Is(err, ErrKeyRequired) {
			t.Errorf("%+v kept the key: %v", moved, err)
		}
	}
	next, err = saved.Update(SettingsUpdate{API: APIResponses, BaseURL: "https://gateway.example.com/v1", APIKey: key("sk-new")})
	if err != nil || next.APIKey != "sk-new" {
		t.Fatalf("new endpoint with a key: %+v, %v", next, err)
	}
	if next, err := saved.Update(SettingsUpdate{API: APIMessages, APIKey: key("")}); err != nil || next.APIKey != "" {
		t.Fatalf("clearing the key: %+v, %v", next, err)
	}
	if next, err := saved.Update(SettingsUpdate{BaseURL: "https://ignored.example.com"}); err != nil || next != (Settings{}) {
		t.Fatalf("environment: %+v, %v", next, err)
	}
	if _, err := saved.Update(SettingsUpdate{API: APIMessages, APIKey: key("has space")}); err == nil {
		t.Fatal("accepted an invalid key")
	}
	if hint := saved.KeyHint(); hint != "…1234" {
		t.Errorf("hint = %q", hint)
	}
}

func TestEnvironmentReplacesTheWholeConnection(t *testing.T) {
	env := []string{
		"PATH=/bin", "KOU_CONVEYOR_LLM_PROVIDER=openrouter", "KOU_CONVEYOR_LLM_BASE_URL=https://openrouter.ai/api/v1",
		"KOU_CONVEYOR_LLM_API_KEY=sk-or", "KOU_CONVEYOR_LLM_MODEL=other/model", "ANTHROPIC_API_KEY=sk-ant",
	}
	if got := (Settings{}).Environment(env); !slices.Equal(got, env) {
		t.Fatalf("environment settings changed the environment: %v", got)
	}
	got := Settings{API: APIMessages, APIKey: "sk-settings"}.Environment(env)
	want := []string{
		"PATH=/bin", "ANTHROPIC_API_KEY=sk-ant", "KOU_CONVEYOR_LLM_PROVIDER=anthropic",
		"KOU_CONVEYOR_LLM_BASE_URL=", "KOU_CONVEYOR_LLM_API_KEY=sk-settings", "KOU_CONVEYOR_LLM_MODEL=",
	}
	if !slices.Equal(got, want) {
		t.Fatalf("env = %v", got)
	}
}

func TestResolveConnection(t *testing.T) {
	env := func(values map[string]string) func(string) string {
		return func(name string) string { return values[name] }
	}
	c := ResolveConnection("", Settings{}, env(map[string]string{"OPENAI_API_KEY": "sk-openai"}))
	if c.Source != "environment" || c.Provider != "openai" || c.API != APIResponses || c.BaseURL != "https://api.openai.com/v1" ||
		c.Model != "gpt-6-astra" || c.Key != "OPENAI_API_KEY" || c.apiKey != "sk-openai" {
		t.Fatalf("defaults: %+v", c)
	}
	c = ResolveConnection("", Settings{API: APIMessages, Model: "claude-sonnet-5"}, env(map[string]string{
		"ANTHROPIC_API_KEY": "sk-ant", "KOU_CONVEYOR_LLM_API_KEY": "sk-other", "KOU_CONVEYOR_LLM_BASE_URL": "https://other.example.com",
	}))
	if c.Source != "settings" || c.Provider != "anthropic" || c.BaseURL != "https://api.anthropic.com" ||
		c.Model != "claude-sonnet-5" || c.Key != "ANTHROPIC_API_KEY" || c.apiKey != "sk-ant" {
		t.Fatalf("settings without a key: %+v", c)
	}
	c = ResolveConnection("ollama", Settings{API: APIMessages, APIKey: "sk-ant"}, env(map[string]string{"KOU_CONVEYOR_LLM_MODEL": "llama4"}))
	if c.Source != "flag" || c.Provider != "ollama" || c.API != APIEnvironment || c.Model != "llama4" || c.Key != "" {
		t.Fatalf("flag: %+v", c)
	}
}

func TestWorkspaceEnvFallsBackToDotEnv(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, ".env"), []byte("# comment\nCOCKPIT_TEST_FROM_FILE = file value\nCOCKPIT_TEST_SHADOWED=file\n"), 0o600)
	t.Setenv("COCKPIT_TEST_SHADOWED", "process")
	getenv := WorkspaceEnv(dir)
	if getenv("COCKPIT_TEST_FROM_FILE") != "file value" || getenv("COCKPIT_TEST_SHADOWED") != "process" || getenv("COCKPIT_TEST_MISSING") != "" {
		t.Fatal("wrong lookups")
	}
}

func TestCheckConnection(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.URL.Path == "/openai/models" && r.Header.Get("Authorization") == "Bearer good":
			fmt.Fprint(w, `{"object":"list","data":[{"id":"gpt-b"},{"id":"gpt-a"}]}`)
		case r.URL.Path == "/anthropic/v1/models" && r.Header.Get("X-Api-Key") == "good":
			fmt.Fprint(w, `{"data":[{"id":"claude-opus-5","type":"model","display_name":"Claude Opus 5","created_at":"2026-01-01T00:00:00Z"}],"has_more":false,"first_id":"claude-opus-5","last_id":"claude-opus-5"}`)
		case strings.HasPrefix(r.URL.Path, "/bare/"):
			http.NotFound(w, r)
		default:
			w.WriteHeader(http.StatusUnauthorized)
			fmt.Fprint(w, `{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}`)
		}
	}))
	defer server.Close()
	check := func(api API, base, key, model string) Check {
		return CheckConnection(context.Background(), Connection{API: api, BaseURL: server.URL + base, Model: model, apiKey: key})
	}
	if c := check(APIResponses, "/openai", "good", "gpt-c"); !c.OK || !slices.Equal(c.Models, []string{"gpt-a", "gpt-b"}) ||
		c.Message != "Connected; 2 models available, but gpt-c is not among them" {
		t.Errorf("responses: %+v", c)
	}
	if c := check(APIResponses, "/openai", "bad", ""); c.OK || !strings.Contains(c.Message, "rejected the API key") {
		t.Errorf("responses, bad key: %+v", c)
	}
	if c := check(APIMessages, "/anthropic", "good", "claude-opus-5"); !c.OK || c.Message != "Connected; 1 model available" {
		t.Errorf("messages: %+v", c)
	}
	if c := check(APIMessages, "/anthropic", "bad", ""); c.OK || !strings.Contains(c.Message, "invalid x-api-key") {
		t.Errorf("messages, bad key: %+v", c)
	}
	if c := check(APIMessages, "/bare", "good", ""); c.OK || !strings.Contains(c.Message, "unverified") {
		t.Errorf("messages without a model list: %+v", c)
	}
	if c := check(APIResponses, "/bare", "good", ""); c.OK || !strings.Contains(c.Message, "check the base URL") {
		t.Errorf("responses without a model list: %+v", c)
	}
	if c := check(APIResponses, "/openai", "", ""); c.OK || !strings.Contains(c.Message, "OPENAI_API_KEY") {
		t.Errorf("no key: %+v", c)
	}
	if c := CheckConnection(context.Background(), Connection{Provider: "ollama"}); c.OK {
		t.Errorf("unknown API: %+v", c)
	}
}
