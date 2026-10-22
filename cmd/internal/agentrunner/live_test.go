package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func outputLines(t *testing.T, output *bytes.Buffer) []map[string]any {
	t.Helper()
	var lines []map[string]any
	for line := range strings.Lines(output.String()) {
		var value map[string]any
		if err := json.Unmarshal([]byte(line), &value); err != nil {
			t.Fatalf("line %q: %v", line, err)
		}
		lines = append(lines, value)
	}
	return lines
}

func TestStreamedTextPrecedesTheResponse(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	var output bytes.Buffer
	observer := &sessionObserver{sessionID: "session", output: &output, cancel: cancel, live: newLiveOutput(&output)}
	reporting := llm.WithStreamReporter(ctx, observer.stream)

	// A compaction's summary streams as any response does, under its turn.
	observer.Observe("session", sessionstore.Item{Kind: sessionstore.ItemTurn, Data: session.Turn{ID: "turn-0", Type: session.TurnCompaction}})
	llm.ReportStream(reporting, llm.StreamEvent{Kind: llm.StreamText, Text: "summary"})

	observer.Observe("session", sessionstore.Item{Kind: sessionstore.ItemTurn, Data: session.Turn{ID: "turn-1", Type: session.TurnRegular}})
	llm.ReportStream(reporting, llm.StreamEvent{Reset: true}) // nothing streamed yet: nothing to void
	llm.ReportStream(reporting, llm.StreamEvent{Index: 0, Kind: llm.StreamReasoning, Text: "Think"})
	llm.ReportStream(reporting, llm.StreamEvent{Index: 0, Kind: llm.StreamReasoning, Text: "ing"})
	llm.ReportStream(reporting, llm.StreamEvent{Reset: true})
	llm.ReportStream(reporting, llm.StreamEvent{Index: 1, Kind: llm.StreamText, Text: "Hel"})
	llm.ReportStream(reporting, llm.StreamEvent{Index: 1, Kind: llm.StreamText, Text: "lo"})
	observer.Observe("session", sessionstore.Item{Kind: sessionstore.ItemModelResponse, Data: sessionstore.ModelResponse{TurnID: "turn-1"}})
	if err := observer.Err(); err != nil {
		t.Fatal(err)
	}

	lines := outputLines(t, &output)
	var kinds []string
	for _, line := range lines {
		switch {
		case line["type"] == "delta" && line["reset"] == true:
			kinds = append(kinds, "reset")
		case line["type"] == "delta":
			kinds = append(kinds, line["turn"].(string)+"/"+line["kind"].(string)+":"+line["text"].(string))
		default:
			kinds = append(kinds, line["Kind"].(string))
		}
	}
	want := []string{"turn", "turn-0/text:summary", "turn", "turn-1/reasoning:Thinking", "reset", "turn-1/text:Hello", "model_response"}
	if strings.Join(kinds, " ") != strings.Join(want, " ") {
		t.Fatalf("lines = %q, want %q", kinds, want)
	}

	// The run's end writes what waits, and nothing streams after it.
	llm.ReportStream(reporting, llm.StreamEvent{Index: 0, Kind: llm.StreamText, Text: "late"})
	observer.closeLive()
	llm.ReportStream(reporting, llm.StreamEvent{Index: 0, Kind: llm.StreamText, Text: "later"})
	observer.flushDeltas()
	last := outputLines(t, &output)
	if len(last) != len(lines)+1 || last[len(lines)]["text"] != "late" {
		t.Fatalf("after the end: %v", last[len(lines):])
	}
}

func TestCommandOutputIsRead(t *testing.T) {
	_, cancel := context.WithCancel(t.Context())
	defer cancel()
	base := t.TempDir()
	spec, err := operation.NewShellSpec(operation.ShellInput{Shell: "/bin/sh", Command: "make"}, base, 1000)
	if err != nil {
		t.Fatal(err)
	}
	running := operation.Operation{ID: "op-1", MaxOutputLength: spec.MaxOutputLength, Type: spec.Type, Version: spec.Version, Status: operation.StatusReady, State: spec.State}
	var output bytes.Buffer
	observer := &sessionObserver{sessionID: "session", output: &output, cancel: cancel, live: newLiveOutput(&output)}
	observer.Observe("session", sessionstore.Item{Kind: sessionstore.ItemToolCallStatus, Data: sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Status: tool.CallStatus{WaitingFor: []operation.ID{"op-1"}},
		Operations: []operation.Operation{running},
	}})
	read := func() []map[string]any {
		output.Reset()
		observer.mu.Lock()
		observer.readOutput()
		observer.mu.Unlock()
		return outputLines(t, &output)
	}
	if lines := read(); len(lines) != 0 {
		t.Fatalf("output before the command started: %v", lines)
	}

	directory := filepath.Join(base, "op-1")
	if err := os.MkdirAll(directory, 0o755); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(directory, operation.ShellOutFilename)
	write := func(data string) {
		file, err := os.OpenFile(out, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		if _, err := file.WriteString(data); err != nil {
			t.Fatal(err)
		}
	}
	// A character and an escape sequence cut in two wait for their ends.
	write("building ✓"[:len("building ✓")-1])
	if lines := read(); len(lines) != 1 || lines[0]["text"] != "building " || lines[0]["call"] != "call-1" || lines[0]["stream"] != "out" {
		t.Fatalf("first read = %v", lines)
	}
	write("\x93 done\x1b[3")
	if lines := read(); len(lines) != 1 || lines[0]["text"] != "✓ done" {
		t.Fatalf("second read = %v", lines)
	}
	write("2mred\x1b[0m\n")
	if lines := read(); len(lines) != 1 || lines[0]["text"] != "\x1b[32mred\x1b[0m\n" {
		t.Fatalf("third read = %v", lines)
	}

	// What the command writes faster than it is read is skipped.
	write(strings.Repeat("x", outputChunk+10))
	lines := read()
	if len(lines) != 1 || len(lines[0]["text"].(string)) != outputChunk || lines[0]["skipped"] != float64(10) {
		t.Fatalf("a long read: %d lines, skipped %v", len(lines), lines[0]["skipped"])
	}

	// The command's end stops the reads.
	running.Status = operation.StatusCompleted
	observer.Observe("session", sessionstore.Item{Kind: sessionstore.ItemToolCallStatus, Data: sessionstore.ToolCallStatus{
		TurnID: "turn-1", CallID: "call-1", Operations: []operation.Operation{running},
	}})
	write("after the end")
	if lines := read(); len(lines) != 0 {
		t.Fatalf("output after the command's end: %v", lines)
	}
}

func TestUnfinished(t *testing.T) {
	for _, test := range []struct {
		data string
		keep int
	}{
		{"plain", 0},
		{"cut \xe2\x9c", 2},
		{"whole ✓", 0},
		{"csi \x1b[31", 4},
		{"csi \x1b[31m", 0},
		{"lone \x1b", 1},
		{"osc \x1b]0;title", 9},
		{"osc \x1b]0;title\x07", 0},
		{"two-byte escape \x1bc", 0},
	} {
		if keep := unfinished([]byte(test.data)); keep != test.keep {
			t.Errorf("unfinished(%q) = %d, want %d", test.data, keep, test.keep)
		}
	}
}
