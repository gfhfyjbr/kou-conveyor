package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Editing a prompt must not show the rewound run what its discarded future
// wrote of the model's memory: the notes, which the harness shows the model
// as a run starts when they changed since the conversation last showed them,
// nor the summaries of the compactions, which it points the model at. The
// runner runs on its own here, with no cockpit to snapshot the memory each
// run starts from: the rewind tells it from the log.
func TestRewoundRunIsNotShownTheMemoryOfItsDiscardedFuture(t *testing.T) {
	workspace, sessions := t.TempDir(), t.TempDir()
	ctx, cancel := context.WithTimeout(t.Context(), 30*time.Second)
	defer cancel()
	notes := filepath.Join(sessions, "edited.notes.md")
	summaries := filepath.Join(sessions, "edited.summaries.md")
	const (
		firstNotes       = "notes of the first run"
		discardedNotes   = "notes of the discarded future"
		firstSummary     = "Summary of the first run."
		discardedSummary = "Summary of the discarded future."
		edited           = "second question, edited"
	)
	// What the model writes in its notes as it answers a prompt.
	writes := map[string]string{"first question": firstNotes, "second question": discardedNotes}
	prompts := []string{"first question", "second question", edited}

	var mu sync.Mutex
	var requests []llm.Request
	compactions := 0
	client := &fakeClient{}
	client.respond = func(_ context.Context, request llm.Request) (llm.Response, error) {
		mu.Lock()
		defer mu.Unlock()
		requests = append(requests, request)
		answer := func(text string) (llm.Response, error) {
			return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{
				Role: llm.RoleAssistant, Text: text,
			}}}}, nil
		}
		if compactionPromptOf(request) != "" {
			compactions++
			if compactions == 1 {
				return answer(firstSummary)
			}
			return answer(discardedSummary)
		}
		// The prompt of the run, and whether the model wrote its notes for
		// it already.
		prompt, wrote := "", false
		for _, item := range request.Input {
			switch data := item.Data.(type) {
			case llm.Message:
				if data.Role == llm.RoleUser && slices.Contains(prompts, data.Text) {
					prompt, wrote = data.Text, false
				}
			case llm.ToolResult:
				wrote = true
			}
		}
		text, ok := writes[prompt]
		if !ok || wrote {
			return answer("done")
		}
		arguments, err := json.Marshal(map[string]string{"command": fmt.Sprintf("printf '%%s' '%s' > '%s'", text, notes)})
		if err != nil {
			return llm.Response{}, err
		}
		return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
			CallID: fmt.Sprintf("call-%d", len(requests)), Name: "Bash", Arguments: string(arguments),
		}}}}, nil
	}
	run := func(request string) {
		t.Helper()
		var stdout, stderr bytes.Buffer
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
			t.Fatalf("%s: exit %d: %s", request, code, stderr.String())
		}
	}
	prompt := func(messageID, text string) {
		t.Helper()
		run(fmt.Sprintf(`{"session_id":"edited","messages":[{"role":"user","content":%q,"message_id":%q}]}`, text, messageID))
	}
	read := func(path string) string {
		t.Helper()
		data, err := os.ReadFile(path)
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return string(data)
	}

	// Each run writes the notes, and a compaction after it shows them to
	// the model and keeps its summary.
	prompt("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b101", "first question")
	run(`{"session_id":"edited","compact":true}`)
	prompt("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b102", "second question")
	run(`{"session_id":"edited","compact":true}`)
	if got := read(notes); got != discardedNotes {
		t.Fatalf("notes before the rewind = %q", got)
	}
	if got := read(summaries); !strings.Contains(got, firstSummary) || !strings.Contains(got, discardedSummary) {
		t.Fatalf("summaries before the rewind = %q", got)
	}

	if err := cockpit.RewindSession(sessions, "edited", "0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b102"); err != nil {
		t.Fatal(err)
	}
	// The notes are as the compaction that remains showed them, and the
	// summaries are those of the compactions that remain.
	if got := read(notes); got != firstNotes+"\n" {
		t.Errorf("notes after the rewind = %q", got)
	}
	if got := read(summaries); !strings.Contains(got, firstSummary) || strings.Contains(got, discardedSummary) {
		t.Errorf("summaries after the rewind = %q", got)
	}

	mu.Lock()
	requests = nil
	mu.Unlock()
	prompt("0b0a5ac2-5ad1-4e60-9d69-0c3d8ea0b103", edited)
	mu.Lock()
	defer mu.Unlock()
	if len(requests) == 0 {
		t.Fatal("the edited prompt asked the model nothing")
	}
	shown := requestText(requests[0])
	if strings.Contains(shown, discardedNotes) || strings.Contains(shown, discardedSummary) || strings.Contains(shown, contextbuilder.NotesUpdatePrefix) {
		t.Fatalf("the edited prompt's run is shown its discarded future:\n%s", shown)
	}
	if !strings.Contains(shown, firstSummary) || !strings.Contains(shown, edited) {
		t.Fatalf("context of the edited prompt:\n%s", shown)
	}
}

// requestText is the text a model request shows the model.
func requestText(request llm.Request) string {
	var parts []string
	for _, item := range request.Input {
		switch data := item.Data.(type) {
		case llm.Message:
			parts = append(parts, fmt.Sprintf("%s: %s", data.Role, data.Text))
		case llm.ToolCall:
			parts = append(parts, "call: "+data.Arguments)
		case llm.ToolResult:
			for _, output := range data.Output {
				parts = append(parts, "result: "+output.Value)
			}
		}
	}
	return strings.Join(parts, "\n")
}
