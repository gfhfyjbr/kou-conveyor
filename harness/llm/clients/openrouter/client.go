package openrouter

import (
	"encoding/json/jsontext"
	"errors"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/responsesapi"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

type Config struct {
	APIKey      string
	BaseURL     string
	MaxAttempts *int
	// CacheTTL is 5m or 1h for explicit prompt-cache breakpoints. Empty keeps
	// the one-hour default.
	CacheTTL string
	Trace    func(Exchange)
}

type Exchange = responsesapi.Exchange

type Client struct {
	llm.Adapter
	remote *primitives.RemoteClient
}

var _ llm.Adapter = (*Client)(nil)

func NewClient(config Config) (*Client, error) {
	if strings.TrimSpace(config.APIKey) == "" {
		return nil, errors.New("OpenRouter API key must be set")
	}
	baseURL := strings.TrimRight(strings.TrimSpace(config.BaseURL), "/")
	if baseURL == "" {
		return nil, errors.New("OpenRouter base URL must be set")
	}
	cacheTTL := strings.ToLower(strings.TrimSpace(config.CacheTTL))
	if cacheTTL == "" {
		cacheTTL = "1h"
	}
	if cacheTTL != "5m" && cacheTTL != "1h" {
		return nil, errors.New("cache TTL must be 5m or 1h")
	}

	remote := primitives.NewRemoteClient()
	adapter, err := responsesapi.NewAdapter(remote, responsesapi.Config{
		Endpoint: baseURL + "/responses",
		Headers: map[string][]string{
			"Authorization": {"Bearer " + config.APIKey},
			"Content-Type":  {"application/json"},
		},
		Trace:             config.Trace,
		MaxAttempts:       config.MaxAttempts,
		CacheKeyPlacement: responsesapi.CacheKeyPlacement{Header: "x-session-id"},
		// OpenRouter only caches prompts for upstreams that need explicit breakpoints
		// (Anthropic among them) when the request opts in; the top-level field places
		// the breakpoint on the last cacheable block and moves it forward each turn.
		// With the x-session-id header, every turn of a session lands on one warm upstream.
		// The one-hour default survives the gaps an agent session produces (a long
		// tool call, a many-minute reasoning turn, the tool heartbeat). Five minutes
		// is available when cheaper writes matter more than keeping a cache warm.
		Extensions: map[string]jsontext.Value{
			"cache_control": jsontext.Value(`{"type":"ephemeral","ttl":"` + cacheTTL + `"}`),
		},
	})
	if err != nil {
		_ = remote.Close()
		return nil, err
	}
	return &Client{Adapter: adapter, remote: remote}, nil
}

func (client *Client) Close() error {
	return client.remote.Close()
}
