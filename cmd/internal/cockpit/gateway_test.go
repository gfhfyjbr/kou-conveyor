package cockpit

import (
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestGatewaySettings(t *testing.T) {
	for _, bad := range []Settings{
		{API: APIGateway},
		{API: APIGateway, Model: "claude-opus-5", BaseURL: "http://127.0.0.1:8318"},
		{API: APIGateway, Model: "claude-opus-5", APIKey: "sk"},
		{API: APIGateway, Model: "claude-opus-5", Protocol: "grpc"},
		{API: APIMessages, Protocol: APIResponses},
	} {
		if err := bad.Validate(); err == nil {
			t.Errorf("%+v was accepted", bad)
		}
	}
	saved := Settings{API: APIMessages, BaseURL: "http://127.0.0.1:8317", APIKey: "sk-old-endpoint-key", Model: "claude-opus-5"}
	next, err := saved.Update(SettingsUpdate{API: " Gateway ", Model: " gpt-5.6-codex ", Protocol: "Responses"})
	if err != nil || next != (Settings{API: APIGateway, Model: "gpt-5.6-codex", Protocol: APIResponses}) {
		t.Fatalf("update = %+v, %v", next, err)
	}
	if _, err := saved.Update(SettingsUpdate{API: APIGateway}); err == nil {
		t.Error("the gateway without a model was accepted")
	}
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := SaveSettings(path, next); err != nil {
		t.Fatal(err)
	}
	if loaded, err := LoadSettings(path); err != nil || loaded != next {
		t.Errorf("loaded %+v, %v", loaded, err)
	}
	if env := next.Environment([]string{"PATH=/bin"}); len(env) != 1 {
		t.Errorf("unresolved gateway settings changed the environment: %v", env)
	}
}

func TestGatewayAPI(t *testing.T) {
	for model, want := range map[string]API{
		"claude-opus-5-5": APIMessages, "Claude-Sonnet-5": APIMessages, "anthropic/claude-3.5-haiku": APIMessages,
		"gpt-5.6-codex": APIResponses, "gemini-3.5-flash": APIResponses, "grok-4.5": APIResponses, "": APIResponses,
	} {
		if got := GatewayAPI(model); got != want {
			t.Errorf("GatewayAPI(%q) = %q, want %q", model, got, want)
		}
	}
	g := Gateway{URL: "http://127.0.0.1:8318/", APIKey: "sk-ua-key"}
	if got := (Settings{API: APIGateway, Model: "claude-opus-5-5"}).Through(g); got != (Settings{API: APIMessages, BaseURL: "http://127.0.0.1:8318", APIKey: "sk-ua-key", Model: "claude-opus-5-5"}) {
		t.Errorf("messages = %+v", got)
	}
	if got := (Settings{API: APIGateway, Model: "gpt-5.6-codex"}).Through(g); got.API != APIResponses || got.BaseURL != "http://127.0.0.1:8318/v1" {
		t.Errorf("responses = %+v", got)
	}
	if got := (Settings{API: APIGateway, Model: "claude-opus-5-5", Protocol: APIResponses}).Through(g); got.API != APIResponses {
		t.Errorf("a chosen protocol wins: %+v", got)
	}
	direct := Settings{API: APIResponses, BaseURL: "https://api.openai.com/v1", APIKey: "sk"}
	if got := direct.Through(g); got != direct {
		t.Errorf("direct settings changed: %+v", got)
	}
	c := ResolveConnection("", Settings{API: APIGateway, Model: "claude-opus-5-5"}, func(string) string { return "" })
	if c.Source != "gateway" || c.API != APIMessages || c.Provider != "anthropic" || c.Model != "claude-opus-5-5" {
		t.Errorf("connection = %+v", c)
	}
	if c := ResolveConnection("ollama", Settings{API: APIGateway, Model: "x"}, func(string) string { return "" }); c.Source != "flag" {
		t.Errorf("the flag wins: %+v", c)
	}
}

func TestGatewayPublication(t *testing.T) {
	settingsFile := filepath.Join(t.TempDir(), "kou-conveyor", "settings.json")
	path := GatewayPath(settingsFile)
	settings := Settings{API: APIGateway, Model: "claude-opus-5-5"}
	if _, err := ResolveGateway(t.Context(), settingsFile, settings); !errors.Is(err, ErrGatewayOff) {
		t.Fatalf("nothing published: %v", err)
	}
	server := httptest.NewServer(http.NotFoundHandler())
	defer server.Close()
	g := Gateway{URL: server.URL, APIKey: "sk-ua-key"}
	if err := PublishGateway(path, g); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
			t.Errorf("mode = %v, %v", info.Mode(), err)
		}
	}
	resolved, err := ResolveGateway(t.Context(), settingsFile, settings)
	if err != nil || resolved.BaseURL != server.URL || resolved.APIKey != "sk-ua-key" || resolved.API != APIMessages {
		t.Fatalf("resolved = %+v, %v", resolved, err)
	}
	if direct, err := ResolveGateway(t.Context(), settingsFile, Settings{}); err != nil || direct != (Settings{}) {
		t.Errorf("settings without the gateway = %+v, %v", direct, err)
	}
	// Another gateway published since: withdrawing the first leaves it.
	other := Gateway{URL: "http://127.0.0.1:1", APIKey: "sk-ua-other"}
	if err := PublishGateway(path, other); err != nil {
		t.Fatal(err)
	}
	WithdrawGateway(path, g)
	if current, err := LoadGateway(path); err != nil || current != other {
		t.Fatalf("after withdrawing another: %+v, %v", current, err)
	}
	// Published, but nothing answers there.
	listener, _ := net.Listen("tcp", "127.0.0.1:0")
	address := listener.Addr().String()
	listener.Close()
	if err := PublishGateway(path, Gateway{URL: "http://" + address, APIKey: "k"}); err != nil {
		t.Fatal(err)
	}
	if _, err := ResolveGateway(t.Context(), settingsFile, settings); !errors.Is(err, ErrGatewayOff) {
		t.Errorf("unreachable: %v", err)
	}
	WithdrawGateway(path, Gateway{URL: "http://" + address, APIKey: "k"})
	if _, err := LoadGateway(path); !errors.Is(err, ErrGatewayOff) {
		t.Errorf("withdrawn: %v", err)
	}
	if err := os.WriteFile(path, []byte(`{"url":""}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadGateway(path); err == nil || errors.Is(err, ErrGatewayOff) {
		t.Errorf("a broken publication: %v", err)
	}
}
