package agentrunner

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A run writes a tool's output to its session file once: the status it
// records as the tool finishes names the operation the file has saved, while
// its own output has the status whole.
func TestRunWritesAToolsOutputOnce(t *testing.T) {
	client := &fakeClient{}
	client.respond = func(_ context.Context, _ llm.Request) (llm.Response, error) {
		client.mu.Lock()
		defer client.mu.Unlock()
		client.calls++
		if client.calls == 1 {
			return llm.Response{
				ID: "response-1", Stop: llm.StopComplete,
				Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
					CallID: "call-1", Name: "Bash", Arguments: `{"command":"printf '%s-%s' tool once"}`,
				}}},
			}, nil
		}
		return llm.Response{
			ID: "response-2", Stop: llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}},
		}, nil
	}
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	sessions := t.TempDir()
	var stdout, stderr bytes.Buffer
	code := RunMain(
		ctx,
		[]string{"-workspace", t.TempDir(), "-session-directory", sessions},
		func(name string) string {
			switch name {
			case llmAPIKeyEnvironment:
				return "secret"
			case "SHELL":
				return "/bin/sh"
			}
			return ""
		},
		func() []string { return []string{"PATH=/usr/bin:/bin"} },
		strings.NewReader(`{"messages":[{"role":"user","content":"run it"}],"model":"gpt-test"}`),
		&stdout,
		&stderr,
		testConfig(client),
	)
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	paths, err := filepath.Glob(filepath.Join(sessions, "*.session.jsonl"))
	if err != nil || len(paths) != 1 {
		t.Fatalf("session files = %v, %v", paths, err)
	}
	encoded, err := os.ReadFile(paths[0])
	if err != nil {
		t.Fatal(err)
	}
	if count := bytes.Count(encoded, []byte("tool-once")); count != 1 {
		t.Fatalf("the output is written %d times:\n%s", count, encoded)
	}
	named := false
	for line := range bytes.Lines(encoded) {
		if bytes.Contains(line, []byte(`"Kind":"tool_call_status"`)) && bytes.Contains(line, []byte(`"Operations":["`)) {
			named = true
		}
	}
	if !named {
		t.Fatalf("no status names its operation:\n%s", encoded)
	}
	if !strings.Contains(stdout.String(), "tool-once") {
		t.Fatalf("the run's output has no result:\n%s", stdout.String())
	}
}
