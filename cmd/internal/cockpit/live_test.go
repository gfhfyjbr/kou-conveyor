package cockpit

import (
	"encoding/json/v2"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func streamLine(t *testing.T, fields map[string]any) []byte {
	t.Helper()
	line, err := json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	// The runner writes the type first, as its struct has it.
	return append([]byte(`{"type":"delta",`), line[1:]...)
}

func live(t *testing.T, tr *Transcript, line []byte) Streamed {
	t.Helper()
	streamed, ok := tr.Live(line)
	if !ok {
		t.Fatalf("Live did not take %s", line)
	}
	return streamed
}

func TestTranscriptPreviewsStreamedText(t *testing.T) {
	tr := NewTranscript()
	apply(t, tr, liveLine(t, 1, sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular}))

	first := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 0, "kind": "reasoning", "text": "Look"}))
	if first.Delta == nil || first.Delta.Kind != KindReasoning || first.Delta.Text != "Look" || first.Delta.ID == "" {
		t.Fatalf("first delta = %#v", first.Delta)
	}
	more := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 0, "kind": "reasoning", "text": "ing\x1b[1m around"}))
	if more.Delta == nil || more.Delta.ID != first.Delta.ID || more.Delta.Text != "ing around" {
		t.Fatalf("delta of the same item = %#v", more.Delta)
	}
	answer := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 1, "kind": "text", "text": "Done"}))
	if answer.Delta == nil || answer.Delta.Kind != KindAssistant || answer.Delta.ID == first.Delta.ID {
		t.Fatalf("answer delta = %#v", answer.Delta)
	}
	extra := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 2, "kind": "text", "text": "dropped later"}))
	if len(tr.Entries) != 0 {
		t.Fatalf("previews made entries: %#v", tr.Entries)
	}

	changed, err := tr.Apply(liveLine(t, 2, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
		Output: []llm.Item{
			{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"Looking around"}}},
			{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Done."}},
		},
	}}))
	if err != nil {
		t.Fatal(err)
	}
	replaced := map[string]string{}
	var gone []string
	for _, e := range changed {
		switch {
		case e.Gone:
			gone = append(gone, e.ID)
		case e.Replaces != "":
			replaced[e.Replaces] = e.ID
		}
	}
	if replaced[first.Delta.ID] != "output:2.0" || replaced[answer.Delta.ID] != "output:2.1" {
		t.Fatalf("replaced = %v", replaced)
	}
	if len(gone) != 1 || gone[0] != extra.Delta.ID {
		t.Fatalf("gone = %v, want %s", gone, extra.Delta.ID)
	}
	if e := tr.Entry(extra.Delta.ID); e != nil {
		t.Fatalf("a void preview is an entry: %#v", e)
	}
}

func TestTranscriptVoidsPreviewsThatStartOver(t *testing.T) {
	tr := NewTranscript()
	apply(t, tr, liveLine(t, 1, sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular}))
	first := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 0, "kind": "text", "text": "Half"}))
	reset := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "reset": true}))
	if len(reset.Changed) != 1 || reset.Changed[0].ID != first.Delta.ID || !reset.Changed[0].Gone {
		t.Fatalf("reset changed = %#v", reset.Changed)
	}
	again := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 0, "kind": "text", "text": "Whole"}))
	if again.Delta == nil || again.Delta.ID == first.Delta.ID {
		t.Fatalf("a response that started over reused its void preview: %#v", again.Delta)
	}

	// A turn that never got its response leaves its previews when the next
	// starts, and the run leaves those of the last.
	changed, err := tr.Apply(liveLine(t, 2, sessionstore.ItemTurn, session.Turn{ID: "turn-2", Type: session.TurnRegular}))
	if err != nil {
		t.Fatal(err)
	}
	if len(changed) != 1 || changed[0].ID != again.Delta.ID || !changed[0].Gone {
		t.Fatalf("next turn changed = %#v", changed)
	}
	last := live(t, tr, streamLine(t, map[string]any{"turn": "turn-2", "index": 0, "kind": "text", "text": "Cut"}))
	var gone bool
	for _, e := range tr.Finish(errors.New("killed"), false, time.Now()) {
		gone = gone || e.ID == last.Delta.ID && e.Gone
	}
	if !gone {
		t.Fatal("the run's end left a preview")
	}
}

func TestTranscriptStreamsTheSummaryOfACompaction(t *testing.T) {
	tr := NewTranscript()
	apply(t, tr, liveLine(t, 1, sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnCompaction}))
	summary := live(t, tr, streamLine(t, map[string]any{"turn": "turn-1", "index": 0, "kind": "text", "text": "<summary>The user"}))
	if summary.Delta == nil || !summary.Delta.Compaction || summary.Delta.Kind != KindAssistant || summary.Delta.Text != "<summary>The user" {
		t.Fatalf("delta of a compaction = %#v", summary.Delta)
	}
	if line, err := json.Marshal(summary.Delta); err != nil || !strings.Contains(string(line), `"compaction":true`) {
		t.Fatalf("delta of a compaction encodes as %s (%v)", line, err)
	}

	// The notice holds the summary, and what streamed of it goes: no entry
	// takes its place.
	changed, err := tr.Apply(liveLine(t, 2, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
		Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "<summary>The user asked.</summary>"}}},
	}}))
	if err != nil {
		t.Fatal(err)
	}
	var notice, gone bool
	for _, e := range changed {
		notice = notice || e.Kind == KindNotice && e.Detail == "The user asked."
		gone = gone || e.Gone && e.ID == summary.Delta.ID
		if e.Replaces != "" {
			t.Fatalf("an entry replaces what streamed of a compaction: %#v", e)
		}
	}
	if !notice || !gone {
		t.Fatalf("changed = %#v", changed)
	}

	// The turns after it stream as any.
	apply(t, tr, liveLine(t, 3, sessionstore.ItemTurn, session.Turn{ID: "turn-2", Type: session.TurnRegular}))
	answer := live(t, tr, streamLine(t, map[string]any{"turn": "turn-2", "index": 0, "kind": "text", "text": "Hi"}))
	if answer.Delta == nil || answer.Delta.Compaction {
		t.Fatalf("delta after the compaction = %#v", answer.Delta)
	}
	if line, _ := json.Marshal(answer.Delta); strings.Contains(string(line), "compaction") {
		t.Fatalf("delta of an answer encodes as %s", line)
	}
}

func TestTranscriptStreamsCommandOutput(t *testing.T) {
	tr := NewTranscript()
	waiting := tool.CallStatus{WaitingFor: []operation.ID{"op-1"}}
	apply(t, tr,
		liveLine(t, 1, sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular}),
		liveLine(t, 2, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"go test ./..."}`}}},
		}}),
		liveLine(t, 3, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "turn-1", CallID: "call-1", Status: waiting,
			Operations: []operation.Operation{shellOperation(t, operation.StatusReady, nil)},
		}),
	)
	output := live(t, tr, []byte(`{"type":"output","call":"call-1","operation":"op-1","stream":"err","text":"\u001b[31mFAIL\u001b[0m\r\nok\r50%","skipped":12}`))
	if output.Output == nil || *output.Output != (Output{ID: "tool:call-1", Stream: "err", Text: "FAIL\r\nok\r50%", Skipped: 12}) {
		t.Fatalf("output = %#v", output.Output)
	}
	if unknown := live(t, tr, []byte(`{"type":"output","call":"call-9","operation":"op-9","stream":"out","text":"x"}`)); unknown.Output != nil {
		t.Fatalf("output of an unknown call = %#v", unknown.Output)
	}
	apply(t, tr, liveLine(t, 4, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Status: waiting,
		Operations: []operation.Operation{shellOperation(t, operation.StatusCompleted, &operation.ShellResult{Out: "ok\n"})},
	}))
	if late := live(t, tr, []byte(`{"type":"output","call":"call-1","operation":"op-1","stream":"out","text":"late"}`)); late.Output != nil {
		t.Fatalf("output after the outcome = %#v", late.Output)
	}
}

func TestTranscriptApplyIgnoresStreamedLines(t *testing.T) {
	tr := NewTranscript()
	for _, line := range []string{
		`{"type":"delta","turn":"turn-1","index":0,"kind":"text","text":"x"}`,
		`{"type":"output","call":"call-1","operation":"op-1","stream":"out","text":"x"}`,
	} {
		if changed, err := tr.Apply([]byte(line)); err != nil || changed != nil {
			t.Fatalf("Apply(%s) = %v, %v", line, changed, err)
		}
	}
	if _, ok := tr.Live(liveLine(t, 1, sessionstore.ItemTurn, session.Turn{ID: "turn-1"})); ok {
		t.Fatal("Live took a session item")
	}
}
