package tool

import (
	"fmt"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

type unavailableTranslator struct {
	name string
}

func (translator unavailableTranslator) Translate(Context, llm.ToolCall) CallStatus {
	return CallStatus{Error: translator.errorMessage()}
}

func (translator unavailableTranslator) TranslateResult(
	callID string,
	_ CallStatus,
	_ []operation.Operation,
) (llm.ToolResult, error) {
	return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: translator.errorMessage()}}}, nil
}

func (translator unavailableTranslator) errorMessage() string {
	return fmt.Sprintf("static tool %q is not configured", translator.name)
}

// DefaultShellTimeout is how many seconds a Bash command may run unless the
// call or the configuration says otherwise.
const DefaultShellTimeout = 10 * 60

func StaticNames() []string {
	definitions := staticDefinitions()
	names := make([]string, 0, len(definitions))
	for _, definition := range definitions {
		names = append(names, definition.Tool.Name)
	}
	return names
}

func staticDefinitions() []Definition {
	return []Definition{
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        BashName,
			Description: "Execute a shell command in background. Independent commands may be issued as parallel tool calls in one turn. Command child processes are killed when the shell exits.",
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"command": map[string]any{
						"type":        "string",
						"description": "The shell command to execute.",
					},
					"max_output_length": maxOutputLengthSchema(),
					"timeout": map[string]any{
						"type":        "number",
						"description": fmt.Sprintf("Seconds the command may run before it is stopped (SIGTERM, then SIGKILL). Defaults to %d; 0 lets it run; at most %d.", DefaultShellTimeout, operation.MaxShellTimeout),
						"minimum":     0,
						"maximum":     operation.MaxShellTimeout,
					},
				},
				"required": []any{"command"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        ReadName,
			Description: ReadDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"path":   map[string]any{"type": "string", "description": "The file to read, absolute or relative to the workspace."},
					"offset": map[string]any{"type": "integer", "description": "The line to start at, counting from 1. Defaults to the first line.", "minimum": 1},
					"limit":  map[string]any{"type": "integer", "description": "How many lines to show. Defaults to 2000.", "minimum": 1},
				},
				"required": []any{"path"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        EditName,
			Description: EditDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"path":        map[string]any{"type": "string", "description": "The file to edit, absolute or relative to the workspace."},
					"old_string":  map[string]any{"type": "string", "description": "The text to replace, exactly as it is in the file."},
					"new_string":  map[string]any{"type": "string", "description": "The text that takes its place."},
					"replace_all": map[string]any{"type": "boolean", "description": "Replace every occurrence of old_string instead of requiring exactly one.", "default": false},
				},
				"required": []any{"path", "old_string", "new_string"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        WriteName,
			Description: WriteDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"path":    map[string]any{"type": "string", "description": "The file to write, absolute or relative to the workspace."},
					"content": map[string]any{"type": "string", "description": "The whole content of the file."},
				},
				"required": []any{"path", "content"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        ApplyPatchName,
			Description: ApplyPatchDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"input": map[string]any{"type": "string", "description": "The patch, from *** Begin Patch to *** End Patch."},
				},
				"required": []any{"input"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        ViewImageName,
			Description: "View a local JPEG, PNG, BMP, TIFF, or WebP image.",
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"path": map[string]any{
						"type":        "string",
						"description": "Image file path, absolute or relative to the workspace.",
					},
				},
				"required": []any{"path"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        TranscriptSearchName,
			Description: TranscriptSearchDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"query": map[string]any{"type": "string", "description": "A regular expression (Go/RE2 syntax, case-insensitive) to look for."},
					"limit": map[string]any{"type": "integer", "description": "How many matches to show at most. Defaults to 20.", "minimum": 1, "maximum": 200},
				},
				"required": []any{"query"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        CodeName,
			Description: CodeDescription,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"code": map[string]any{"type": "string", "description": "The JavaScript to run."},
				},
				"required": []any{"code"},
			},
		}},
		{Tool: llm.Tool{
			Type:        llm.ToolFunction,
			Name:        SkillUseName,
			Description: "Load the instructions for a registered skill.",
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"name": map[string]any{
						"type":        "string",
						"description": "The exact name of the skill to load.",
					},
				},
				"required": []any{"name"},
			},
		}},
	}
}

// The descriptions of the file tools, the code tool and the transcript
// search: what the model reads about them.
const (
	ReadDescription = "Read a text file: its lines, numbered, from `offset` (a line number, 1 by default) up to `limit` lines (2000 by default). Long lines are cut at 2000 bytes. Prefer it to cat, sed -n or head in Bash: it is cheaper and its line numbers can be quoted. ViewImage shows images."

	EditDescription = "Replace text in a file. `old_string` must match the file exactly, whitespace and indentation included and without the line numbers Read shows, and appear exactly once unless `replace_all` is set: include enough surrounding lines to make it unique. Read the file before editing it. The result shows the lines around the change. Use it instead of sed, python or heredocs for changes to existing files."

	WriteDescription = "Create a file or replace its whole content, creating missing directories. To change part of an existing file use Edit, which keeps the rest as it is."

	ApplyPatchDescription = "Apply a patch to files of the workspace, in this format:\n\n*** Begin Patch\n*** Update File: path/to/file.py\n@@ def function_name():\n context line, a space first\n-removed line\n+added line\n*** Add File: path/to/new.txt\n+each line of the new file, a + first\n*** Delete File: path/to/old.txt\n*** End Patch\n\nPaths are relative to the workspace. In an Update File, give 3 lines of context before and after each change; a line starting with @@ names the function or class the change is in, to find the right place when the context alone is ambiguous (several @@ lines stack for nested scopes); write *** End of File after a change that reaches the end of the file. *** Move to: new/path right after *** Update File: renames the file. A patch that does not apply changes nothing and says which section was not found; read the file again then and copy its lines exactly."

	CodeDescription = "Run JavaScript in an isolated VM whose only capabilities are the functions declared in the system prompt under \"Code tool\": they run the tools (shell commands, files, images, skills) and return their results as values, so one call can do several steps, branch on results and run independent work at once with Promise.all. Write the body of an async function: `await` the calls, `console.log` what should be reported, and `return` a value (a string or JSON) that the result ends with. The result lists every tool call the code made; call outputs are shown only for calls that failed, or when the code logs and returns nothing."

	TranscriptSearchDescription = "Search the session's full transcript, including what was compacted away or pruned from the context: the user's messages, your messages, tool calls and their outputs. Returns the matching lines with their context. Use it to recover exact code, commands, errors or outputs from earlier in the session."
)

func maxOutputLengthSchema() map[string]any {
	return map[string]any{
		"type":        "integer",
		"description": fmt.Sprintf("Maximum characters per output text field. Truncated text keeps its head and tail, around a marker stating how much was omitted, and path to the file with the complete stream. Defaults to %d.", operation.DefaultMaxOutputLength),
		"minimum":     1,
		"maximum":     operation.MaxOutputLength,
		"default":     operation.DefaultMaxOutputLength,
	}
}
