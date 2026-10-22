package agentrunner

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestARunWorksInTheFolderItIsGiven(t *testing.T) {
	run := func(t *testing.T, workspace, workdir string) (int, string) {
		t.Helper()
		client := &fakeClient{}
		client.respond = func(_ context.Context, _ llm.Request) (llm.Response, error) {
			client.mu.Lock()
			defer client.mu.Unlock()
			client.calls++
			if client.calls == 1 {
				return llm.Response{
					ID: "response-1", Stop: llm.StopComplete,
					Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
						CallID: "call-1", Name: "Bash", Arguments: `{"command":"printf here > marker"}`,
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
		var stdout, stderr bytes.Buffer
		code := RunMain(
			ctx,
			[]string{"-workspace", workspace, "-session-directory", t.TempDir()},
			func(name string) string {
				switch name {
				case llmAPIKeyEnvironment:
					return "secret"
				case "SHELL":
					return "/bin/sh"
				case runconfig.WorkdirEnvironment:
					return workdir
				}
				return ""
			},
			func() []string { return []string{"PATH=/usr/bin:/bin", "KOU_CONVEYOR_LLM_API_KEY=secret"} },
			strings.NewReader(`{"messages":[{"role":"user","content":"run it"}],"model":"gpt-test"}`),
			&stdout,
			&stderr,
			testConfig(client),
		)
		return code, stderr.String()
	}

	workspace, workdir := t.TempDir(), t.TempDir()
	if code, stderr := run(t, workspace, workdir); code != 0 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr)
	}
	if data, err := os.ReadFile(filepath.Join(workdir, "marker")); err != nil || string(data) != "here" {
		t.Fatalf("the command did not work in the folder: %q, %v", data, err)
	}
	if _, err := os.Stat(filepath.Join(workspace, "marker")); err == nil {
		t.Fatal("the command worked in the workspace")
	}

	// A folder that is none, or not absolute, is said rather than ignored.
	for _, bad := range []string{"relative", filepath.Join(workspace, "missing")} {
		if code, stderr := run(t, workspace, bad); code == 0 || !strings.Contains(stderr, runconfig.WorkdirEnvironment) {
			t.Fatalf("%s: exit = %d, stderr = %q", bad, code, stderr)
		}
	}
}
