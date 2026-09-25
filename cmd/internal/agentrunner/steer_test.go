//go:build darwin || linux

package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"io"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// With -steer, a message written to stdin while the model answers waits for
// the answer's tool call, and the next request carries it after the result.
func TestRunnerSteersTheRunningAgent(t *testing.T) {
	stdin, feed := io.Pipe()
	defer feed.Close()
	answering := make(chan struct{})
	client := &fakeClient{}
	var second llm.Request
	client.respond = func(ctx context.Context, request llm.Request) (llm.Response, error) {
		client.mu.Lock()
		client.calls++
		call := client.calls
		client.mu.Unlock()
		switch call {
		case 1:
			close(answering)
			// The message reaches the runner while this answer is written.
			time.Sleep(300 * time.Millisecond)
			return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
				Type: llm.ItemToolCall,
				Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"echo tested"}`},
			}}}, nil
		default:
			second = request
			return llm.Response{ID: "response-2", Stop: llm.StopComplete, Output: []llm.Item{{
				Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"},
			}}}, nil
		}
	}
	go func() {
		io.WriteString(feed, `{"messages":[{"content":"run the tests"}],"model":"gpt-test"}`+"\n")
		<-answering
		io.WriteString(feed, `{"content":"not json`+"\n") // reported, and the stream is over
	}()
	// The first line must not be taken for the whole request: a malformed
	// message later on is reported without failing the run.
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(),
		[]string{"-workspace", t.TempDir(), "-session-directory", t.TempDir(), "-steer"},
		steerEnv, func() []string { return []string{"PATH=/usr/bin:/bin"} },
		stdin, &stdout, &stderr, testConfig(client))
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "steering stopped") {
		t.Fatalf("stderr = %q", stderr.String())
	}
	if len(second.Input) == 0 {
		t.Fatal("the agent did not run twice")
	}

	stdin, feed = io.Pipe()
	defer feed.Close()
	client = &fakeClient{}
	answering = make(chan struct{})
	client.respond = func(ctx context.Context, request llm.Request) (llm.Response, error) {
		client.mu.Lock()
		client.calls++
		call := client.calls
		client.mu.Unlock()
		switch call {
		case 1:
			close(answering)
			time.Sleep(300 * time.Millisecond)
			if ctx.Err() != nil {
				t.Error("the message cut the answer short")
			}
			return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
				Type: llm.ItemToolCall,
				Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"sleep 0.3; echo tested"}`},
			}}}, nil
		default:
			second = request
			return llm.Response{ID: "response-2", Stop: llm.StopComplete, Output: []llm.Item{{
				Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"},
			}}}, nil
		}
	}
	go func() {
		<-answering
		io.WriteString(feed, `{"content":"use the race detector","message_id":"11111111-2222-4333-8444-555555555555"}`+"\n")
	}()
	stdout.Reset()
	stderr.Reset()
	// With -p the request is the prompt, and stdin carries only messages.
	code = RunMain(t.Context(),
		[]string{"-workspace", t.TempDir(), "-session-directory", t.TempDir(), "-steer", "-p", "run the tests"},
		steerEnv, func() []string { return []string{"PATH=/usr/bin:/bin"} },
		stdin, &stdout, &stderr, testConfig(client))
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q", code, stderr.String())
	}
	var tail []string
	for _, item := range second.Input {
		switch data := item.Data.(type) {
		case llm.ToolResult:
			tail = append(tail, "result: "+strings.TrimSpace(data.Output[0].Value))
		case llm.Message:
			tail = append(tail, string(data.Role)+": "+data.Text)
		}
	}
	if len(tail) < 2 || !strings.Contains(tail[len(tail)-2], "tested") || tail[len(tail)-1] != "user: use the race detector" {
		t.Fatalf("the second request ends with %q", tail)
	}
	// The session records the message where the model read it: after the
	// call's result, before the turn that answers it.
	var order []string
	decoder := jsontext.NewDecoder(&stdout)
	for {
		var item sessionstore.Item
		if err := json.UnmarshalDecode(decoder, &item); err != nil {
			if !errors.Is(err, io.EOF) {
				t.Fatal(err)
			}
			break
		}
		switch data := item.Data.(type) {
		case inbox.Input:
			if data.Kind == inbox.InputExternal {
				order = append(order, "input "+string(data.Delivery))
			}
		case sessionstore.ToolCallStatus:
			if len(data.Status.WaitingFor) != 0 && data.Operations[0].Status == "completed" {
				order = append(order, "result")
			}
		default:
			order = append(order, string(item.Kind))
		}
	}
	want := "input ,turn,model_response,result,input after_tools,turn,model_response"
	if got := strings.Join(order, ","); got != want {
		t.Fatalf("session items = %s, want %s", got, want)
	}
}

func steerEnv(name string) string {
	switch name {
	case llmAPIKeyEnvironment:
		return "secret"
	case "SHELL":
		return "/bin/sh"
	}
	return ""
}

func TestSteeringInputIsValidated(t *testing.T) {
	for _, line := range []string{
		`{"content":"  "}`,
		`{"content":"hi","role":"assistant"}`,
		`{"content":"hi","message_id":"nope"}`,
		`{"content":"hi","extra":true}`,
		`"hi"`,
	} {
		if _, err := steeringInput(jsontext.Value(line)); err == nil {
			t.Errorf("%s was accepted", line)
		}
	}
	input, err := steeringInput(jsontext.Value(`{"content":"hi","role":"user"}`))
	if err != nil || input.Delivery != inbox.DeliverAfterTools || input.Kind != inbox.InputExternal || input.ID == "" {
		t.Fatalf("input = %+v, %v", input, err)
	}
}
