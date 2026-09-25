package agentrunner

import (
	"flag"
	"fmt"
)

const requestHelp = `
Request schema (JSON object; unknown fields are rejected):
  messages: array of {role: "user", content: string, message_id?: UUID string, images?: array}
    Non-empty array of user messages delivered in order. role defaults to "user";
    message_id defaults to a generated UUID.
    images: up to 20 of {label?: string, media_type: string, data: base64 string}
    or {label?: string, url: http(s) URL}, pictures the model sees beside the
    content, each after its label ("[Image n]" by default), which is how the
    content refers to it. media_type is image/png, image/jpeg, image/gif or
    image/webp, and data at most 20 MiB.
  prompt: string
    Shorthand for one user message; used when messages is absent.
    Supply messages or prompt. messages takes precedence when both are present.
  model: string (optional)
    Provider model ID; defaults to KOU_CONVEYOR_LLM_MODEL or the provider default.
  max_attempts: positive integer (optional)
    Overrides KOU_CONVEYOR_LLM_MAX_ATTEMPTS (default 5); 1 disables retries.
  system_prompt: string (optional)
    Replaces the default system prompt.
  thinking_level: "low" | "medium" | "high" | "xhigh" | "max" (optional; default "high")
  session_id: non-empty string (optional)
    Creates or resumes a persisted session.
  disallowed_tools: array of non-empty strings (optional)
    Static tool names excluded from model context and execution.
  extra_allowed_tools: array of non-empty strings (optional; accepted but ignored)
  include_partial_messages: boolean (optional; accepted but ignored)
  compact: boolean (optional)
    Summarizes the conversation of session_id, which must exist, to free the
    context it takes; messages, if any, run after it. Without messages or
    prompt the run ends once the summary is written.
  compact_instructions: string (optional; needs compact)
    What the summary should focus on.

Environment:
  KOU_CONVEYOR_CONTEXT_WINDOW: tokens a request can hold (default: known per
    model, otherwise 128000).
  KOU_CONVEYOR_AUTO_COMPACT: when to compact the conversation automatically
    before a turn: off, a number of tokens (150000 or 150k), or a share of the
    context window such as 80% (default: 33000 tokens short of the window, or
    three quarters of a window smaller than 132000).
`

func writeUsage(flags *flag.FlagSet) error {
	if _, err := fmt.Fprintf(flags.Output(), `Usage:
  %[1]s [options] < request.json
  %[1]s [options] 'JSON request'
  %[1]s [options] -p 'prompt'

Reads one JSON request from stdin unless a positional request or -p is supplied.
Place options before the positional request. -p and a positional request are mutually exclusive.
With -steer, stdin stays open after the request (or is only messages, with -p
or a positional request): each further JSON object, one per line,
{"content": string, "message_id"?: UUID, "images"?: array}, reaches the running agent once the
tool calls it is making finish (or -steer-wait passes), and never cuts a
response short.

Options:
`, flags.Name()); err != nil {
		return fmt.Errorf("write usage: %w", err)
	}
	flags.PrintDefaults()
	if _, err := fmt.Fprint(flags.Output(), requestHelp); err != nil {
		return fmt.Errorf("write request schema: %w", err)
	}
	return nil
}
