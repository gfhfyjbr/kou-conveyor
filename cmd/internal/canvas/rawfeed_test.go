package canvas

import (
	"encoding/json/v2"
	"fmt"
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

// sessionFile is a session transcript of records, and where each ends.
func sessionFile(t *testing.T, records ...any) (string, []int64) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "session.session.jsonl")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	ends := make([]int64, 0, len(records))
	for _, record := range records {
		ends = append(ends, writeSessionRecord(t, file, record))
	}
	return path, ends
}

func texts(entries []transcriptEntry) []string {
	out := make([]string, 0, len(entries))
	for _, entry := range entries {
		out = append(out, entry.Text)
	}
	return out
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
	page, err := cache.read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if page.Reset || page.More || page.Next != first || len(page.Entries) != 1 || page.Entries[0].Text != "first" {
		t.Fatalf("first read: %+v", page)
	}
	next := page.Next

	partial := []byte(`{"type":"error","message":"second"`)
	if _, err := file.Write(partial); err != nil {
		t.Fatal(err)
	}
	page, err = cache.read(path, next, options)
	if err != nil {
		t.Fatal(err)
	}
	if page.Reset || page.Next != next || len(page.Entries) != 0 {
		t.Fatalf("partial read: %+v, want cursor %d and no entry", page, next)
	}
	if _, err := file.Write([]byte("}\n")); err != nil {
		t.Fatal(err)
	}
	if err := file.Sync(); err != nil {
		t.Fatal(err)
	}
	page, err = cache.read(path, next, options)
	if err != nil {
		t.Fatal(err)
	}
	if page.Reset || page.Next <= next || len(page.Entries) != 1 || page.Entries[0].Text != "second" {
		t.Fatalf("completed tail: %+v", page)
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
	if _, err := cache.read(path, 0, options); err != nil {
		t.Fatal(err)
	}
	page, err := cache.read(path, end-1, options)
	if err != nil {
		t.Fatal(err)
	}
	if !page.Reset || page.Next != end || len(page.Entries) != 1 || page.Entries[0].Text != "old" {
		t.Fatalf("invalid cursor: %+v", page)
	}

	if err := file.Truncate(0); err != nil {
		t.Fatal(err)
	}
	if _, err := file.Seek(0, 0); err != nil {
		t.Fatal(err)
	}
	newEnd := writeSessionRecord(t, file, errorRecord("new"))
	page, err = cache.read(path, end, options)
	if err != nil {
		t.Fatal(err)
	}
	if !page.Reset || page.Next != newEnd || len(page.Entries) != 1 || page.Entries[0].Text != "new" {
		t.Fatalf("truncated file: %+v", page)
	}
}

func TestRawFeedIncludesReasoningAndClipsToolOutput(t *testing.T) {
	at := time.Date(2026, 10, 12, 10, 0, 0, 0, time.UTC)
	response := sessionstore.ModelResponse{TurnID: session.TurnID("turn-1"), Response: llm.Response{Output: []llm.Item{
		{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "assistant answer"}},
		{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"reasoning detail"}}},
		{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"printf hello"}`}},
	}}}
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
	path, ends := sessionFile(t,
		itemRecord(t, 1, at, sessionstore.ItemModelResponse, response),
		itemRecord(t, 2, at.Add(time.Second), sessionstore.ItemToolCallStatus, status))
	end := ends[len(ends)-1]

	options := transcriptTestOptions()
	options.MessageLimit = 64
	options.OutputLimit = 20
	page, err := newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if page.Reset || page.Next != end || len(page.Entries) != 3 {
		t.Fatalf("read: %+v", page)
	}
	var reasoning, toolEntry *transcriptEntry
	for index := range page.Entries {
		switch page.Entries[index].Kind {
		case cockpit.KindReasoning:
			reasoning = &page.Entries[index]
		case cockpit.KindTool:
			toolEntry = &page.Entries[index]
		}
	}
	if reasoning == nil || reasoning.Text != "reasoning detail" {
		t.Fatalf("reasoning = %+v", reasoning)
	}
	if toolEntry == nil || toolEntry.Tool == nil || toolEntry.Tool.Name != "Bash" {
		t.Fatalf("tool = %+v", toolEntry)
	}
	if len(toolEntry.Tool.Output) > len(output) || !strings.Contains(toolEntry.Tool.Output, fmt.Sprintf("[… %d bytes cut]", len(output)-20)) {
		t.Fatalf("clipped tool output = %+v", toolEntry.Tool)
	}

	options.Kinds = map[string]bool{cockpit.KindReasoning: true}
	page, err = newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 1 || page.Entries[0].Kind != cockpit.KindReasoning {
		t.Fatalf("reasoning filter = %+v", page.Entries)
	}
	options = transcriptTestOptions()
	options.Tools = map[string]bool{"bash": true}
	page, err = newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 1 || page.Entries[0].Kind != cockpit.KindTool {
		t.Fatalf("tool filter = %+v", page.Entries)
	}
}

// TestRawFeedPagesWithinTheBudget: entries past the budget of an answer are
// not given and passed over, but left after next, for the reader to read on.
func TestRawFeedPagesWithinTheBudget(t *testing.T) {
	var records []any
	for index := range 7 {
		records = append(records, errorRecord(fmt.Sprintf("record %d %s", index, strings.Repeat("x", 2000))))
	}
	path, ends := sessionFile(t, records...)
	cache := newRawFeedCache()
	options := transcriptTestOptions()
	options.Budget = 5000

	var seen []string
	after, pages := int64(0), 0
	for {
		page, err := cache.read(path, after, options)
		if err != nil {
			t.Fatal(err)
		}
		pages++
		size := 0
		for _, entry := range page.Entries {
			size += entrySize(entry)
		}
		if len(page.Entries) == 0 || size > options.Budget {
			t.Fatalf("page %d: %d entries of %d bytes, over the budget of %d", pages, len(page.Entries), size, options.Budget)
		}
		seen = append(seen, texts(page.Entries)...)
		if !page.More {
			if page.Next != ends[len(ends)-1] {
				t.Fatalf("last page: next %d, want the end %d", page.Next, ends[len(ends)-1])
			}
			break
		}
		if page.Next != ends[len(seen)-1] {
			t.Fatalf("page %d: next %d, want the end of its last record %d", pages, page.Next, ends[len(seen)-1])
		}
		after = page.Next
	}
	if pages != 4 || len(seen) != len(records) {
		t.Fatalf("%d pages of %d entries, want 4 pages of all %d", pages, len(seen), len(records))
	}
	for index, text := range seen {
		if !strings.HasPrefix(text, fmt.Sprintf("record %d ", index)) {
			t.Fatalf("entry %d is %.20q: out of order or lost", index, text)
		}
	}

	// The limit of entries pages them the same way.
	options = transcriptTestOptions()
	options.Limit = 3
	page, err := cache.read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 3 || !page.More || page.Next != ends[2] {
		t.Fatalf("limited page: %d entries, more %t, next %d", len(page.Entries), page.More, page.Next)
	}
}

// TestRawFeedFitsAnEntryToTheBudget: an entry alone over the budget is cut
// further than its default limits, but not past limits the reader chose.
func TestRawFeedFitsAnEntryToTheBudget(t *testing.T) {
	path, _ := sessionFile(t, errorRecord(strings.Repeat("y", 50000)))
	options := transcriptTestOptions()
	options.Budget = 3000
	page, err := newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 1 || entrySize(page.Entries[0]) > options.Budget || !strings.Contains(page.Entries[0].Text, "bytes cut]") {
		t.Fatalf("fitted entry: %d entries, the first of %d bytes", len(page.Entries), entrySize(page.Entries[0]))
	}
	options.ExplicitLimits = true
	page, err = newRawFeedCache().read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 1 || len(page.Entries[0].Text) < options.MessageLimit {
		t.Fatalf("explicit limits: the entry was cut to %d bytes, want its limit of %d", len(page.Entries[0].Text), options.MessageLimit)
	}
}

func TestRawFeedTail(t *testing.T) {
	var records []any
	for index := range 5 {
		records = append(records, errorRecord(fmt.Sprintf("record %d", index)))
	}
	path, ends := sessionFile(t, records...)
	cache := newRawFeedCache()
	options := transcriptTestOptions()
	options.Tail = 2
	page, err := cache.read(path, 0, options)
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(texts(page.Entries), ","); got != "record 3,record 4" || page.Older != 3 || page.Next != ends[4] || page.More {
		t.Fatalf("tail 2: %q, older %d, next %d, more %t", got, page.Older, page.Next, page.More)
	}
	options.Tail = 50
	if page, err = cache.read(path, 0, options); err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 5 || page.Older != 0 {
		t.Fatalf("tail 50: %d entries, older %d", len(page.Entries), page.Older)
	}

	// Of a tail over the budget the latest entries are given, one at least.
	path, _ = sessionFile(t, errorRecord("old "+strings.Repeat("o", 3000)), errorRecord("new "+strings.Repeat("n", 3000)))
	options.Tail, options.Budget = 2, 4000
	if page, err = newRawFeedCache().read(path, 0, options); err != nil {
		t.Fatal(err)
	}
	if len(page.Entries) != 1 || !strings.HasPrefix(page.Entries[0].Text, "new ") || page.Older != 1 {
		t.Fatalf("tail over the budget: %d entries, older %d", len(page.Entries), page.Older)
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
	if !options.ExplicitLimits || options.Budget != transcriptBudget || options.Tail != 0 {
		t.Fatalf("explicit limits %t, budget %d, tail %d", options.ExplicitLimits, options.Budget, options.Tail)
	}
	options, err = parseTranscriptOptions(map[string][]string{"tail": {"9999"}})
	if err != nil || options.Tail != maxTranscriptLimit || options.ExplicitLimits {
		t.Fatalf("tail: %+v %v", options, err)
	}
	if _, err := parseTranscriptOptions(map[string][]string{"tail": {"0"}}); err == nil {
		t.Fatal("tail 0 accepted")
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
	plain := transcriptCursorKey("n_r", "n_t", transcriptOptions{})
	if plain != "n_r\x00n_t" {
		t.Errorf("unfiltered cursor key %q: the cursors kept before filters had their own would be lost", plain)
	}
	one := transcriptCursorKey("n_r", "n_t", transcriptOptions{Kinds: map[string]bool{"tool": true, "assistant": true}})
	same := transcriptCursorKey("n_r", "n_t", transcriptOptions{Kinds: map[string]bool{"assistant": true, "tool": true}})
	other := transcriptCursorKey("n_r", "n_t", transcriptOptions{Tools: map[string]bool{"assistant": true, "tool": true}})
	if one != same || one == plain || one == other {
		t.Errorf("cursor keys: %q %q %q %q", plain, one, same, other)
	}
}
