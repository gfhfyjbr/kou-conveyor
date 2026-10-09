package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// The cockpits edit a prompt by rewinding its session to before it. The
// runner must continue a rewound session as if the prompt had never been
// sent: the earlier exchange and its command results are in context, and
// nothing of the removed run is.
func TestRunContinuesARewoundSession(t *testing.T) {
	workspace, sessions := t.TempDir(), t.TempDir()
	ctx, cancel := context.WithTimeout(t.Context(), 20*time.Second)
	defer cancel()
	var requests []llm.Request
	client := &fakeClient{}
	// Every prompt gets one command, then an answer.
	client.respond = func(_ context.Context, request llm.Request) (llm.Response, error) {
		client.mu.Lock()
		defer client.mu.Unlock()
		client.calls++
		requests = append(requests, request)
		if last := request.Input[len(request.Input)-1]; last.Type == llm.ItemMessage {
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
				CallID: fmt.Sprintf("call-%d", client.calls), Name: "Bash", Arguments: fmt.Sprintf(`{"command":"printf result-%d"}`, client.calls),
			}}}}, nil
		}
		return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{
			Role: llm.RoleAssistant, Text: fmt.Sprintf("answer-%d", client.calls),
		}}}}, nil
	}
	run := func(messageID, prompt string) {
		t.Helper()
		var stdout, stderr bytes.Buffer
		request := fmt.Sprintf(`{"session_id":"edited","messages":[{"role":"user","content":%q,"message_id":%q}]}`, prompt, messageID)
		code := RunMain(ctx, []string{"-workspace", workspace, "-session-directory", sessions},
			func(name string) string {
				switch name {
				case llmAPIKeyEnvironment:
					return "secret"
				case "SHELL":
					return "/bin/sh"
				}
				return ""
			}, func() []string { return []string{"PATH=/usr/bin:/bin"} },
			strings.NewReader(request), &stdout, &stderr, testConfig(client))
		if code != 0 {
			t.Fatalf("%q: exit %d: %s", prompt, code, stderr.String())
		}
	}
	commands := func() int {
		t.Helper()
		entries, err := os.ReadDir(filepath.Join(sessions, "operations", "edited"))
		if err != nil {
			t.Fatal(err)
		}
		return len(entries)
	}

	run("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b001", "first question")
	run("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b002", "second question")
	if n := commands(); n != 2 {
		t.Fatalf("command output directories = %d, want 2", n)
	}
	if err := cockpit.RewindSession(sessions, "edited", "0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b002"); err != nil {
		t.Fatal(err)
	}
	if n := commands(); n != 1 {
		t.Fatalf("command output directories after the rewind = %d, want 1", n)
	}
	requests = nil
	run("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b003", "second question, edited")
	if len(requests) != 2 {
		t.Fatalf("model calls for the edited prompt = %d, want 2", len(requests))
	}
	want := "user: first question | call: printf result-1 | result: result-1 | assistant: answer-2 | user: second question, edited"
	if got := describeInput(requests[0].Input); got != want {
		t.Fatalf("context of the edited prompt:\n got %s\nwant %s", got, want)
	}

	// The edited run is a run like any other: it can be rewound and edited again.
	if err := cockpit.RewindSession(sessions, "edited", "0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b003"); err != nil {
		t.Fatal(err)
	}
	requests = nil
	run("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b004", "second question, edited twice")
	if got := describeInput(requests[0].Input); !strings.HasSuffix(got, "assistant: answer-2 | user: second question, edited twice") {
		t.Fatalf("context of the prompt edited twice: %s", got)
	}
}

// describeInput summarizes the conversation part of a model request.
func describeInput(items []llm.Item) string {
	var parts []string
	for _, item := range items {
		switch data := item.Data.(type) {
		case llm.Message:
			if data.Role != llm.RoleSystem {
				parts = append(parts, fmt.Sprintf("%s: %s", data.Role, data.Text))
			}
		case llm.ToolCall:
			var arguments struct {
				Command string `json:"command"`
			}
			_ = json.Unmarshal([]byte(data.Arguments), &arguments)
			parts = append(parts, "call: "+arguments.Command)
		case llm.ToolResult:
			var text []string
			for _, output := range data.Output {
				text = append(text, output.Value)
			}
			parts = append(parts, "result: "+strings.Join(text, ""))
		}
	}
	return strings.Join(parts, " | ")
}
