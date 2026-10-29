package agentrunner

import (
	"context"
	"encoding/json/v2"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/anthropic"
)

func TestRunnerProviderRetries(t *testing.T) {
	for _, provider := range DefaultProviders() {
		for _, maxAttempts := range []int{1, 2} {
			t.Run(provider.Name+"/"+strconv.Itoa(maxAttempts), func(t *testing.T) {
				t.Parallel()
				var attempts atomic.Int64
				server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
					var body struct {
						MaxAttempts *int `json:"max_attempts"`
					}
					if err := json.UnmarshalRead(request.Body, &body); err != nil {
						t.Error(err)
					}
					if body.MaxAttempts != nil {
						t.Error("harness retry configuration leaked into provider request")
					}
					attempts.Add(1)
					writer.WriteHeader(http.StatusServiceUnavailable)
				}))
				defer server.Close()
				ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
				defer cancel()
				code := RunMain(ctx,
					[]string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()},
					func(name string) string {
						switch name {
						case "KOU_CONVEYOR_LLM_PROVIDER":
							return provider.Name
						case "KOU_CONVEYOR_LLM_BASE_URL":
							return server.URL
						case "OPENAI_CODEX_ACCESS_TOKEN":
							return "subscription-token"
						case "OPENAI_CODEX_ACCOUNT_ID":
							return "account-1"
						case "KOU_CONVEYOR_LLM_API_KEY":
							return "test-key"
						default:
							return ""
						}
					}, func() []string { return nil },
					strings.NewReader(`{"prompt":"hello","model":"test","max_attempts":`+strconv.Itoa(maxAttempts)+`}`),
					io.Discard, io.Discard, Config{Name: "kou-conveyor-runner", ParseRequest: parseTestRequest, Providers: DefaultProviders()})
				if code != 1 || attempts.Load() != int64(maxAttempts) {
					t.Fatalf("exit = %d, attempts = %d, want %d", code, attempts.Load(), maxAttempts)
				}
			})
		}
	}
}

func TestRunnerProviderDefaultModels(t *testing.T) {
	defaults := map[string]string{"openai": "gpt-6-astra", "anthropic": "claude-opus-5"}
	for _, provider := range DefaultProviders() {
		want := defaults[provider.Name]
		if provider.DefaultModel != want {
			t.Errorf("%s default model = %q, want %q", provider.Name, provider.DefaultModel, want)
		}
	}
}

// TestRunnerAnthropicThinkingTurns checks where KOU_CONVEYOR_THINKING_TURNS
// goes. With pruning on, as by default, the runner leaves the thinking of
// older turns out with a batch of pruned results, and the request asks the
// API to keep all it is sent; with automatic compaction off nothing is
// pruned, and the request's clear_thinking edit keeps the latest turns' (2
// by default, all with 0). A value that is not a whole number stops the run.
func TestRunnerAnthropicThinkingTurns(t *testing.T) {
	const all = `{"edits":[{"keep":"all","type":"clear_thinking_20251015"}]}`
	latest := func(turns int) string {
		return `{"edits":[{"keep":{"type":"thinking_turns","value":` + strconv.Itoa(turns) + `},"type":"clear_thinking_20251015"}]}`
	}
	for _, test := range []struct {
		value, autoCompact string
		want               string // the request's context_management; "" refuses the value
	}{
		{"", "", all}, {"3", "", all}, {"0", "", all},
		{"", "off", latest(2)}, {"3", "off", latest(3)}, {"0", "off", all},
		{"-1", "", ""}, {"two", "off", ""},
	} {
		t.Run("turns="+test.value+",compact="+test.autoCompact, func(t *testing.T) {
			t.Parallel()
			bodies := make(chan []byte, 4)
			server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
				data, _ := io.ReadAll(request.Body)
				bodies <- data
				writer.Header().Set("Content-Type", "application/json")
				writer.WriteHeader(http.StatusUnauthorized)
				_, _ = io.WriteString(writer, `{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}`)
			}))
			defer server.Close()
			var stderr strings.Builder
			code := RunMain(t.Context(), []string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()}, func(name string) string {
				return map[string]string{
					"KOU_CONVEYOR_LLM_PROVIDER": "anthropic",
					"KOU_CONVEYOR_LLM_BASE_URL": server.URL,
					"KOU_CONVEYOR_LLM_API_KEY":  "test-key",
					thinkingTurnsEnvironment:    test.value,
					autoCompactEnvironment:      test.autoCompact,
				}[name]
			}, func() []string { return nil }, strings.NewReader(`{"prompt":"hello","model":"claude-opus-5","max_attempts":1}`),
				io.Discard, &stderr, Config{Name: "kou-conveyor-runner", ParseRequest: parseTestRequest, Providers: DefaultProviders()})
			if code == 0 {
				t.Fatal("the run succeeded against a server that refuses it")
			}
			if test.want == "" {
				if len(bodies) != 0 || !strings.Contains(stderr.String(), thinkingTurnsEnvironment) {
					t.Fatalf("%q was accepted: %d requests, stderr %q", test.value, len(bodies), stderr.String())
				}
				return
			}
			if len(bodies) == 0 {
				t.Fatalf("no request; stderr %q", stderr.String())
			}
			data := <-bodies
			var body map[string]any
			if err := json.Unmarshal(data, &body); err != nil {
				t.Fatal(err)
			}
			edits, err := json.Marshal(body["context_management"], json.Deterministic(true))
			if err != nil {
				t.Fatal(err)
			}
			if string(edits) != test.want {
				t.Fatalf("context_management = %s, want %s", edits, test.want)
			}
		})
	}
}

// TestThinkingPruning checks that the runner leaves the thinking of older
// turns out itself only for a provider that keeps it and only with pruning
// on, and that the client then keeps all of it while reading the rest of
// the environment as it is.
func TestThinkingPruning(t *testing.T) {
	keeps := Provider{Name: "anthropic", KeepsThinking: true}
	for _, test := range []struct {
		provider    Provider
		value       string
		autoCompact int64
		turns       int    // kept when a batch is pruned
		client      string // the client's KOU_CONVEYOR_THINKING_TURNS
	}{
		{keeps, "", 160_000, anthropic.DefaultThinkingTurns, "0"},
		{keeps, "3", 160_000, 3, "0"},
		{keeps, "0", 160_000, 0, "0"},
		{keeps, "3", 0, 0, "3"},
		{Provider{Name: "openai"}, "3", 160_000, 0, "3"},
	} {
		getenv := func(name string) string {
			return map[string]string{thinkingTurnsEnvironment: test.value, llmMaxTokensEnvironment: "4096"}[name]
		}
		turns, environment, err := thinkingPruning(getenv, test.provider, test.autoCompact)
		if err != nil || turns != test.turns || environment(thinkingTurnsEnvironment) != test.client || environment(llmMaxTokensEnvironment) != "4096" {
			t.Errorf("%s, %q, %d: turns %d, client %q, err %v", test.provider.Name, test.value, test.autoCompact, turns, environment(thinkingTurnsEnvironment), err)
		}
	}
	if _, _, err := thinkingPruning(func(string) string { return "two" }, keeps, 160_000); err == nil {
		t.Error("a value that is not a whole number was accepted")
	}
}

func TestRunnerCodexUsesSubscriptionWithoutAPIKey(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer subscription-token" || r.Header.Get("ChatGPT-Account-ID") != "account" {
			t.Error("wrong authentication")
		}
		var body struct {
			Stream bool `json:"stream"`
		}
		if err := json.UnmarshalRead(r.Body, &body); err != nil {
			t.Error(err)
		}
		if !body.Stream {
			t.Errorf("body = %#v", body)
		}
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"output\":[{\"id\":\"msg-1\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"subscription works\"}]}]}}\n\n")
	}))
	defer server.Close()
	var output, stderr strings.Builder
	code := RunMain(t.Context(), []string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()}, func(key string) string {
		return map[string]string{
			"KOU_CONVEYOR_LLM_PROVIDER": "openai-codex",
			"KOU_CONVEYOR_LLM_BASE_URL": server.URL,
			"OPENAI_CODEX_ACCESS_TOKEN": "subscription-token",
			"OPENAI_CODEX_ACCOUNT_ID":   "account",
		}[key]
	}, func() []string { return nil }, strings.NewReader(`{"prompt":"hello","model":"gpt-test","system_prompt":"my system prompt"}`), &output, &stderr, Config{Name: "kou-conveyor-runner", ParseRequest: parseTestRequest, Providers: DefaultProviders()})
	if code != 0 || !strings.Contains(output.String(), "subscription works") {
		t.Fatalf("exit = %d, stderr = %s", code, stderr.String())
	}
	if strings.Contains(output.String(), "subscription-token") {
		t.Fatal("credential leaked into session output")
	}
}

func TestRunnerListsItsProviders(t *testing.T) {
	var output strings.Builder
	code := RunMain(t.Context(), []string{"-providers"}, func(string) string { return "" }, func() []string { return nil },
		strings.NewReader(""), &output, io.Discard, Config{Name: "kou-conveyor-runner", ParseRequest: parseTestRequest, Providers: DefaultProviders()})
	if code != 0 || !strings.Contains(output.String(), "\nanthropic\n") || !strings.HasPrefix(output.String(), "ollama\n") {
		t.Fatalf("exit %d, output %q", code, output.String())
	}
}
