package openai

import "testing"

func TestContextWindowFollowsTheModel(t *testing.T) {
	for model, want := range map[string]int64{
		"gpt-5":          272_000,
		"gpt-5.1-codex":  272_000,
		"gpt-6-astra":    272_000,
		"openai/gpt-4.1": 1_047_576,
		"gpt-4o-mini":    128_000,
		"o4-mini":        200_000,
		"claude-opus-5":  0,
		"llama3.1:8b":    0,
		"":               0,
	} {
		if got := ContextWindow(model); got != want {
			t.Errorf("ContextWindow(%q) = %d, want %d", model, got, want)
		}
	}
}
