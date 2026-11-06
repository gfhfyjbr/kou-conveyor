package agentrunner

import (
	"fmt"
	"math"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

func TestCompactionDefaultWindowEdges(t *testing.T) {
	for _, test := range []struct {
		window, wantWindow, want int64
	}{
		{0, 128_000, 96_000},
		{-1, 128_000, 96_000},
		{1, 1, 1},
		{3, 3, 3},
		{4, 4, 3},
		{32_000, 32_000, 24_000},
		{128_000, 128_000, 96_000},
		{132_000, 132_000, 99_000},
		{200_000, 200_000, 167_000},
		{1_000_000, 1_000_000, 967_000},
		{math.MaxInt64, math.MaxInt64, math.MaxInt64 - 33_000},
	} {
		t.Run(fmt.Sprint(test.window), func(t *testing.T) {
			provider := Provider{ContextWindow: func(string) int64 { return test.window }}
			window, limit, err := compactionLimits(func(string) string { return "" }, provider, "model")
			if err != nil || window != test.wantWindow || limit != test.want {
				t.Fatalf("window=%d limit=%d err=%v; want %d/%d", window, limit, err, test.wantWindow, test.want)
			}
		})
	}
}

func TestPruneBudgetUsesCompactionThreshold(t *testing.T) {
	for _, test := range []struct {
		setting string
		limit   int64
		budget  int64
	}{
		{"", 967_000, 676_900},
		{"90%", 900_000, 630_000},
		{"100%", 1_000_000, 700_000},
		{"off", 0, 0},
		{"0", 0, 0},
		{"150k", 150_000, 105_000},
	} {
		t.Run(test.setting, func(t *testing.T) {
			_, limit, err := compactionLimits(func(name string) string {
				switch name {
				case contextWindowEnvironment:
					return "1M"
				case autoCompactEnvironment:
					return test.setting
				}
				return ""
			}, Provider{}, "unknown")
			if err != nil || limit != test.limit || pruneBudget(limit) != test.budget {
				t.Fatalf("limit=%d budget=%d err=%v; want %d/%d", limit, pruneBudget(limit), err, test.limit, test.budget)
			}
		})
	}
	if got := pruneBudget(math.MaxInt64); got != 6_456_360_425_798_343_064 {
		t.Fatalf("large pruning budget = %d", got)
	}
}

func TestRunnerDefaultCompactionUsesWholeWindow(t *testing.T) {
	for _, estimated := range []int64{200_001, 390_001, 966_999, 967_000, 967_001} {
		t.Run(fmt.Sprint(estimated), func(t *testing.T) {
			run := newCompactionRun(t)
			run.env[contextWindowEnvironment] = "1M"
			run.answer = func(request llm.Request) llm.Response {
				if compactionPromptOf(request) != "" {
					return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Summary."}}}}
				}
				return llm.Response{
					Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Answer."}}},
					// The second prompt adds four framing tokens and one text token.
					Usage: llm.Usage{InputTokens: estimated - 5},
				}
			}
			if output, code := run.do(`{"session_id":"whole-window","prompt":"a"}`); code != 0 {
				t.Fatalf("first run exited %d: %s", code, output)
			}
			output, code := run.do(`{"session_id":"whole-window","prompt":"b"}`)
			if code != 0 {
				t.Fatalf("second run exited %d: %s", code, output)
			}
			var turns []session.TurnType
			for _, item := range decodeLogItems(t, []byte(output)) {
				if turn, ok := item.Data.(session.Turn); ok {
					turns = append(turns, turn.Type)
				}
			}
			want := []session.TurnType{session.TurnRegular}
			requests := 2
			if estimated >= 967_000 {
				want = []session.TurnType{session.TurnCompaction, session.TurnRegular}
				requests = 3
				if !strings.Contains(summaryOf(run.requests[len(run.requests)-1]), "Summary.") {
					t.Fatal("the continuation lost its summary")
				}
			} else if summaryOf(run.requests[len(run.requests)-1]) != "" {
				t.Fatal("the history was summarized before the window's threshold")
			}
			if !slices.Equal(turns, want) || len(run.requests) != requests {
				t.Fatalf("estimated=%d: turns=%v requests=%d; want %v/%d", estimated, turns, len(run.requests), want, requests)
			}
		})
	}
}
