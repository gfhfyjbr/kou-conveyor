package agentrunner

import (
	"math"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

func TestCompactionServedContextWindowPrecedence(t *testing.T) {
	provider := Provider{ContextWindow: func(model string) int64 {
		if model == "known" {
			return 400_000
		}
		return 0
	}}
	for _, test := range []struct {
		name, hint, explicit, model, setting string
		window, limit                        int64
		bad                                  bool
	}{
		{name: "unknown alias served at 1M", hint: "1000000", window: 1_000_000, limit: 967_000},
		{name: "served capacity beats builtin", hint: "1000000", model: "known", window: 1_000_000, limit: 967_000},
		{name: "explicit window beats served capacity", hint: "1000000", explicit: "300k", window: 300_000, limit: 267_000},
		{name: "invalid explicit window still fails", hint: "1000000", explicit: "wrong", bad: true},
		{name: "zero explicit window still fails", hint: "1000000", explicit: "0", bad: true},
		{name: "absent capacity uses builtin", model: "known", window: 400_000, limit: 367_000},
		{name: "absent capacity uses fallback", window: 128_000, limit: 96_000},
		{name: "zero capacity uses fallback", hint: "0", window: 128_000, limit: 96_000},
		{name: "negative capacity uses fallback", hint: "-1", window: 128_000, limit: 96_000},
		{name: "invalid hint uses builtin", hint: "wrong", model: "known", window: 400_000, limit: 367_000},
		{name: "fractional hint is not metadata", hint: "1000000.5", window: 128_000, limit: 96_000},
		{name: "overflowing hint uses fallback", hint: "9223372036854775808", window: 128_000, limit: 96_000},
		{name: "largest hint is safe", hint: "9223372036854775807", window: math.MaxInt64, limit: math.MaxInt64 - 33_000},
		{name: "90 percent", hint: "1000000", setting: "90%", window: 1_000_000, limit: 900_000},
		{name: "100 percent", hint: "1000000", setting: "100%", window: 1_000_000, limit: 1_000_000},
		{name: "off", hint: "1000000", setting: "off", window: 1_000_000},
		{name: "zero disables compaction", hint: "1000000", setting: "0", window: 1_000_000},
		{name: "tokens remain explicit", hint: "1000000", setting: "150k", window: 1_000_000, limit: 150_000},
	} {
		t.Run(test.name, func(t *testing.T) {
			getenv := func(name string) string {
				switch name {
				case contextWindowEnvironment:
					return test.explicit
				case runconfig.ServedContextWindowEnvironment:
					return test.hint
				case autoCompactEnvironment:
					return test.setting
				}
				return ""
			}
			model := test.model
			if model == "" {
				model = "served-only-alias"
			}
			window, limit, err := compactionLimits(getenv, provider, model)
			if test.bad {
				if err == nil || !strings.Contains(err.Error(), contextWindowEnvironment) {
					t.Fatalf("explicit window was not rejected: %v", err)
				}
				return
			}
			if err != nil || window != test.window || limit != test.limit {
				t.Fatalf("window=%d limit=%d err=%v; want %d/%d", window, limit, err, test.window, test.limit)
			}
		})
	}
}

func TestRunnerUsesServedContextForUnknownAlias(t *testing.T) {
	run := newCompactionRun(t)
	run.env[runconfig.ServedContextWindowEnvironment] = "1000000"
	run.answer = func(request llm.Request) llm.Response {
		if request.Model.ID != "served-only-alias" {
			t.Fatalf("request model = %q", request.Model.ID)
		}
		if compactionPromptOf(request) != "" {
			t.Fatal("served 1M alias compacted at the unknown-model or former fixed limit")
		}
		return llm.Response{
			Stop: llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{
				Role: llm.RoleAssistant, Text: "Answer.",
			}}},
			Usage: llm.Usage{InputTokens: 250_000},
		}
	}
	for _, prompt := range []string{"a", "b"} {
		output, code := run.do(`{"session_id":"served-alias","model":"served-only-alias","prompt":"` + prompt + `"}`)
		if code != 0 {
			t.Fatalf("run exited %d: %s", code, output)
		}
		var turns []session.TurnType
		for _, item := range decodeLogItems(t, []byte(output)) {
			if turn, ok := item.Data.(session.Turn); ok {
				turns = append(turns, turn.Type)
			}
		}
		if !slices.Equal(turns, []session.TurnType{session.TurnRegular}) {
			t.Fatalf("turns = %v", turns)
		}
	}
	if len(run.requests) != 2 || summaryOf(run.requests[1]) != "" {
		t.Fatalf("the served alias lost its uncompressed history: %d requests", len(run.requests))
	}
}

func TestServedContextWindowEmptySentinelBlocksDotEnv(t *testing.T) {
	t.Setenv(runconfig.ServedContextWindowEnvironment, "")
	t.Setenv(contextWindowEnvironment, "")
	t.Setenv(autoCompactEnvironment, "")
	path := filepath.Join(t.TempDir(), ".env")
	if err := os.WriteFile(path, []byte(runconfig.ServedContextWindowEnvironment+"=1000000\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	scope, err := loadDotEnv(path)
	if err != nil {
		t.Fatal(err)
	}
	defer scope.Close()
	window, limit, err := compactionLimits(os.Getenv, Provider{}, "served-only-alias")
	if err != nil || window != 128_000 || limit != 96_000 {
		t.Fatalf("workspace .env restored a stale serving capacity: %d/%d %v", window, limit, err)
	}
}
