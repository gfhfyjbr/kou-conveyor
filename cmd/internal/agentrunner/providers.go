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
			KeepsThinking:     true,
			CompactCeiling:    claudeCeiling,
			NewClient: func(apiKey, baseURL string, maxAttempts int, getenv func(string) string) (Client, error) {
				var maxTokens int64
				if value := strings.TrimSpace(getenv(llmMaxTokensEnvironment)); value != "" {
					parsed, err := strconv.ParseInt(value, 10, 64)
					if err != nil || parsed <= 0 {
						return nil, fmt.Errorf("%s must be a positive integer", llmMaxTokensEnvironment)
					}
					maxTokens = parsed
				}
				// How many of the latest turns keep their thinking in the
				// context (anthropic.DefaultThinkingTurns); 0 keeps all, as
				// the runner asks when it prunes (thinkingPruning).
				thinkingTurns, err := readThinkingTurns(getenv)
				if err != nil {
					return nil, err
				}
				return anthropic.NewClient(anthropic.Config{
					APIKey: apiKey, BaseURL: baseURL, MaxAttempts: &maxAttempts, MaxTokens: maxTokens, ThinkingTurns: thinkingTurns,
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

// readThinkingTurns reads KOU_CONVEYOR_THINKING_TURNS: how many of the
// latest turns keep their thinking in a Claude model's context, 0 for all;
// nil when it is not set.
func readThinkingTurns(getenv func(string) string) (*int, error) {
	value := strings.TrimSpace(getenv(thinkingTurnsEnvironment))
	if value == "" {
		return nil, nil
	}
	parsed, err := strconv.Atoi(value)
	if err != nil || parsed < 0 {
		return nil, fmt.Errorf("%s must be a whole number, or 0 to keep the thinking of every turn", thinkingTurnsEnvironment)
	}
	return &parsed, nil
}

// thinkingPruning says who leaves the thinking of older turns out of the
// context of a provider that keeps it. The API can clear it before every
// request, but each turn that clears some loses the prompt cache from there
// on. With pruning on, the runner leaves it out with a batch of pruned
// results instead, where the cached prefix changes anyway, and the API is
// asked to keep all it is sent. It returns how many of the latest turns
// keep their thinking when a batch is pruned, 0 when the runner leaves none
// out, and the environment to make the client with.
func thinkingPruning(getenv func(string) string, provider Provider, autoCompact int64) (int, func(string) string, error) {
	if !provider.KeepsThinking || autoCompact <= 0 {
		return 0, getenv, nil
	}
	turns, err := readThinkingTurns(getenv)
	if err != nil {
		return 0, nil, err
	}
	kept := anthropic.DefaultThinkingTurns
	if turns != nil {
		kept = *turns
	}
	return kept, func(name string) string {
		if name == thinkingTurnsEnvironment {
			return "0"
		}
		return getenv(name)
	}, nil
}

// compactCeiling is the most a conversation with the model grows before it
// is compacted automatically, unless configured: the provider's
// CompactCeiling, else autoCompactCeiling.
func compactCeiling(provider Provider, model string) int64 {
	if provider.CompactCeiling != nil {
		if ceiling := provider.CompactCeiling(model); ceiling > 0 {
			return ceiling
		}
	}
	return autoCompactCeiling
}

// claudeCeiling is the anthropic provider's CompactCeiling:
// claudeCompactCeiling for a Claude model. Through OpenRouter, which bills
// the reads, Claude keeps autoCompactCeiling.
func claudeCeiling(model string) int64 {
	if anthropic.ContextWindow(model) > 0 {
		return claudeCompactCeiling
	}
	return 0
}

// gatewayContextWindow knows the models a gateway names by vendor, such as
// "anthropic/claude-sonnet-4.5" or "openai/gpt-5".
func gatewayContextWindow(model string) int64 {
	if window := anthropic.ContextWindow(model); window > 0 {
		return window
	}
	return openai.ContextWindow(model)
}
