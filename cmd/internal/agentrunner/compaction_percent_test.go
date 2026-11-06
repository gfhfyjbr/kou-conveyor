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

func TestCompactionPercentLimits(t *testing.T) {
	for _, test := range []struct {
		setting string
		want    int64
		bad     bool
	}{
		{setting: "", want: 96_000},
		{setting: "62.5%", want: 80_000},
		{setting: "100%", want: 128_000},
		{setting: "0.000001%", want: 1},
		{setting: "5e-324%", want: 1},
		{setting: "NaN%", bad: true},
		{setting: "+Inf%", bad: true},
		{setting: "-Inf%", bad: true},
		{setting: "101%", bad: true},
		{setting: "-1%", bad: true},
		{setting: "0%", bad: true},
	} {
		t.Run(test.setting, func(t *testing.T) {
			_, got, err := compactionLimits(func(name string) string {
				if name == autoCompactEnvironment {
					return test.setting
				}
				return ""
			}, Provider{}, "unknown")
			if (err != nil) != test.bad || got != test.want {
				t.Fatalf("percent %q = %d, %v; want %d, bad=%v", test.setting, got, err, test.want, test.bad)
			}
		})
	}
}

func TestCompactionPercentUsesPhysicalWindow(t *testing.T) {
	for _, test := range []struct {
		name, model string
		provider    Provider
		defaultAt   int64
	}{
		{name: "standard", model: "gpt-6-sol", provider: Provider{}, defaultAt: 200_000},
		{name: "claude", model: "claude-opus-5-5", provider: Provider{CompactCeiling: claudeCeiling}, defaultAt: 390_000},
	} {
		for _, setting := range []string{"", "62.5%"} {
			window, limit, err := compactionLimits(func(name string) string {
				switch name {
				case contextWindowEnvironment:
					return "1M"
				case autoCompactEnvironment:
					return setting
				}
				return ""
			}, test.provider, test.model)
			want := test.defaultAt
			if setting != "" {
				want = 625_000
			}
			if err != nil || window != 1_000_000 || limit != want {
				t.Fatalf("%s/%q: window=%d limit=%d err=%v; want 1M/%d", test.name, setting, window, limit, err, want)
			}
		}
	}
	provider := Provider{ContextWindow: func(model string) int64 {
		if model == "larger" {
			return 1_000_000
		}
		return 200_000
	}}
	for _, model := range []string{"smaller", "larger"} {
		window, limit, err := compactionLimits(func(name string) string {
			if name == autoCompactEnvironment {
				return "50%"
			}
			return ""
		}, provider, model)
		if err != nil || window != provider.ContextWindow(model) || limit != window/2 {
			t.Fatalf("model %s: window=%d limit=%d err=%v", model, window, limit, err)
		}
	}
	provider.ContextWindow = func(string) int64 { return math.MaxInt64 }
	for _, setting := range []string{"100%", "99.99999999999999%", "50%", "5e-324%"} {
		window, limit, err := compactionLimits(func(name string) string {
			if name == autoCompactEnvironment {
				return setting
			}
			return ""
		}, provider, "huge")
		if err != nil || limit < 1 || limit > window || (setting == "100%" && limit != window) {
			t.Fatalf("int64 window/%s: window=%d limit=%d err=%v", setting, window, limit, err)
		}
	}
}

func TestRunnerCompactionPercentBoundary(t *testing.T) {
	for _, percent := range []string{"50%", "62.5%", ""} {
		for _, delta := range []int64{-1, 0, 1} {
			t.Run(fmt.Sprintf("percent=%s/delta=%d", percent, delta), func(t *testing.T) {
				run := newCompactionRun(t)
				run.env[contextWindowEnvironment] = "128k"
				run.env[autoCompactEnvironment] = percent
				_, limit, err := compactionLimits(func(name string) string { return run.env[name] }, Provider{}, "unknown")
				if err != nil {
					t.Fatal(err)
				}
				// The next prompt is four tokens of framing and one text token.
				run.answer = func(request llm.Request) llm.Response {
					if compactionPromptOf(request) != "" {
						return llm.Response{Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Summary."}}}}
					}
					return llm.Response{
						Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "Answer."}}},
						Usage: llm.Usage{InputTokens: limit + delta - 5},
					}
				}
				if output, code := run.do(`{"session_id":"boundary","prompt":"a"}`); code != 0 {
					t.Fatalf("first run exited %d: %s", code, output)
				}
				output, code := run.do(`{"session_id":"boundary","prompt":"b"}`)
				if code != 0 {
					t.Fatalf("second run exited %d: %s", code, output)
				}
				var types []session.TurnType
				for _, item := range decodeLogItems(t, []byte(output)) {
					if turn, ok := item.Data.(session.Turn); ok {
						types = append(types, turn.Type)
					}
				}
				want := []session.TurnType{session.TurnRegular}
				if delta >= 0 {
					want = []session.TurnType{session.TurnCompaction, session.TurnRegular}
					if len(run.requests) != 3 || !strings.Contains(summaryOf(run.requests[2]), "Summary.") {
						t.Fatalf("compaction did not continue with its summary: %#v", run.requests)
					}
				}
				if !slices.Equal(types, want) {
					t.Fatalf("estimated=%d threshold=%d: turns=%v, want %v", limit+delta, limit, types, want)
				}
			})
		}
	}
}
