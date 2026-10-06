package contextbuilder

import (
	"errors"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// compactedText compacts the conversation and returns the message that
// stands in for it.
func compactedText(t *testing.T, current Builder) string {
	t.Helper()
	current.Commit()
	if !current.Compact(answer("<summary>The work so far.</summary>")) {
		t.Fatal("did not compact")
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	return messageText(built.Request.Input[1])
}

func finish(current Builder, output string, ids ...string) {
	for _, id := range ids {
		current.AddToolResult(id, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: output}}, false)
	}
}

func TestCompactionListsTheFilesTheConversationTouched(t *testing.T) {
	current := NewBuilder()
	addInput(t, current, "task")
	current.Commit()
	current.AddModelResponse(answer("",
		llm.ToolCall{CallID: "read", Name: "mcp__kou__grace_Read", Arguments: `{"path":"a.go"}`},
		llm.ToolCall{CallID: "edit", Name: tool.EditName, Arguments: `{"path":"b.go","old_string":"x","new_string":"y"}`},
		llm.ToolCall{CallID: "failed", Name: tool.WriteName, Arguments: `{"path":"c.go","content":""}`},
		llm.ToolCall{CallID: "patch", Name: tool.ApplyPatchName, Arguments: `{"input":"*** Begin Patch\n*** Update File: d.go\n@@\n-x\n+y\n*** Add File: e.go\n+z\n*** End Patch"}`},
		llm.ToolCall{CallID: "bash", Name: "Bash", Arguments: `{"command":"cat f.go"}`},
	))
	finish(current, "ok", "read", "edit", "patch", "bash")
	finish(current, "Error: permission denied", "failed")
	current.Commit()
	current.AddModelResponse(answer("", llm.ToolCall{CallID: "reread", Name: tool.ReadName, Arguments: `{"path":"b.go"}`}))
	finish(current, "y", "reread")

	// The latest first; the failed call and the command touched nothing the
	// harness can tell.
	want := "<files>\n- b.go (read, changed)\n- e.go (changed)\n- d.go (changed)\n- a.go (read)\n</files>"
	if text := compactedText(t, current); !strings.Contains(text, want) || strings.Contains(text, "c.go") || strings.Contains(text, "f.go") {
		t.Fatalf("compacted message:\n%s", text)
	}

	// A later compaction lists the files the earlier one did after its own.
	addInput(t, current, "more")
	current.Commit()
	current.AddModelResponse(answer("", llm.ToolCall{CallID: "write", Name: tool.WriteName, Arguments: `{"path":"g.go","content":"package g"}`}))
	finish(current, "ok", "write")
	want = "<files>\n- g.go (changed)\n- b.go (read, changed)\n- e.go (changed)\n- d.go (changed)\n- a.go (read)\n</files>"
	if text := compactedText(t, current); !strings.Contains(text, want) {
		t.Fatalf("second compacted message:\n%s", text)
	}
}

func TestTouchedFilesKeepTheLatest(t *testing.T) {
	var items []llm.Item
	for index := range touchedLimit + 5 {
		path := strings.Repeat("x", index+1)
		items = append(items, llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: path, Name: tool.ReadName, Arguments: `{"path":"` + path + `"}`}})
	}
	files := touchedFiles(items, []touchedFile{{path: "earlier", read: true}})
	if len(files) != touchedLimit || files[0].path != strings.Repeat("x", touchedLimit+5) {
		t.Fatalf("files = %v", files)
	}
}

func TestCompactionCarriesTheNotes(t *testing.T) {
	notes, readErr := "", error(nil)
	current := NewBuilder()
	current.AddTool(llm.Tool{Type: llm.ToolFunction, Name: tool.TranscriptSearchName})
	current.SetTranscript("/work/.harness/sessions/abc.session.jsonl")
	current.SetMemory(Memory{
		Notes:     "/work/.harness/sessions/abc.notes.md",
		ReadNotes: func() (string, error) { return notes, readErr },
		Summaries: "/work/.harness/sessions/abc.summaries.md",
	})
	addInput(t, current, "task")
	current.Commit()
	current.AddModelResponse(answer("done"))

	text := compactedText(t, current)
	for _, want := range []string{
		"You keep notes across compactions in /work/.harness/sessions/abc.notes.md, which is empty so far.",
		"search the session's transcript with TranscriptSearch",
		"The summaries of this session's compactions, oldest first, are in /work/.harness/sessions/abc.summaries.md.",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("compacted message lacks %q:\n%s", want, text)
		}
	}
	// TranscriptSearch is there to search the transcript with.
	if strings.Contains(text, "abc.session.jsonl") || !strings.HasSuffix(text, compactedResume) {
		t.Fatalf("compacted message:\n%s", text)
	}

	notes = "  - hypothesis: the cache is stale\n"
	addInput(t, current, "more")
	current.Commit()
	current.AddModelResponse(answer("done"))
	if text := compactedText(t, current); !strings.Contains(text, "in /work/.harness/sessions/abc.notes.md across compactions; keep them current, since no compaction changes them:\n<notes>\n- hypothesis: the cache is stale\n</notes>") {
		t.Fatalf("compacted message:\n%s", text)
	}

	// Notes that cannot be read are no notes.
	readErr = errors.New("gone")
	addInput(t, current, "again")
	current.Commit()
	current.AddModelResponse(answer("done"))
	if text := compactedText(t, current); strings.Contains(text, "<notes>") || !strings.Contains(text, "which is empty so far") {
		t.Fatalf("compacted message:\n%s", text)
	}
}

func TestCompactionPointsToTheTranscriptSearchOfCode(t *testing.T) {
	current := NewBuilder()
	current.AddTool(llm.Tool{Type: llm.ToolFunction, Name: tool.CodeName})
	current.SetTranscript("/work/.harness/sessions/abc.session.jsonl")
	addInput(t, current, "task")
	current.Commit()
	current.AddModelResponse(answer("done"))
	if text := compactedText(t, current); !strings.Contains(text, "with transcriptSearch() in Code") || strings.Contains(text, "notes") {
		t.Fatalf("compacted message:\n%s", text)
	}
}
