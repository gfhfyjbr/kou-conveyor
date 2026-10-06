package transcript

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const sample = `{"type":"session","data":{"Version":2,"Session":{"ID":"s","CreatedAt":"2026-08-27T10:00:00Z"}}}
{"type":"item","data":{"Item":{"Sequence":1,"RecordedAt":"2026-08-27T10:01:00Z","Kind":"input","Data":{"ID":"i1","Kind":"external","Payload":"Fix the flaky TestRetry please"}}}}
{"type":"item","data":{"Item":{"Sequence":2,"RecordedAt":"2026-08-27T10:01:00Z","Kind":"turn","Data":{"ID":"t1","PreviousTurnID":"","Type":"regular"}}}}
{"type":"item","data":{"Item":{"Sequence":3,"RecordedAt":"2026-08-27T10:01:00Z","Kind":"model_response","Data":{"TurnID":"t1","Response":{"ID":"r1","Stop":"complete","Output":[{"Type":"message","Data":{"Role":"assistant","Text":"Looking at TestRetry now.\nSecond line."}},{"Type":"tool_call","Data":{"CallID":"c1","Name":"Bash","Arguments":"{\"command\":\"go test -run TestRetry ./...\"}"}}]}}}}}
{"type":"operation","data":{"Operation":{"ID":"op1","Type":"shell","Version":3,"Status":"completed","State":{"Input":{"Command":"go test"},"Result":{"Out":"--- FAIL: TestRetry (0.00s)\n    retry_test.go:12: expected 3 attempts\nFAIL","Err":"","ExitCode":1}}}}}
{"type":"operation","data":{"Operation":{"ID":"op2","Type":"view_image","Version":1,"Status":"completed","State":{"Path":"a.png","Result":{"Content":"TestRetryBASE64","OriginalWidth":1}}}}}
{"type":"item","data":{"Item":{"Sequence":4,"RecordedAt":"2026-08-27T10:01:00Z","Kind":"input","Data":{"ID":"i2","Kind":"external","Payload":{"Text":"Also check [Image 1]","Images":[{"Label":"[Image 1]","URL":"data:image/png;base64,TestRetryXX"}]}}}}}
`

func TestSearchFindsTextInEveryKindOfRecord(t *testing.T) {
	path := filepath.Join(t.TempDir(), "s.session.jsonl")
	if err := os.WriteFile(path, []byte(sample), 0o600); err != nil {
		t.Fatal(err)
	}
	result, err := Search(path, "testretry", 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		"4 matches for \"testretry\"",
		"[user · record 1, line 1]\n> Fix the flaky TestRetry please",
		"[assistant · record 3, line 1]\n> Looking at TestRetry now.\n  Second line.",
		"[tool call Bash · record 3, line 1]",
		"[tool result shell, line 1]\n> --- FAIL: TestRetry (0.00s)\n      retry_test.go:12: expected 3 attempts",
	} {
		if !strings.Contains(result, want) {
			t.Fatalf("result lacks %q:\n%s", want, result)
		}
	}
	if strings.Contains(result, "BASE64") || strings.Contains(result, "TestRetryXX") {
		t.Fatalf("images were searched:\n%s", result)
	}
	limited, err := Search(path, "TestRetry", 2)
	if err != nil || !strings.Contains(limited, "the first 2 shown") {
		t.Fatalf("limited = %q, err = %v", limited, err)
	}
	none, err := Search(path, "nothing-here", 0)
	if err != nil || !strings.HasPrefix(none, "No match") {
		t.Fatalf("none = %q, err = %v", none, err)
	}
	if _, err := Search(path, "(", 0); err == nil {
		t.Fatal("a bad pattern was accepted")
	}
	if _, err := Search(filepath.Join(t.TempDir(), "missing"), "x", 0); err == nil {
		t.Fatal("a missing transcript was accepted")
	}
}

func TestSearchLeavesOutTheSearches(t *testing.T) {
	searches := `{"type":"item","data":{"Item":{"Sequence":5,"RecordedAt":"2026-08-27T10:02:00Z","Kind":"model_response","Data":{"TurnID":"t2","Response":{"ID":"r2","Stop":"complete","Output":[{"Type":"tool_call","Data":{"CallID":"c2","Name":"TranscriptSearch","Arguments":"{\"query\":\"TestRetry\"}"}},{"Type":"tool_call","Data":{"CallID":"c3","Name":"mcp__kou__mistake_TranscriptSearch","Arguments":"{\"query\":\"TestRetry\"}"}}]}}}}}
{"type":"operation","data":{"Operation":{"ID":"op3","Type":"file","Version":1,"Status":"completed","State":{"Action":"transcript_search","Path":"s.session.jsonl","Query":"TestRetry","Result":{"Text":"4 matches for \"TestRetry\" in the transcript"}}}}}
{"type":"operation","data":{"Operation":{"ID":"op4","Type":"code","Version":1,"Status":"completed","State":{"Code":"x","Result":{"Calls":[{"Index":0,"Name":"transcriptSearch","Arguments":"TestRetry","Output":"found TestRetry"},{"Index":1,"Name":"bash","Arguments":"echo","Output":"TestRetry in bash"}]}}}}}
`
	path := filepath.Join(t.TempDir(), "s.session.jsonl")
	if err := os.WriteFile(path, []byte(sample+searches), 0o600); err != nil {
		t.Fatal(err)
	}
	result, err := Search(path, "testretry", 0)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(result, "5 matches for \"testretry\"") || !strings.Contains(result, "TestRetry in bash") ||
		strings.Contains(result, "TranscriptSearch") || strings.Contains(result, "4 matches") || strings.Contains(result, "found TestRetry") {
		t.Fatalf("result:\n%s", result)
	}
}
