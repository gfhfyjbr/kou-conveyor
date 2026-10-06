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

// TestRunnerAnthropicThinkingTurns checks that KOU_CONVEYOR_THINKING_TURNS
// reaches the Messages API's clear_thinking edit: 2 turns by default, none
// with 0, and a value that is not a whole number stops the run.
func TestRunnerAnthropicThinkingTurns(t *testing.T) {
	for _, test := range []struct {
		value string
		want  int // turns kept; 0 for no edit, -1 for a refused value
	}{{"", 2}, {"3", 3}, {"0", 0}, {"-1", -1}, {"two", -1}} {
		t.Run("turns="+test.value, func(t *testing.T) {
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
				}[name]
			}, func() []string { return nil }, strings.NewReader(`{"prompt":"hello","model":"claude-opus-5","max_attempts":1}`),
				io.Discard, &stderr, Config{Name: "kou-conveyor-runner", ParseRequest: parseTestRequest, Providers: DefaultProviders()})
			if code == 0 {
				t.Fatal("the run succeeded against a server that refuses it")
			}
			if test.want < 0 {
				if len(bodies) != 0 || !strings.Contains(stderr.String(), thinkingTurnsEnvironment) {
					t.Fatalf("%q was accepted: %d requests, stderr %q", test.value, len(bodies), stderr.String())
				}
				return
			}
			if len(bodies) == 0 {
				t.Fatalf("no request; stderr %q", stderr.String())
			}
			data := <-bodies
			var body struct {
				ContextManagement *struct {
					Edits []struct {
						Type string `json:"type"`
						Keep struct {
							Value int `json:"value"`
						} `json:"keep"`
					} `json:"edits"`
				} `json:"context_management"`
			}
			if err := json.Unmarshal(data, &body); err != nil {
				t.Fatal(err)
			}
			switch edits := body.ContextManagement; {
			case test.want == 0 && edits != nil:
				t.Fatalf("0 keeps all thinking, but the request clears it: %s", data)
			case test.want > 0 && (edits == nil || len(edits.Edits) != 1 || edits.Edits[0].Type != "clear_thinking_20251015" || edits.Edits[0].Keep.Value != test.want):
				t.Fatalf("the request does not keep the thinking of %d turns: %s", test.want, data)
			}
		})
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
