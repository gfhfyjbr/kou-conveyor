package anthropic

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"strings"

	sdk "github.com/anthropics/anthropic-sdk-go"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func responseFromMessage(message sdk.BetaMessage, inputs map[int][]byte) llm.Response {
	response := llm.Response{ID: message.ID, Usage: usage(message)}
	switch message.StopReason {
	case sdk.BetaStopReasonRefusal:
		// A decline can cut an answer off midway; what was streamed before it
		// is not an answer and must not be acted on.
		response.Stop = llm.StopRefused
		return response
	case sdk.BetaStopReasonMaxTokens:
		response.Stop = llm.StopMaxOutputTokens
	case sdk.BetaStopReasonModelContextWindowExceeded:
		response.Failure = &llm.Failure{
			Code:    "context_window_exceeded",
			Message: "the conversation no longer fits the model's context window",
		}
	default:
		response.Stop = llm.StopComplete
	}

	// After a mid-output fallback only the declined model's text carries over;
	// its thinking and tool calls must not be replayed or run.
	boundary := -1
	for index, block := range message.Content {
		if block.Type == "fallback" {
			boundary = index
		}
	}
	for index, block := range message.Content {
		if index < boundary && block.Type != "text" {
			continue
		}
		switch block.Type {
		case "text":
			if strings.TrimSpace(block.Text) != "" {
				response.Output = append(response.Output, llm.Item{
					Type: llm.ItemMessage,
					Data: llm.Message{Role: llm.RoleAssistant, Text: block.Text},
				})
			}
		case "thinking", "redacted_thinking":
			if item, ok := reasoning(block); ok {
				response.Output = append(response.Output, item)
			}
		case "tool_use":
			input := jsontext.Value(block.Input)
			if streamed, ok := inputs[index]; ok {
				input = streamed
			}
			if len(input) == 0 {
				input = jsontext.Value("{}")
			}
			if response.Stop == llm.StopMaxOutputTokens && (index == len(message.Content)-1 || !input.IsValid()) {
				// The limit cut the call off while its input was being written.
				continue
			}
			// Malformed input stays as written, for the harness to reject.
			response.Output = append(response.Output, llm.Item{
				Type: llm.ItemToolCall,
				Data: llm.ToolCall{CallID: block.ID, Name: block.Name, Arguments: string(input)},
			})
		}
	}
	return response
}

// reasoning records a thinking block verbatim for replay, with its summary
// for display.
func reasoning(block sdk.BetaContentBlockUnion) (llm.Item, bool) {
	var raw []byte
	var err error
	var summary []string
	if block.Type == "thinking" {
		if block.Signature == "" {
			return llm.Item{}, false
		}
		raw, err = json.Marshal(struct {
			Type      string `json:"type"`
			Thinking  string `json:"thinking"`
			Signature string `json:"signature"`
		}{"thinking", block.Thinking, block.Signature})
		if strings.TrimSpace(block.Thinking) != "" {
			summary = []string{block.Thinking}
		}
	} else {
		if block.Data == "" {
			return llm.Item{}, false
		}
		raw, err = json.Marshal(struct {
			Type string `json:"type"`
			Data string `json:"data"`
		}{"redacted_thinking", block.Data})
	}
	if err != nil {
		return llm.Item{}, false
	}
	return llm.Item{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: summary, Raw: raw}}, true
}

// usage converts to the harness's accounting, where input includes the cached
// and cache-written tokens that the Messages API counts separately.
func usage(message sdk.BetaMessage) llm.Usage {
	source := message.Usage
	converted := llm.Usage{
		InputTokens:           source.InputTokens + source.CacheReadInputTokens + source.CacheCreationInputTokens,
		CachedInputTokens:     source.CacheReadInputTokens,
		CacheWriteInputTokens: source.CacheCreationInputTokens,
		OutputTokens:          source.OutputTokens,
		ReasoningTokens:       source.OutputTokensDetails.ThinkingTokens,
	}
	var envelope struct {
		Usage jsontext.Value `json:"usage"`
	}
	if json.Unmarshal([]byte(message.RawJSON()), &envelope) == nil && envelope.Usage.Kind() == jsontext.KindBeginObject {
		converted.Raw = envelope.Usage
	}
	return converted
}
