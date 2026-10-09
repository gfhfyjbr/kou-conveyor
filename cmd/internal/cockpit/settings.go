package cockpit

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"unicode"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/anthropic"
)

// API is the wire protocol a connection speaks.
type API string

const (
	// APIEnvironment leaves the connection to the runner's environment.
	APIEnvironment API = ""
	// APIResponses is the OpenAI Responses API.
	APIResponses API = "responses"
	// APIMessages is the Anthropic Messages API.
	APIMessages API = "messages"
	// APIGateway sends runs through the accounts gateway the browser
	// cockpit runs (gateway.go), to its accounts and endpoints, over the API
	// the model speaks there.
	APIGateway API = "gateway"
)

// APIDefaults are what a connection over an API uses when settings leave
// something out.
type APIDefaults struct {
	// Provider is the runner provider that speaks the API.
	Provider    string `json:"provider"`
	BaseURL     string `json:"base_url"`
	Model       string `json:"model"`
	KeyVariable string `json:"key_variable"`
}

// A test keeps these in step with the runner's provider table.
var apiDefaults = map[API]APIDefaults{
	APIResponses: {"openai", "https://api.openai.com/v1", "gpt-6-astra", "OPENAI_API_KEY"},
	APIMessages:  {"anthropic", "https://api.anthropic.com", "claude-opus-5", "ANTHROPIC_API_KEY"},
}

// DefaultsFor returns an API's defaults.
func DefaultsFor(api API) APIDefaults { return apiDefaults[api] }

// Runner environment variables that settings control.
const (
	providerVariable = "KOU_CONVEYOR_LLM_PROVIDER"
	baseURLVariable  = "KOU_CONVEYOR_LLM_BASE_URL"
	apiKeyVariable   = "KOU_CONVEYOR_LLM_API_KEY"
	modelVariable    = "KOU_CONVEYOR_LLM_MODEL"
)

// Settings are the connection both cockpits hand to the runner. They are
// stored with mode 0600 because they may hold an API key.
type Settings struct {
	API     API    `json:"provider_type,omitzero"`
	BaseURL string `json:"base_url,omitzero"`
	APIKey  string `json:"api_key,omitzero"`
	Model   string `json:"model,omitzero"`
	// Protocol is the API runs speak through the gateway; empty picks it by
	// the model (GatewayAPI). Only APIGateway settings have one.
	Protocol API `json:"protocol,omitzero"`
}

// SettingsPath is KOU_CONVEYOR_CONFIG, or settings.json in the user's
// configuration directory.
func SettingsPath() (string, error) {
	if path := strings.TrimSpace(os.Getenv("KOU_CONVEYOR_CONFIG")); path != "" {
		return filepath.Abs(path)
	}
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", fmt.Errorf("find the settings directory: %w; set KOU_CONVEYOR_CONFIG", err)
	}
	return filepath.Join(dir, "kou-conveyor", "settings.json"), nil
}

// LoadSettings reads settings; a missing file means the environment decides.
func LoadSettings(path string) (Settings, error) {
	data, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return Settings{}, nil
	}
	if err != nil {
		return Settings{}, fmt.Errorf("read settings: %w", err)
	}
	var s Settings
	if err := json.Unmarshal(data, &s); err != nil {
		return Settings{}, fmt.Errorf("read settings %s: %w", path, err)
	}
	s = s.normalized()
	if err := s.Validate(); err != nil {
		return Settings{}, fmt.Errorf("settings %s: %w", path, err)
	}
	return s, nil
}

// SaveSettings replaces the settings file atomically, readable only by the user.
func SaveSettings(path string, s Settings) error {
	s = s.normalized()
	if err := s.Validate(); err != nil {
		return err
	}
	data, err := json.Marshal(s, json.Deterministic(true))
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("save settings: %w", err)
	}
	file, err := os.CreateTemp(dir, ".settings-*.json")
	if err != nil {
		return fmt.Errorf("save settings: %w", err)
	}
	defer os.Remove(file.Name())
	if err := file.Chmod(0o600); err != nil {
		file.Close()
		return fmt.Errorf("save settings: %w", err)
	}
	_, err = file.Write(append(data, '\n'))
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(file.Name(), path)
	}
	if err != nil {
		return fmt.Errorf("save settings: %w", err)
	}
	return nil
}

func (s Settings) normalized() Settings {
	s.API = API(strings.ToLower(strings.TrimSpace(string(s.API))))
	s.BaseURL = strings.TrimRight(strings.TrimSpace(s.BaseURL), "/")
	s.APIKey = strings.TrimSpace(s.APIKey)
	s.Model = strings.TrimSpace(s.Model)
	s.Protocol = API(strings.ToLower(strings.TrimSpace(string(s.Protocol))))
	return s
}

// Validate reports settings the runner could not use.
func (s Settings) Validate() error {
	switch s.API {
	case APIEnvironment:
		if s != (Settings{}) {
			return errors.New("choose the Responses or Messages API before setting a base URL, key or model")
		}
		return nil
	case APIGateway:
		switch {
		case s.BaseURL != "" || s.APIKey != "":
			return errors.New("the gateway keeps its own address and key; settings for it name a model only")
		case s.Model == "":
			return errors.New("choose the model runs use through the gateway")
		case s.Protocol != "" && s.Protocol != APIResponses && s.Protocol != APIMessages:
			return fmt.Errorf("unknown protocol %q; use responses or messages", s.Protocol)
		case len(s.Model) > 256 || strings.ContainsFunc(s.Model, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }):
			return errors.New("model must be an ID without spaces")
		}
		return nil
	case APIResponses, APIMessages:
		if s.Protocol != "" {
			return errors.New("a protocol goes with the gateway only")
		}
	default:
		return fmt.Errorf("unknown provider type %q; use gateway, responses or messages", s.API)
	}
	if s.BaseURL != "" {
		parsed, err := url.Parse(s.BaseURL)
		if err != nil || (parsed.Scheme != "https" && parsed.Scheme != "http") || parsed.Host == "" ||
			parsed.RawQuery != "" || parsed.Fragment != "" || parsed.User != nil {
			return fmt.Errorf("base URL %q must be an http(s) URL such as %s", s.BaseURL, apiDefaults[s.API].BaseURL)
		}
	}
	if len(s.APIKey) > 4096 || strings.ContainsFunc(s.APIKey, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }) {
		return errors.New("API key must be a single token without spaces")
	}
	if len(s.Model) > 256 || strings.ContainsFunc(s.Model, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }) {
		return errors.New("model must be an ID without spaces")
	}
	return nil
}

// ValidModel reports a model ID the runner could not be handed: one with
// spaces or control characters, or too long to be an ID.
func ValidModel(model string) error {
	if len(model) > 256 || strings.ContainsFunc(model, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }) {
		return errors.New("model must be an ID without spaces")
	}
	return nil
}

// ErrKeyRequired reports a changed endpoint without a new key. A saved key
// only ever goes to the endpoint it was entered for, so a form that cannot
// see the key cannot redirect it elsewhere.
var ErrKeyRequired = errors.New("enter the API key again: the saved key belongs to a different endpoint")

// SettingsUpdate is a change made in a settings form. Forms never receive the
// saved key: a nil APIKey keeps it and an empty one removes it.
type SettingsUpdate struct {
	API     API     `json:"provider_type"`
	BaseURL string  `json:"base_url"`
	Model   string  `json:"model"`
	APIKey  *string `json:"api_key,omitzero"`
	// Protocol is the API runs speak through the gateway, if not the model's.
	Protocol API `json:"protocol,omitzero"`
}

// Update applies a form's change to the saved settings.
func (s Settings) Update(u SettingsUpdate) (Settings, error) {
	next := Settings{API: u.API, BaseURL: u.BaseURL, Model: u.Model}.normalized()
	switch next.API {
	case APIEnvironment:
		// The environment decides everything, so nothing else is kept.
		return Settings{}, nil
	case APIGateway:
		// The gateway has an address and a key of its own; a key saved for
		// another endpoint is dropped rather than kept for nothing.
		next = Settings{API: APIGateway, Model: next.Model, Protocol: u.Protocol}.normalized()
		return next, next.Validate()
	}
	switch {
	case u.APIKey != nil:
		next.APIKey = strings.TrimSpace(*u.APIKey)
	case s.APIKey != "" && s.endpoint() != next.endpoint():
		return s, ErrKeyRequired
	default:
		next.APIKey = s.APIKey
	}
	return next, next.Validate()
}

// endpoint is where the key is sent.
func (s Settings) endpoint() string {
	base := s.BaseURL
	if base == "" {
		base = apiDefaults[s.API].BaseURL
	}
	return string(s.API) + " " + strings.ToLower(base)
}

// KeyHint shows enough of the saved key to recognize it.
func (s Settings) KeyHint() string {
	switch {
	case s.APIKey == "":
		return ""
	case len(s.APIKey) < 12:
		return "set"
	default:
		return "…" + s.APIKey[len(s.APIKey)-4:]
	}
}

// Environment applies settings to a runner environment. Settings that name an
// API define the whole connection, so no base URL, key or model meant for
// another provider leaks into it. Unset values are set empty rather than
// removed: the runner treats empty as unset, and a present variable keeps
// the workspace .env from filling it in. Settings of APIGateway apply once
// resolved against the gateway (Through); unresolved, they leave the
// environment alone.
func (s Settings) Environment(env []string) []string {
	if s.API == APIEnvironment || s.API == APIGateway {
		return env
	}
	values := [][2]string{
		{providerVariable, apiDefaults[s.API].Provider},
		{baseURLVariable, s.BaseURL},
		{apiKeyVariable, s.APIKey},
		{modelVariable, s.Model},
	}
	env = slices.DeleteFunc(slices.Clone(env), func(entry string) bool {
		name, _, _ := strings.Cut(entry, "=")
		return slices.ContainsFunc(values, func(value [2]string) bool { return value[0] == name })
	})
	for _, value := range values {
		env = append(env, value[0]+"="+value[1])
	}
	return env
}

// Connection is where runs go, as far as a cockpit can tell before starting one.
type Connection struct {
	// Source is "flag", "settings", "gateway" or "environment".
	Source   string `json:"source"`
	Provider string `json:"provider"`
	API      API    `json:"provider_type,omitzero"`
	BaseURL  string `json:"base_url,omitzero"`
	Model    string `json:"model,omitzero"`
	// Key names where the API key comes from, empty when there is none.
	Key    string `json:"key,omitzero"`
	apiKey string
}

// ResolveConnection mirrors how the runner picks its connection: an explicit
// provider uses the environment as is, otherwise saved settings, otherwise
// the environment and the workspace .env file.
func ResolveConnection(provider string, s Settings, getenv func(string) string) Connection {
	c := Connection{Source: "environment", Provider: strings.TrimSpace(getenv(providerVariable))}
	if provider != "" {
		c.Source, c.Provider = "flag", provider
	}
	if c.Source != "flag" && s.API == APIGateway {
		// The gateway's address and key are known once a run starts.
		api := s.Protocol
		if api == "" {
			api = GatewayAPI(s.Model)
		}
		return Connection{Source: "gateway", Provider: apiDefaults[api].Provider, API: api, Model: s.Model, Key: "gateway"}
	}
	if c.Source != "flag" && s.API != APIEnvironment {
		c = Connection{Source: "settings", Provider: apiDefaults[s.API].Provider, BaseURL: s.BaseURL, Model: s.Model}
		if s.APIKey != "" {
			c.Key, c.apiKey = "settings", s.APIKey
		}
	} else {
		if c.Provider == "" {
			c.Provider = apiDefaults[APIResponses].Provider
		}
		c.BaseURL = strings.TrimSpace(getenv(baseURLVariable))
		c.Model = strings.TrimSpace(getenv(modelVariable))
	}
	for api, defaults := range apiDefaults {
		if defaults.Provider != c.Provider {
			continue
		}
		c.API = api
		if c.BaseURL == "" {
			c.BaseURL = defaults.BaseURL
		}
		if c.Model == "" {
			c.Model = defaults.Model
		}
		if c.Key == "" && c.Source != "settings" {
			if key := strings.TrimSpace(getenv(apiKeyVariable)); key != "" {
				c.Key, c.apiKey = apiKeyVariable, key
			}
		}
		if key := strings.TrimSpace(getenv(defaults.KeyVariable)); c.Key == "" && key != "" {
			c.Key, c.apiKey = defaults.KeyVariable, key
		}
	}
	return c
}

// WorkspaceEnv looks variables up like the runner: the process environment
// first, then the workspace's .env file.
func WorkspaceEnv(workspace string) func(string) string {
	file := map[string]string{}
	if data, err := os.ReadFile(filepath.Join(workspace, ".env")); err == nil {
		for line := range strings.Lines(string(data)) {
			name, value, ok := strings.Cut(strings.TrimSpace(line), "=")
			if name = strings.TrimSpace(name); ok && name != "" && !strings.HasPrefix(name, "#") {
				file[name] = strings.TrimSpace(value)
			}
		}
	}
	return func(name string) string {
		if value, ok := os.LookupEnv(name); ok {
			return value
		}
		return file[name]
	}
}

// Check is the outcome of a connection check.
type Check struct {
	OK      bool     `json:"ok"`
	Message string   `json:"message"`
	Models  []string `json:"models,omitzero"`
}

// CheckConnection asks the endpoint for its models, which proves the URL and
// the key without spending tokens.
func CheckConnection(ctx context.Context, c Connection) Check {
	if c.API == APIEnvironment {
		return Check{Message: fmt.Sprintf("Checks cover the Responses and Messages APIs; %s is configured by its environment", c.Provider)}
	}
	if c.apiKey == "" {
		return Check{Message: fmt.Sprintf("No API key: enter one or set %s", apiDefaults[c.API].KeyVariable)}
	}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	var models []string
	var err error
	if c.API == APIMessages {
		attempts := 1
		models, err = anthropic.ListModels(ctx, anthropic.Config{APIKey: c.apiKey, BaseURL: c.BaseURL, MaxAttempts: &attempts})
		if errors.Is(err, anthropic.ErrModelsUnavailable) {
			return Check{Message: "Reached the server, but it lists no models there, so the base URL and key are unverified"}
		}
	} else {
		models, err = responsesModels(ctx, c.BaseURL, c.apiKey)
	}
	if err != nil {
		return Check{Message: err.Error()}
	}
	slices.Sort(models)
	message := fmt.Sprintf("Connected; %d models available", len(models))
	if len(models) == 1 {
		message = "Connected; 1 model available"
	}
	if c.Model != "" && len(models) != 0 && !slices.Contains(models, c.Model) {
		message += fmt.Sprintf(", but %s is not among them", c.Model)
	}
	return Check{OK: true, Message: message, Models: models}
}

func responsesModels(ctx context.Context, baseURL, key string) ([]string, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(baseURL, "/")+"/models", nil)
	if err != nil {
		return nil, fmt.Errorf("invalid base URL: %w", err)
	}
	request.Header.Set("Authorization", "Bearer "+key)
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		return nil, fmt.Errorf("cannot reach the endpoint: %w", err)
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 8<<20))
	if err != nil {
		return nil, fmt.Errorf("read the model list: %w", err)
	}
	switch {
	case response.StatusCode == http.StatusUnauthorized || response.StatusCode == http.StatusForbidden:
		return nil, fmt.Errorf("the endpoint rejected the API key (%s)", response.Status)
	case response.StatusCode == http.StatusNotFound:
		return nil, fmt.Errorf("the server has no model list at %s; check the base URL, which usually ends in /v1", request.URL)
	case response.StatusCode != http.StatusOK:
		return nil, fmt.Errorf("the endpoint answered %s", response.Status)
	}
	var list struct {
		Data []struct {
			ID string `json:"id"`
		} `json:"data"`
	}
	if err := json.Unmarshal(body, &list); err != nil {
		return nil, errors.New("the endpoint's model list is not in the OpenAI format; check the base URL")
	}
	models := make([]string, 0, len(list.Data))
	for _, model := range list.Data {
		if model.ID != "" {
			models = append(models, model.ID)
		}
	}
	return models, nil
}
