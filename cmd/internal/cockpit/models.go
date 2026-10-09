package cockpit

import (
	"cmp"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"slices"
	"strings"
	"time"
)

// ModelInfo is a model a prompt can run with.
type ModelInfo struct {
	ID string `json:"id"`
	// Name is the model's name for people, when the list gives one.
	Name string `json:"name,omitzero"`
	// Provider is who makes the model: anthropic, openai, xai, google…;
	// ProviderName says it for people.
	Provider     string `json:"provider,omitzero"`
	ProviderName string `json:"provider_name,omitzero"`
	// API is the API runs speak with it.
	API API `json:"api,omitzero"`
	// Created is when the provider released it, in Unix seconds, if known.
	Created int64 `json:"created,omitzero"`
	// Context is how many tokens its context window holds, if known.
	Context int64 `json:"context,omitzero"`
	// Media marks a model that makes images, video or sound, which an agent
	// does not run on.
	Media bool `json:"media,omitzero"`
	// Cooling: every account that serves it waits out a limit for now.
	Cooling bool `json:"cooling,omitzero"`
	// Via names the accounts that serve it, through the gateway.
	Via []string `json:"via,omitzero"`
}

// Catalog is what the model of a prompt is chosen from: the models the
// connection reaches, whichever provider serves them.
type Catalog struct {
	// Source is where the models come from: "gateway", "endpoint", or ""
	// for a connection that lists none, which runs any model it is given.
	Source string `json:"source"`
	// Default is the model a prompt runs with unless it picks another.
	Default string      `json:"default,omitzero"`
	Models  []ModelInfo `json:"models"`
	// Error says why the connection listed no models.
	Error string `json:"error,omitzero"`
}

// Find returns the catalog's model with the given ID.
func (c Catalog) Find(id string) (ModelInfo, bool) {
	for _, m := range c.Models {
		if m.ID == id {
			return m, true
		}
	}
	return ModelInfo{}, false
}

// ListModels lists the models a prompt can run with over the connection
// runs use: the gateway's, published beside settingsFile, or those of the
// endpoint the settings or the environment name. provider is the -provider
// flag, which takes precedence over the settings.
func ListModels(ctx context.Context, provider, settingsFile string, s Settings, getenv func(string) string) Catalog {
	c := ResolveConnection(provider, s, getenv)
	catalog := Catalog{Default: c.Model, Models: []ModelInfo{}}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	if c.Source == "gateway" {
		catalog.Source = "gateway"
		g, err := LoadGateway(GatewayPath(settingsFile))
		if err == nil {
			err = g.Reach(ctx)
		}
		if err == nil {
			catalog.Models, err = GatewayModels(ctx, g)
		}
		if err != nil {
			catalog.Error = err.Error()
		}
		return catalog
	}
	if c.API != APIResponses && c.API != APIMessages {
		return catalog
	}
	check := CheckConnection(ctx, c)
	if !check.OK {
		catalog.Error = check.Message
		return catalog
	}
	catalog.Source = "endpoint"
	for _, id := range check.Models {
		catalog.Models = append(catalog.Models, DescribeModel(ModelInfo{ID: id, API: c.API}, apiDefaults[c.API].Provider))
	}
	SortModels(catalog.Models)
	return catalog
}

// GatewayModels lists the models the gateway serves now, as a run sees
// them.
func GatewayModels(ctx context.Context, g Gateway) ([]ModelInfo, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(g.URL, "/")+"/v1/models", nil)
	if err != nil {
		return nil, fmt.Errorf("the gateway's address %q is not a URL", g.URL)
	}
	request.Header.Set("Authorization", "Bearer "+g.APIKey)
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", ErrGatewayOff, err)
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 8<<20))
	if err != nil {
		return nil, fmt.Errorf("read the gateway's models: %w", err)
	}
	if response.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("the gateway answered %s to its model list", response.Status)
	}
	var list struct {
		Data []struct {
			ID      string `json:"id"`
			OwnedBy string `json:"owned_by"`
			Created int64  `json:"created"`
		} `json:"data"`
	}
	if err := json.Unmarshal(body, &list); err != nil {
		return nil, errors.New("the gateway's model list is not in the OpenAI format")
	}
	models := make([]ModelInfo, 0, len(list.Data))
	for _, m := range list.Data {
		if strings.TrimSpace(m.ID) == "" {
			continue
		}
		models = append(models, DescribeModel(ModelInfo{ID: m.ID, Provider: m.OwnedBy, Created: m.Created, API: GatewayAPI(m.ID)}, ""))
	}
	SortModels(models)
	return models, nil
}

// providerNames are how the cockpits name who makes a model.
var providerNames = map[string]string{
	"anthropic": "Anthropic", "openai": "OpenAI", "google": "Google", "xai": "xAI", "moonshot": "Moonshot",
	"meta": "Meta", "deepseek": "DeepSeek", "mistral": "Mistral", "qwen": "Qwen", "zhipu": "Zhipu",
	"antigravity": "Antigravity", "cognition": "Cognition", "minimax": "MiniMax", "iflow": "iFlow",
	"openrouter": "OpenRouter", "ollama": "Ollama", "groq": "Groq", "cohere": "Cohere", "amazon": "Amazon",
}

// ProviderName is a model provider's name for people.
func ProviderName(provider string) string {
	if name, ok := providerNames[strings.ToLower(provider)]; ok {
		return name
	}
	if provider == "" {
		return "Other"
	}
	return strings.ToUpper(provider[:1]) + provider[1:]
}

// families are the model name prefixes that tell who makes a model the
// list does not say.
var families = []struct{ prefix, provider string }{
	{"claude", "anthropic"}, {"gpt", "openai"}, {"chatgpt", "openai"}, {"codex", "openai"}, {"o1", "openai"},
	{"o3", "openai"}, {"o4", "openai"}, {"gemini", "google"}, {"gemma", "google"}, {"grok", "xai"},
	{"kimi", "moonshot"}, {"moonshot", "moonshot"}, {"deepseek", "deepseek"}, {"qwen", "qwen"}, {"qwq", "qwen"},
	{"llama", "meta"}, {"muse", "meta"}, {"mistral", "mistral"}, {"codestral", "mistral"}, {"devstral", "mistral"},
	{"glm", "zhipu"}, {"minimax", "minimax"},
}

// media matches the names of models that make images, video or sound.
var media = regexp.MustCompile(`(?i)(^|[-_/.])(imagine|image|images|video|tts|audio|speech|transcribe|whisper|embedding|embeddings|embed|dall-e|sora|veo|imagen)([-_.]|$)`)

// DescribeModel fills in what a model's ID tells about it: who makes it,
// when the list does not say (fallback, else), and whether it makes media.
func DescribeModel(m ModelInfo, fallback string) ModelInfo {
	m.Provider = strings.ToLower(strings.TrimSpace(m.Provider))
	if m.Provider == "" {
		name := strings.ToLower(m.ID)
		if vendor, rest, ok := strings.Cut(name, "/"); ok && vendor != "" {
			m.Provider, name = vendor, rest
		}
		for _, f := range families {
			if m.Provider == "" && strings.HasPrefix(name, f.prefix) {
				m.Provider = f.provider
			}
		}
		if m.Provider == "" {
			m.Provider = fallback
		}
	}
	m.ProviderName = ProviderName(m.Provider)
	m.Media = m.Media || media.MatchString(m.ID)
	return m
}

// providerRank orders the providers a list of models shows first.
var providerRank = []string{"anthropic", "openai", "google", "xai", "moonshot", "deepseek", "qwen", "meta", "mistral", "zhipu"}

// SortModels orders models by provider, the best known first, then the
// newest first, then by ID; models that make media come last.
func SortModels(models []ModelInfo) {
	rank := func(provider string) int {
		if i := slices.Index(providerRank, provider); i >= 0 {
			return i
		}
		return len(providerRank)
	}
	slices.SortStableFunc(models, func(a, b ModelInfo) int {
		if a.Media != b.Media {
			if a.Media {
				return 1
			}
			return -1
		}
		return cmp.Or(
			cmp.Compare(rank(a.Provider), rank(b.Provider)),
			strings.Compare(a.Provider, b.Provider),
			cmp.Compare(b.Created, a.Created),
			strings.Compare(a.ID, b.ID),
		)
	})
}
