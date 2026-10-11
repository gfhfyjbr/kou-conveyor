package canvas

import (
	"encoding/json/v2"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func transcriptTestOptions() transcriptOptions {
	return transcriptOptions{Limit: 50, MessageLimit: defaultMessageLimit, OutputLimit: defaultOutputLimit}
}

func writeSessionRecord(t *testing.T, file *os.File, value any) int64 {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	before, err := file.Seek(0, 2)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := file.Write(data); err != nil {
		t.Fatal(err)
	}
	return before + int64(len(data))
}

func itemRecord(t *testing.T, sequence uint64, at time.Time, kind sessionstore.ItemKind, data any) map[string]any {
	t.Helper()
	return map[string]any{
		"type": "item",
		"data": map[string]any{
			"Item": sessionstore.Item{
				Sequence:   sessionstore.Sequence(sequence),
				RecordedAt: at,
				Kind:       kind,
				Data:       data,
			},
		},
	}
}

func errorRecord(message string) map[string]any {
	return map[string]any{"type": "error", "message": message}
}

func TestRawFeedPreservesPartialTailAndContinues(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.session.jsonl")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()

	first := writeSessionRecord(t, file, errorRecord("first"))
	cache := newRawFeedCache()
	options := transcriptTestOptions()
	entries, next, reset, err := cache.read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 0 || next != first || len(entries) != 1 || entries[0].Text != "first" {
		t.Fatalf("first read: entries=%+v next=%d reset=%d", entries, next, reset)
	}

	partial := []byte(`{"type":"error","message":"second"`)
	if _, err := file.Write(partial); err != nil {
		t.Fatal(err)
	}
	entries, nextAgain, reset, err := cache.read(path, next, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 0 || nextAgain != next || len(entries) != 0 {
		t.Fatalf("partial read: entries=%+v next=%d reset=%d, want cursor %d and no entry", entries, nextAgain, reset, next)
	}
	if _, err := file.Write([]byte("}\n")); err != nil {
		t.Fatal(err)
	}
	if err := file.Sync(); err != nil {
		t.Fatal(err)
	}
	entries, nextAgain, reset, err = cache.read(path, next, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 0 || nextAgain <= next || len(entries) != 1 || entries[0].Text != "second" {
		t.Fatalf("completed tail: entries=%+v next=%d reset=%d", entries, nextAgain, reset)
	}
}

func TestRawFeedResetsInvalidAndTruncatedCursors(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.session.jsonl")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	end := writeSessionRecord(t, file, errorRecord("old"))

	cache := newRawFeedCache()
	options := transcriptTestOptions()
	if _, _, _, err := cache.read(path, 0, options); err != nil {
		t.Fatal(err)
	}
	entries, next, reset, err := cache.read(path, end-1, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 1 || next != end || len(entries) != 1 || entries[0].Text != "old" {
		t.Fatalf("invalid cursor: entries=%+v next=%d reset=%d", entries, next, reset)
	}

	if err := file.Truncate(0); err != nil {
		t.Fatal(err)
	}
	if _, err := file.Seek(0, 0); err != nil {
		t.Fatal(err)
	}
	newEnd := writeSessionRecord(t, file, errorRecord("new"))
	entries, next, reset, err = cache.read(path, end, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 1 || next != newEnd || len(entries) != 1 || entries[0].Text != "new" {
		t.Fatalf("truncated file: entries=%+v next=%d reset=%d", entries, next, reset)
	}
}

func TestRawFeedIncludesReasoningAndClipsToolOutput(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.session.jsonl")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	at := time.Date(2026, 10, 12, 10, 0, 0, 0, time.UTC)
	response := sessionstore.ModelResponse{TurnID: session.TurnID("turn-1"), Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "assistant answer"}},
		{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"reasoning detail"}}},
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"printf hello"}`}},
	}}}
	writeSessionRecord(t, file, itemRecord(t, 1, at, sessionstore.ItemModelResponse, response))
	output := strings.Repeat("output-", 20)
	state, err := json.Marshal(operation.ShellState{
		Input:  operation.ShellInput{Command: "printf hello"},
		Result: &operation.ShellResult{Out: output, OutSize: int64(len(output))},
	})
	if err != nil {
		t.Fatal(err)
	}
	status := sessionstore.ToolCallStatus{
		TurnID: session.TurnID("turn-1"), CallID: "call-1", Status: tool.CallStatus{},
		Operations: []operation.Operation{{ID: "operation-1", Type: operation.TypeShell, Version: operation.VersionShell, Status: operation.StatusCompleted, State: state}},
	}
	end := writeSessionRecord(t, file, itemRecord(t, 2, at.Add(time.Second), sessionstore.ItemToolCallStatus, status))

	options := transcriptTestOptions()
	options.MessageLimit = 64
	options.OutputLimit = 20
	entries, next, reset, err := newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if reset != 0 || next != end || len(entries) != 3 {
		t.Fatalf("read: entries=%+v next=%d reset=%d", entries, next, reset)
	}
	var reasoning, toolEntry *rawFeedEntry
	for index := range entries {
		switch entries[index].Kind {
		case cockpit.KindReasoning:
			reasoning = &entries[index]
		case cockpit.KindTool:
			toolEntry = &entries[index]
		}
	}
	if reasoning == nil || reasoning.Text != "reasoning detail" {
		t.Fatalf("reasoning = %+v", reasoning)
	}
	if toolEntry == nil || toolEntry.Tool == nil || toolEntry.Tool.Name != "Bash" {
		t.Fatalf("tool = %+v", toolEntry)
	}
	public := toolEntry.public(options)
	if public.Tool == nil || len(public.Tool.Output) > len(output) || !strings.Contains(public.Tool.Output, "bytes cut") {
		t.Fatalf("clipped tool output = %+v", public.Tool)
	}

	options.Kinds = map[string]bool{cockpit.KindReasoning: true}
	entries, _, _, err = newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || entries[0].Kind != cockpit.KindReasoning {
		t.Fatalf("reasoning filter = %+v", entries)
	}
	options = transcriptTestOptions()
	options.Tools = map[string]bool{"bash": true}
	entries, _, _, err = newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || entries[0].Kind != cockpit.KindTool {
		t.Fatalf("tool filter = %+v", entries)
	}
}

func TestTranscriptOptionParsingAndKindClassification(t *testing.T) {
	since := "2026-10-12T10:00:00Z"
	options, err := parseTranscriptOptions(map[string][]string{
		"limit": {"3"}, "kinds": {"reasoning, tool"}, "tools": {"Bash"},
		"since": {since}, "message_limit": {"99"}, "output_limit": {"100"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if options.Limit != 3 || !options.Kinds["reasoning"] || !options.Kinds["tool"] || !options.Tools["bash"] || options.MessageLimit != 99 || options.OutputLimit != 100 || options.Since == nil {
		t.Fatalf("options = %+v", options)
	}
	if offset, last, err := parseTranscriptAfter("last"); err != nil || !last || offset != 0 {
		t.Fatalf("last cursor: offset=%d last=%t err=%v", offset, last, err)
	}
	if offset, last, err := parseTranscriptAfter("42"); err != nil || last || offset != 42 {
		t.Fatalf("numeric cursor: offset=%d last=%t err=%v", offset, last, err)
	}
	if _, _, err := parseTranscriptAfter("not-a-cursor"); err == nil {
		t.Fatal("invalid cursor accepted")
	}
	for text, want := range map[string]string{
		"[harness] Progress report: still working": "heartbeat",
		"[canvas] reply from «worker» (n_1): done": "peer",
		"Context compacted from 10 tokens":         "compaction",
	} {
		entry := &cockpit.Entry{Kind: cockpit.KindUser, Text: text}
		if strings.HasPrefix(text, "Context") {
			entry.Kind = cockpit.KindNotice
		}
		if got := rawKind(entry); got != want {
			t.Errorf("rawKind(%q) = %q, want %q", text, got, want)
		}
	}
}
