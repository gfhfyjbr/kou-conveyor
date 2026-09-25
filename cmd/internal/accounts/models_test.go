package accounts

import (
	"slices"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy"
)

// fakeRegistry stands in for the gateway's model registry.
type fakeRegistry struct {
	clients   map[string][]*cliproxy.ModelInfo
	available []map[string]any
}

func (r fakeRegistry) GetAvailableModels(string) []map[string]any { return r.available }

func (r fakeRegistry) GetModelsForClient(id string) []*cliproxy.ModelInfo { return r.clients[id] }

// The gateway serves the models its accounts and endpoints registered. One
// whose accounts all wait out a limit stays listed, cooling, instead of
// vanishing from the list as it does from /v1/models; one only the
// registry's available list has is listed too.
func TestServedModels(t *testing.T) {
	reg := fakeRegistry{
		clients: map[string][]*cliproxy.ModelInfo{
			"claude-me.json": {
				{ID: "claude-opus-5-5", DisplayName: "Claude Opus 5.5", OwnedBy: "anthropic", Created: 3, ContextLength: 1_000_000},
				{ID: "claude-sonnet-5", DisplayName: "Claude Sonnet 5", OwnedBy: "anthropic", Created: 2},
			},
			"claude-other.json": {{ID: "claude-opus-5-5", DisplayName: "Claude Opus 5.5", OwnedBy: "anthropic"}},
			"xai-me.json":       {{ID: "grok-4.7", DisplayName: "Grok 4.7", OwnedBy: "xai", ContextLength: 500_000}},
			"gone.json":         nil,
		},
		available: []map[string]any{
			{"id": "claude-opus-5-5", "owned_by": "anthropic"},
			{"id": "grok-4.7", "owned_by": "xai"},
			{"id": "fake-model", "owned_by": "fakeai", "display_name": "Fake", "context_length": float64(8192)},
		},
	}
	models := servedModels(reg, []string{"claude-me.json", "claude-other.json", "gone.json", "xai-me.json"})
	byID := map[string]Model{}
	var ids []string
	for _, m := range models {
		byID[m.ID] = m
		ids = append(ids, m.ID)
	}
	if !slices.Equal(ids, []string{"claude-opus-5-5", "claude-sonnet-5", "fake-model", "grok-4.7"}) {
		t.Fatalf("models = %v", ids)
	}
	opus := byID["claude-opus-5-5"]
	if opus.Name != "Claude Opus 5.5" || opus.Context != 1_000_000 || opus.Cooling || !slices.Equal(opus.Clients, []string{"claude-me.json", "claude-other.json"}) {
		t.Errorf("opus = %+v", opus)
	}
	if sonnet := byID["claude-sonnet-5"]; !sonnet.Cooling {
		t.Errorf("a model no account can take now is not cooling: %+v", sonnet)
	}
	if fake := byID["fake-model"]; fake.Cooling || fake.Name != "Fake" || fake.Context != 8192 || fake.OwnedBy != "fakeai" {
		t.Errorf("a model only the available list has = %+v", fake)
	}
	if got := accountModels(reg, "claude-me.json"); !slices.Equal(got, []string{"claude-opus-5-5", "claude-sonnet-5"}) {
		t.Errorf("account's models = %v", got)
	}
}

func TestClientLabel(t *testing.T) {
	accounts := []Account{{ID: "claude-me.json", Name: "claude-me.json", ProviderName: "Claude", Label: "me@example.com"}}
	for id, want := range map[string]string{
		"claude-me.json":                     "Claude · me@example.com",
		"claude:apikey:1a2b3c4d5e6f":         "Anthropic API key",
		"codex:apikey:1a2b3c4d5e6f":          "OpenAI API key",
		"xai:apikey:1a2b3c4d5e6f":            "xAI API key",
		"openai-compatibility:fakeai:0a1b2c": "fakeai",
		"vertex:apikey:1a2b3c":               "Vertex AI API key",
		"something-else":                     "something-else",
	} {
		if got := ClientLabel(id, accounts); got != want {
			t.Errorf("ClientLabel(%q) = %q, want %q", id, got, want)
		}
	}
}
