package agentrunner

import (
	"bytes"
	"context"
	"errors"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

func TestCompactionLimits(t *testing.T) {
	knows := Provider{ContextWindow: func(model string) int64 {
		if model == "known" {
			return 400_000
		}
		return 0
	}}
	for _, test := range []struct {
		name, model, window, setting string
		provider                     Provider
		wantWindow, want             int64
		err                          bool
	}{
		{name: "known model", model: "known", provider: knows, wantWindow: 400_000, want: 367_000},
		{name: "unknown model", model: "other", provider: knows, wantWindow: 128_000, want: 96_000},
		{name: "provider without models", model: "known", wantWindow: 128_000, want: 96_000},
		{name: "window from the environment", model: "known", provider: knows, window: "1m", wantWindow: 1_000_000, want: 967_000},
		{name: "small window", model: "known", provider: knows, window: "32k", wantWindow: 32_000, want: 24_000},
		{name: "share of the window", model: "known", provider: knows, setting: "50%", wantWindow: 400_000, want: 200_000},
		{name: "fractional share", model: "known", provider: knows, setting: " 62.5 % ", wantWindow: 400_000, want: 250_000},
		{name: "tokens", model: "known", provider: knows, setting: "150000", wantWindow: 400_000, want: 150_000},
		{name: "thousands", model: "known", provider: knows, setting: "150K", wantWindow: 400_000, want: 150_000},
		{name: "off", model: "known", provider: knows, setting: "off", wantWindow: 400_000, want: 0},
		{name: "zero", model: "known", provider: knows, setting: "0", wantWindow: 400_000, want: 0},
		{name: "bad setting", setting: "soon", err: true},
		{name: "share over the window", setting: "120%", err: true},
		{name: "negative tokens", setting: "-5", err: true},
		{name: "bad window", window: "large", err: true},
		{name: "zero window", window: "0", err: true},
	} {
		t.Run(test.name, func(t *testing.T) {
			window, got, err := compactionLimits(func(name string) string {
				switch name {
				case contextWindowEnvironment:
					return test.window
				case autoCompactEnvironment:
					return test.setting
				}
				return ""
			}, test.provider, test.model)
			if (err != nil) != test.err || got != test.want || window != test.wantWindow {
				t.Fatalf("compactionLimits = %d, %d, %v; want %d, %d, error %v", window, got, err, test.wantWindow, test.want, test.err)
			}
		})
	}
}

func TestValidateCompactRequests(t *testing.T) {
	id := "session-1"
	for _, test := range []struct {
		name    string
		request Request
		want    string
	}{
		{name: "compaction alone", request: Request{SessionID: &id, Compact: true, CompactInstructions: "tests"}},
		{name: "compaction then a prompt", request: Request{SessionID: &id, Compact: true, Prompt: new("go on")}},
		{name: "no session", request: Request{Compact: true}, want: "compact needs the session_id"},
		{name: "instructions alone", request: Request{SessionID: &id, Prompt: new("hi"), CompactInstructions: "tests"}, want: "compact_instructions needs compact"},
	} {
		t.Run(test.name, func(t *testing.T) {
			_, err := validateRequest(test.request)
			if test.want == "" && err != nil || test.want != "" && (err == nil || !strings.Contains(err.Error(), test.want)) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
}

// compactionRun drives the runner through a fake model and records what it
// was asked.
type compactionRun struct {
	t         *testing.T
	workspace string
	sessions  string
	env       map[string]string

	mu       sync.Mutex
	requests []llm.Request
	answer   func(llm.Request) llm.Response
	// reject, when set, fails the requests it returns an error for.
	reject func(llm.Request) error
}

func newCompactionRun(t *testing.T) *compactionRun {
	return &compactionRun{t: t, workspace: t.TempDir(), sessions: t.TempDir(), env: map[string]string{"OPENAI_API_KEY": "secret"}}
}

func (run *compactionRun) do(request string) (string, int) {
	run.t.Helper()
	client := &fakeClient{respond: func(_ context.Context, request llm.Request) (llm.Response, error) {
		run.mu.Lock()
		defer run.mu.Unlock()
		run.requests = append(run.requests, request)
		if run.reject != nil {
			if err := run.reject(request); err != nil {
				return llm.Response{}, err
			}
		}
		return run.answer(request), nil
	}}
	var stdout, stderr bytes.Buffer
	code := RunMain(run.t.Context(), []string{"-workspace", run.workspace, "-session-directory", run.sessions},
		func(name string) string { return run.env[name] }, func() []string { return nil },
		strings.NewReader(request), &stdout, &stderr, testConfig(client))
	if code != 0 {
		run.t.Logf("stderr: %s", stderr.String())
	}
	return stdout.String(), code
}

func compactionPromptOf(request llm.Request) string {
	if len(request.Input) == 0 {
		return ""
	}
	message, _ := request.Input[len(request.Input)-1].Data.(llm.Message)
	if !strings.HasPrefix(message.Text, "CRITICAL: respond with text only, and do not call any tools.") {
		return ""
	}
	return message.Text
}

func summaryOf(request llm.Request) string {
	for _, item := range request.Input {
		if message, ok := item.Data.(llm.Message); ok && strings.Contains(message.Text, "<summary>") {
			return message.Text
		}
	}
	return ""
}

func TestRunnerCompactsSessionOnRequest(t *testing.T) {
	run := newCompactionRun(t)
	run.answer = func(request llm.Request) llm.Response {
		text := "First answer."
		if compactionPromptOf(request) != "" {
			text = "Summary of the first task."
		}
		return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text}}}}
	}
	if _, code := run.do(`{"session_id":"compact-me","prompt":"first task"}`); code != 0 {
		t.Fatalf("first run exited %d", code)
	}
	output, code := run.do(`{"session_id":"compact-me","compact":true,"compact_instructions":"the API"}`)
	if code != 0 {
		t.Fatalf("compaction exited %d: %s", code, output)
	}
	assertItemSequence(t, output,
		"input.control input.control input.control turn model_response",
		"input.control input.control turn input.control model_response",
		"input.control input.control turn model_response input.control",
	)
	items := decodeLogItems(t, []byte(output))
	var turn session.Turn
	for _, item := range items {
		if value, ok := item.Data.(session.Turn); ok {
			turn = value
		}
	}
	if turn.Type != session.TurnCompaction || len(run.requests) != 2 ||
		!strings.Contains(compactionPromptOf(run.requests[1]), "The user asked the summary to focus on:\nthe API\n\nReminder:") {
		t.Fatalf("turn %#v after %d requests", turn, len(run.requests))
	}

	// The next run starts from the summary.
	if output, code := run.do(`{"session_id":"compact-me","prompt":"second task"}`); code != 0 {
		t.Fatalf("second run exited %d: %s", code, output)
	}
	last := run.requests[len(run.requests)-1]
	if summary := summaryOf(last); !strings.Contains(summary, "<summary>\nSummary of the first task.\n</summary>") || len(last.Input) != 3 ||
		!strings.Contains(summary, "full transcript at "+filepath.Join(run.sessions, "compact-me.session.jsonl")+" ") {
		t.Fatalf("request after the compaction = %#v", last.Input)
	}
}

func TestRunnerRefusesToCompactMissingSession(t *testing.T) {
	run := newCompactionRun(t)
	output, code := run.do(`{"session_id":"missing","compact":true}`)
	if code == 0 || !strings.Contains(output, "nothing to compact") {
		t.Fatalf("exit %d, output %s", code, output)
	}
	if entries, _ := filepath.Glob(filepath.Join(run.sessions, "*.session.jsonl")); len(entries) != 0 {
		t.Fatalf("created %v", entries)
	}
}

func TestRunnerCompactsAutomaticallyAndContinues(t *testing.T) {
	run := newCompactionRun(t)
	run.env[autoCompactEnvironment] = "10k"
	run.answer = func(request llm.Request) llm.Response {
		if compactionPromptOf(request) != "" {
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Summary so far."}}}}
		}
		return llm.Response{
			Stop:   llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Answer."}}},
			Usage:  llm.Usage{InputTokens: 11_000, OutputTokens: 500},
		}
	}
	if _, code := run.do(`{"session_id":"long","prompt":"first task"}`); code != 0 {
		t.Fatalf("first run exited %d", code)
	}
	output, code := run.do(`{"session_id":"long","prompt":"second task"}`)
	if code != 0 {
		t.Fatalf("second run exited %d: %s", code, output)
	}
	var types []session.TurnType
	for _, item := range decodeLogItems(t, []byte(output)) {
		if turn, ok := item.Data.(session.Turn); ok {
			types = append(types, turn.Type)
		}
		if response, ok := item.Data.(sessionstore.ModelResponse); ok && response.Response.Stop != llm.StopComplete {
			t.Fatalf("response %#v", response)
		}
	}
	if len(types) != 2 || types[0] != session.TurnCompaction || types[1] != session.TurnRegular || len(run.requests) != 3 {
		t.Fatalf("turns %v after %d requests", types, len(run.requests))
	}
	last := run.requests[2]
	if !strings.Contains(summaryOf(last), "Summary so far.") {
		t.Fatalf("continuation lacks the summary: %#v", last.Input)
	}
	if message, _ := last.Input[len(last.Input)-1].Data.(llm.Message); message.Text != "second task" {
		t.Fatalf("continuation does not end with the prompt: %#v", last.Input)
	}
}

func TestRunnerCompactsARequestTheWindowCannotHold(t *testing.T) {
	run := newCompactionRun(t)
	run.env[contextWindowEnvironment] = "200k"
	run.answer = func(request llm.Request) llm.Response {
		text := "Answer."
		if compactionPromptOf(request) != "" {
			text = "<analysis>notes</analysis>\n<summary>Summary so far.</summary>"
		}
		return llm.Response{
			Stop:   llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text}}},
			Usage:  llm.Usage{InputTokens: 150_000, OutputTokens: 500},
		}
	}
	// The provider counts more than the estimate: the second prompt's
	// request does not fit, until the conversation is compacted.
	run.reject = func(request llm.Request) error {
		if compactionPromptOf(request) == "" && summaryOf(request) == "" && len(request.Input) > 3 {
			return &llm.ContextOverflowError{Tokens: 230_000, Limit: 200_000, Err: errors.New("prompt is too long: 230000 tokens > 200000 maximum")}
		}
		return nil
	}
	if _, code := run.do(`{"session_id":"long","prompt":"first task"}`); code != 0 {
		t.Fatalf("first run exited %d", code)
	}
	output, code := run.do(`{"session_id":"long","prompt":"second task"}`)
	if code != 0 {
		t.Fatalf("second run exited %d: %s", code, output)
	}
	var types []session.TurnType
	for _, item := range decodeLogItems(t, []byte(output)) {
		if turn, ok := item.Data.(session.Turn); ok {
			types = append(types, turn.Type)
		}
	}
	want := []session.TurnType{session.TurnRegular, session.TurnCompaction, session.TurnRegular}
	if !slices.Equal(types, want) || len(run.requests) != 4 {
		t.Fatalf("turns %v after %d requests", types, len(run.requests))
	}
	last := run.requests[3]
	if !strings.Contains(summaryOf(last), "<summary>\nSummary so far.\n</summary>") || strings.Contains(summaryOf(last), "notes") {
		t.Fatalf("continuation lacks the summary: %#v", last.Input)
	}
	if message, _ := last.Input[len(last.Input)-1].Data.(llm.Message); message.Text != "second task" {
		t.Fatalf("continuation does not end with the prompt: %#v", last.Input)
	}
}
