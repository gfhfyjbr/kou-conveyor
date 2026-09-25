// Package accounts runs CLIProxyAPI (github.com/router-for-me/CLIProxyAPI)
// inside the browser cockpit: a gateway on a local port that serves the
// OpenAI Responses and Anthropic Messages APIs from subscription accounts
// signed in with OAuth — Claude, Codex, Antigravity, Grok, Kimi and others —
// and spreads requests over them. Runs reach it like any other endpoint.
//
// The cockpit drives the gateway through its management API on loopback,
// with a password that lives only as long as the process: it lists the
// accounts, signs new ones in, enables, disables and removes them. It also
// keeps what the gateway itself forgets on a restart: each account's
// requests and errors over time (History), and asks providers what is left
// of an account's rate limits (Quota).
package accounts

import (
	"strings"
)

// Sign-in flows.
const (
	// FlowRedirect opens the provider's sign-in page, which sends the
	// browser back to a callback on this machine.
	FlowRedirect = "redirect"
	// FlowDevice shows a code the user enters on the provider's page.
	FlowDevice = "device"
)

// Provider is a kind of account the gateway signs in to.
type Provider struct {
	// ID is the name the gateway gives the accounts' provider.
	ID   string `json:"id"`
	Name string `json:"name"`
	// Detail says which account it takes.
	Detail string `json:"detail"`
	Flow   string `json:"flow"`
	// Quota reports whether the provider tells what is left of an
	// account's limits.
	Quota bool `json:"quota"`
	// endpoint is the management route that starts a sign-in.
	endpoint string
}

// providers are the sign-ins the gateway offers, in the order a person
// picking one expects.
var providers = []Provider{
	{ID: "claude", Name: "Claude", Detail: "Claude Pro or Max subscription, as Claude Code uses it", Flow: FlowRedirect, Quota: true, endpoint: "anthropic-auth-url"},
	{ID: "codex", Name: "Codex", Detail: "ChatGPT Plus, Pro or Business, as OpenAI Codex uses it", Flow: FlowRedirect, Quota: true, endpoint: "codex-auth-url"},
	{ID: "antigravity", Name: "Antigravity", Detail: "Google account with Antigravity: Gemini and Claude models", Flow: FlowRedirect, Quota: true, endpoint: "antigravity-auth-url"},
	{ID: "xai", Name: "Grok", Detail: "xAI account, as Grok Build uses it", Flow: FlowDevice, Quota: true, endpoint: "xai-auth-url"},
	{ID: "kimi", Name: "Kimi", Detail: "Kimi Code membership on kimi.com", Flow: FlowDevice, Quota: true, endpoint: "kimi-auth-url"},
	{ID: "kimi-ai", Name: "Kimi.ai", Detail: "Kimi Code membership on kimi.ai, outside China", Flow: FlowDevice, Quota: true, endpoint: "kimi-ai-auth-url"},
	{ID: "devin", Name: "Devin", Detail: "Devin account", Flow: FlowRedirect, endpoint: "devin-auth-url"},
	{ID: "meta", Name: "Meta", Detail: "Meta AI account", Flow: FlowDevice, endpoint: "meta-auth-url"},
}

// Providers lists the sign-ins the gateway offers.
func Providers() []Provider { return append([]Provider(nil), providers...) }

// ProviderByID finds a sign-in by provider ID; "anthropic" is Claude.
func ProviderByID(id string) (Provider, bool) {
	id = strings.ToLower(strings.TrimSpace(id))
	if id == "anthropic" {
		id = "claude"
	}
	for _, p := range providers {
		if p.ID == id {
			return p, true
		}
	}
	return Provider{}, false
}

// providerNames are how the cockpit names the providers accounts can come
// from, including those only an uploaded credential or the gateway's
// configuration adds.
var providerNames = map[string]string{
	"claude": "Claude", "codex": "Codex", "antigravity": "Antigravity", "xai": "Grok",
	"kimi": "Kimi", "kimi-ai": "Kimi.ai", "devin": "Devin", "meta": "Meta", "openai": "OpenAI",
	"gemini": "Gemini", "gemini-cli": "Gemini CLI", "aistudio": "AI Studio", "vertex": "Vertex AI",
	"qwen": "Qwen", "iflow": "iFlow", "openai-compatibility": "OpenAI-compatible",
}

// ProviderName is a provider's name for people.
func ProviderName(id string) string {
	if name, ok := providerNames[strings.ToLower(id)]; ok {
		return name
	}
	if id == "" {
		return "Unknown"
	}
	return strings.ToUpper(id[:1]) + id[1:]
}
