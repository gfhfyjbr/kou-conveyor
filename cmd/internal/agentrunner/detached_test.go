package agentrunner

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// A detached run reads stdin, a file, past its end: the stop line appended
// to it while the run goes on stops the run, and the last line of the output
// says how the run ended.
func TestDetachedRunFollowsItsInputFile(t *testing.T) {
	workspace, sessions := t.TempDir(), t.TempDir()
	pidFile := filepath.Join(t.TempDir(), "tool.pid")
	inPath := filepath.Join(t.TempDir(), "in.jsonl")
	request := `{"session_id":"detached","messages":[{"content":"run it"}],"model":"gpt-test"}` + "\n"
	if err := os.WriteFile(inPath, []byte(request), 0o600); err != nil {
		t.Fatal(err)
	}
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
			Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"echo $$ > ` + pidFile + `; sleep 60"}`},
		}}}, nil
	}
	go func() {
		for t.Context().Err() == nil {
			if data, err := os.ReadFile(pidFile); err == nil && len(bytes.TrimSpace(data)) > 0 {
				// Appended as a front-end appends, by another open file.
				f, err := os.OpenFile(inPath, os.O_WRONLY|os.O_APPEND, 0)
				if err != nil {
					t.Error(err)
					return
				}
				f.WriteString(`{"stop":{"reason":"the user stopped the run from the cockpit"}}` + "\n")
				f.Close()
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
	}()
	stdin, err := os.Open(inPath)
	if err != nil {
		t.Fatal(err)
	}
	defer stdin.Close()
	env := func() []string { return []string{"PATH=/usr/bin:/bin"} }
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(), []string{"-workspace", workspace, "-session-directory", sessions, "-steer", "-detached"},
		steerEnv, env, stdin, &stdout, &stderr, testConfig(client))
	if code != 130 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	lines := strings.Split(strings.TrimSuffix(stdout.String(), "\n"), "\n")
	if last := lines[len(lines)-1]; last != `{"type":"exit","code":130}` {
		t.Fatalf("last line = %q", last)
	}
	output := []byte(strings.Join(lines[:len(lines)-1], "\n") + "\n")
	if reasons := stopReasons(t, output); !slices.Equal(reasons, []string{"the user stopped the run from the cockpit"}) {
		t.Fatalf("recorded stops = %q", reasons)
	}
}

// A detached run that ends on its own says so last, and stops reading its
// input file.
func TestDetachedRunSaysHowItEnded(t *testing.T) {
	inPath := filepath.Join(t.TempDir(), "in.jsonl")
	if err := os.WriteFile(inPath, []byte(`{"session_id":"ended","messages":[{"content":"hello"}],"model":"gpt-test"}`+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	client := &fakeClient{}
	client.respond = func(context.Context, llm.Request) (llm.Response, error) {
		return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Hi."},
		}}}, nil
	}
	stdin, err := os.Open(inPath)
	if err != nil {
		t.Fatal(err)
	}
	defer stdin.Close()
	var stdout, stderr bytes.Buffer
	done := make(chan int, 1)
	go func() {
		done <- RunMain(t.Context(), []string{"-workspace", t.TempDir(), "-session-directory", t.TempDir(), "-steer", "-detached"},
			steerEnv, func() []string { return nil }, stdin, &stdout, &stderr, testConfig(client))
	}()
	select {
	case code := <-done:
		if code != 0 {
			t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
		}
	case <-time.After(20 * time.Second):
		t.Fatal("the run did not end at the end of its input")
	}
	if !strings.HasSuffix(stdout.String(), "\n"+`{"type":"exit","code":0}`+"\n") {
		t.Fatalf("output = %q", stdout.String())
	}
}

func TestDetachedArgs(t *testing.T) {
	for _, test := range []struct {
		args []string
		want bool
	}{
		{[]string{"-steer", "-detached"}, true},
		{[]string{"--detached", "-steer"}, true},
		{[]string{"-detached=true"}, true},
		{[]string{"-detached=false"}, false},
		{[]string{"-steer"}, false},
		{[]string{"--", "-detached"}, false},
		{[]string{"-detachedx"}, false},
	} {
		if got := detachedArgs(test.args); got != test.want {
			t.Errorf("detachedArgs(%q) = %v, want %v", test.args, got, test.want)
		}
	}
}
