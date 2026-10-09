//go:build darwin || linux

package agentrunner

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// An interrupted runner must not exit while a tool it started is still
// running in its own process group, even a tool that ignores SIGTERM.
func TestRunMainStopsToolsBeforeReturning(t *testing.T) {
	pidFile := filepath.Join(t.TempDir(), "tool.pid")
	client := &fakeClient{}
	client.respond = func(ctx context.Context, _ llm.Request) (llm.Response, error) {
		client.mu.Lock()
		client.calls++
		first := client.calls == 1
		client.mu.Unlock()
		if !first {
			<-ctx.Done()
			return llm.Response{}, ctx.Err()
		}
		return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemToolCall,
			Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"trap '' TERM; echo $$ > ` + pidFile + `; sleep 60"}`},
		}}}, nil
	}
	ctx, interrupt := context.WithCancel(t.Context())
	defer interrupt()
	go func() {
		for ctx.Err() == nil {
			if data, err := os.ReadFile(pidFile); err == nil && len(bytes.TrimSpace(data)) > 0 {
				interrupt() // what SIGINT does to the runner's context
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
	}()
	var stdout, stderr bytes.Buffer
	code := RunMain(ctx,
		[]string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()},
		func(name string) string {
			switch name {
			case llmAPIKeyEnvironment:
				return "secret"
			case "SHELL":
				return "/bin/sh"
			}
			return ""
		},
		func() []string { return []string{"PATH=/usr/bin:/bin", "KOU_CONVEYOR_LLM_API_KEY=secret"} },
		strings.NewReader(`{"messages":[{"role":"user","content":"run it"}],"model":"gpt-test"}`),
		&stdout, &stderr, testConfig(client))
	if code != 130 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	data, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		t.Fatal(err)
	}
	if syscall.Kill(pid, 0) == nil {
		syscall.Kill(-pid, syscall.SIGKILL)
		t.Fatal("the runner returned while its tool was still running")
	}
}

// Saved keys reach the runner in KOU_CONVEYOR_LLM_API_KEY; the commands the
// agent runs must not be able to read it back.
func TestToolsDoNotInheritTheHarnessKey(t *testing.T) {
	t.Setenv(llmAPIKeyEnvironment, "secret")
	var seen string
	client := &fakeClient{}
	client.respond = func(ctx context.Context, request llm.Request) (llm.Response, error) {
		client.mu.Lock()
		client.calls++
		first := client.calls == 1
		client.mu.Unlock()
		if first {
			return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
				Type: llm.ItemToolCall,
				Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"printf 'key=%s' \"${KOU_CONVEYOR_LLM_API_KEY-unset}\""}`},
			}}}, nil
		}
		for _, item := range request.Input {
			if result, ok := item.Data.(llm.ToolResult); ok {
				for _, part := range result.Output {
					if strings.Contains(part.Value, "key=") {
						client.mu.Lock()
						seen = part.Value
						client.mu.Unlock()
					}
				}
			}
		}
		return llm.Response{ID: "response-2", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"},
		}}}, nil
	}
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(),
		[]string{"-workspace", t.TempDir(), "-session-directory", t.TempDir()},
		func(name string) string {
			switch name {
			case llmAPIKeyEnvironment:
				return os.Getenv(name)
			case "SHELL":
				return "/bin/sh"
			}
			return ""
		},
		os.Environ,
		strings.NewReader(`{"messages":[{"role":"user","content":"print the key"}],"model":"gpt-test"}`),
		&stdout, &stderr, testConfig(client))
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	client.mu.Lock()
	defer client.mu.Unlock()
	if !strings.Contains(seen, "key=unset") {
		t.Fatalf("the tool saw %q", seen)
	}
	if os.Getenv(llmAPIKeyEnvironment) != "secret" {
		t.Fatal("the runner did not restore its environment")
	}
}
