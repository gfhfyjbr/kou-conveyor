package agentrunner

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/anthropic"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/fireworks"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/ollama"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openai"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openaicodex"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openrouter"
)

func DefaultProviders() []Provider {
	return []Provider{
		{
			Name:    "ollama",
			BaseURL: ollama.BaseURL,
			NewClient: func(_, baseURL string, maxAttempts int, _ func(string) string) (Client, error) {
				return ollama.NewClient(ollama.Config{BaseURL: baseURL, MaxAttempts: &maxAttempts})
			},
		},
		{
			Name:              "openai",
			BaseURL:           "https://api.openai.com/v1",
			DefaultModel:      "gpt-6-astra",
			APIKeyEnvironment: "OPENAI_API_KEY",
			ContextWindow:     openai.ContextWindow,
			NewClient: func(apiKey, baseURL string, maxAttempts int, _ func(string) string) (Client, error) {
				return openai.NewClient(openai.Config{APIKey: apiKey, BaseURL: baseURL, MaxAttempts: &maxAttempts})
			},
		},
		{
			// Anthropic's Messages API, and gateways that implement it.
			Name:              "anthropic",
			BaseURL:           anthropic.BaseURL,
			DefaultModel:      anthropic.DefaultModel,
			APIKeyEnvironment: "ANTHROPIC_API_KEY",
			ContextWindow:     anthropic.ContextWindow,
			NewClient: func(apiKey, baseURL string, maxAttempts int, getenv func(string) string) (Client, error) {
				var maxTokens int64
				if value := strings.TrimSpace(getenv(llmMaxTokensEnvironment)); value != "" {
					parsed, err := strconv.ParseInt(value, 10, 64)
					if err != nil || parsed <= 0 {
						return nil, fmt.Errorf("%s must be a positive integer", llmMaxTokensEnvironment)
					}
					maxTokens = parsed
				}
				return anthropic.NewClient(anthropic.Config{
					APIKey: apiKey, BaseURL: baseURL, MaxAttempts: &maxAttempts, MaxTokens: maxTokens,
				})
			},
		},
		{
			Name:          "openai-codex",
			BaseURL:       openaicodex.BaseURL,
			ContextWindow: openai.ContextWindow,
			NewClient: func(_, baseURL string, maxAttempts int, getenv func(string) string) (Client, error) {
				config, err := openaicodex.EnvironmentConfig(getenv)
				if err != nil {
					return nil, err
				}
				config.BaseURL, config.MaxAttempts = baseURL, &maxAttempts
				return openaicodex.NewClient(config)
			},
		},

		{
			Name:              "openrouter",
			BaseURL:           "https://openrouter.ai/api/v1",
			APIKeyEnvironment: "OPENROUTER_API_KEY",
			ContextWindow:     gatewayContextWindow,
			NewClient: func(apiKey, baseURL string, maxAttempts int, _ func(string) string) (Client, error) {
				return openrouter.NewClient(openrouter.Config{APIKey: apiKey, BaseURL: baseURL, MaxAttempts: &maxAttempts})
			},
		},
		{
			Name:              "fireworks",
			BaseURL:           "https://api.fireworks.ai/inference/v1",
			APIKeyEnvironment: "FIREWORKS_API_KEY",
			NewClient: func(apiKey, baseURL string, maxAttempts int, _ func(string) string) (Client, error) {
				return fireworks.NewClient(fireworks.Config{APIKey: apiKey, BaseURL: baseURL, MaxAttempts: &maxAttempts})
			},
		},
	}
}

// gatewayContextWindow knows the models a gateway names by vendor, such as
// "anthropic/claude-sonnet-4.5" or "openai/gpt-5".
func gatewayContextWindow(model string) int64 {
	if window := anthropic.ContextWindow(model); window > 0 {
		return window
	}
	return openai.ContextWindow(model)
}
