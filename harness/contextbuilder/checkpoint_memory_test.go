package contextbuilder

import (
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestContextCheckpointPreservesFailedTouchesAfterPruningAndMasking(t *testing.T) {
	for _, mode := range []string{"prune", "mask"} {
		t.Run(mode, func(t *testing.T) {
			full, live := NewBuilder().(*builder), NewBuilder().(*builder)
			for _, current := range []*builder{full, live} {
				current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "failed-read", Name: tool.ReadName, Arguments: `{"path":"missing.go"}`}}}})
				current.AddToolResult("failed-read", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "Error: file missing " + strings.Repeat("failure details ", 1000)}}, false)
				current.AddModelResponse(llm.Response{Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "later-result", Name: "Echo", Arguments: `{}`}}}})
				current.AddToolResult("later-result", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "the newest result"}}, false)
				current.Commit()
			}
			if mode == "prune" {
				live.SetPruning(PruneOptions{Budget: 1})
			} else {
				live.SetMasking(MaskOptions{Results: 1})
			}
			if _, err := live.Build(); err != nil {
				t.Fatal(err)
			}
			if live.pruned+live.masked == 0 {
				t.Fatal("fixture did not remove the failed result")
			}
			response := llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Read failed; do not claim it touched the file."}}}}
			// Differential control: disabling only the failure-ID projection
			// reproduces the old touchedFiles behavior without changing outputs.
			legacyLive, legacyFull := *live, *full
			legacyLive.failedCalls, legacyFull.failedCalls = nil, nil
			legacyMade, _ := legacyLive.Compact(response, nil)
			legacyFull.Compact(response, &legacyMade)
			initialLive, _ := legacyLive.Build()
			initialFull, _ := legacyFull.Build()
			if !reflect.DeepEqual(initialLive.Request, initialFull.Request) {
				t.Fatal("control did not preserve the persisted first summary")
			}
			futureLive, _ := legacyLive.BuildCompaction("next", 0)
			futureFull, _ := legacyFull.BuildCompaction("next", 0)
			if !reflect.DeepEqual(futureLive.Request, futureFull.Request) {
				t.Fatal("control unexpectedly changed the next summary request")
			}
			legacySecondLive, _ := legacyLive.Compact(response, nil)
			legacySecondFull, _ := legacyFull.Compact(response, nil)
			if reflect.DeepEqual(legacySecondLive, legacySecondFull) {
				t.Fatal("control failed to reproduce the second-summary touched-file mismatch")
			}
			t.Log("pre-fix control: initial/future requests match, generated second summary diverges; failure-ID projection fixes it")
			made, ok := live.Compact(response, nil)
			if !ok {
				t.Fatal("live compaction failed")
			}
			full.Compact(response, &made)
			if len(live.touched) != 0 || !reflect.DeepEqual(live.touched, full.touched) {
				t.Fatalf("removed error changed touched files: live=%v full=%v", live.touched, full.touched)
			}
			encoded, err := live.Checkpoint(nil)
			if err != nil {
				t.Fatal(err)
			}
			resumed := NewBuilder().(*builder)
			if err := resumed.RestoreCheckpoint(encoded); err != nil {
				t.Fatal(err)
			}
			want, _ := full.Compact(response, nil)
			got, _ := resumed.Compact(response, nil)
			if !reflect.DeepEqual(got, want) {
				t.Fatal("second compaction resurrected a failed file touch")
			}
		})
	}
}
