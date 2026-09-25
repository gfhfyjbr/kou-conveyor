package openai

import "strings"

// ContextWindow returns how many input tokens a request to an OpenAI model
// can hold, or 0 for a model it does not know. Gateway spellings such as
// "openai/gpt-5" are understood. For the GPT-5 generation that is the input
// limit, which leaves the rest of the model's window to its output.
func ContextWindow(model string) int64 {
	id := strings.ToLower(strings.TrimSpace(model))
	id = strings.TrimPrefix(id, "openai/")
	prefixed := func(prefixes ...string) bool {
		for _, prefix := range prefixes {
			if strings.HasPrefix(id, prefix) {
				return true
			}
		}
		return false
	}
	switch {
	case prefixed("gpt-4.1"):
		return 1_047_576
	case prefixed("gpt-4o", "gpt-4-turbo", "chatgpt-4o"):
		return 128_000
	case prefixed("o1", "o3", "o4"):
		return 200_000
	case prefixed("gpt-5", "gpt-6", "codex-"):
		return 272_000
	}
	return 0
}
