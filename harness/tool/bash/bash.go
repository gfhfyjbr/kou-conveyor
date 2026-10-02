package bash

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type Config struct {
	Shell         string
	Directory     string
	BaseDirectory string
	// DefaultTimeout is how many seconds a command may run unless the
	// call says otherwise; 0 lets commands run.
	DefaultTimeout float64
}

type translator struct {
	config Config
}

func New(config Config) tool.Translator {
	return &translator{config: config}
}

func (translator *translator) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	command, limit, timeout, err := validateArguments(call.Arguments)
	if err != nil {
		return tool.ErrorStatus(err.Error(), limit)
	}
	if timeout < 0 {
		timeout = translator.config.DefaultTimeout
	}
	spec, err := translator.buildOperation(command, limit, timeout)
	if err != nil {
		return tool.ErrorStatus(err.Error(), limit)
	}

	id := ctx.Submit(spec)
	return tool.CallStatus{WaitingFor: []operation.ID{id}}
}

func (translator *translator) TranslateResult(
	callID string,
	status tool.CallStatus,
	operations []operation.Operation,
) (llm.ToolResult, error) {
	if status.Error != "" {
		if len(operations) != 0 {
			return llm.ToolResult{}, fmt.Errorf("bash tool call %q has both a validation error and operations", callID)
		}
		return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "Error: " + status.Error}}}, nil
	}
	if len(operations) != 1 {
		return llm.ToolResult{}, fmt.Errorf("bash tool call %q has %d operations, want 1", callID, len(operations))
	}

	output, err := FormatResult(callID, operations[0])
	if err != nil {
		return llm.ToolResult{}, err
	}
	return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: output}}}, nil
}

// DescribeRunning says where a running command's output so far is, and how
// to stop it; nothing until the command started.
func (translator *translator) DescribeRunning(operations []operation.Operation) string {
	if len(operations) != 1 {
		return ""
	}
	state, err := operation.DecodeShellState(operations[0])
	if err != nil {
		return ""
	}
	return strings.TrimSpace(runningDetail(state))
}

// FormatResult is what the model reads of a shell operation: its output, its
// standard error, a nonzero exit code and any failure.
func FormatResult(
	callID string,
	current operation.Operation,
) (string, error) {
	if current.Type != operation.TypeShell {
		return "", fmt.Errorf(
			"bash tool call %q operation %q has type %q, want %q",
			callID,
			current.ID,
			current.Type,
			operation.TypeShell,
		)
	}

	state, err := operation.DecodeShellState(current)
	if err != nil {
		return "", fmt.Errorf("decode Bash operation %q state: %w", current.ID, err)
	}
	switch current.Status {
	case operation.StatusReady, operation.StatusAwaiting, operation.StatusCanceling:
		return runningText(state), nil
	case operation.StatusCompleted:
		if state.Result == nil {
			return "", fmt.Errorf("bash tool call %q completed operation %q has no result", callID, current.ID)
		}
	case operation.StatusFailed, operation.StatusCanceled:
		if state.TerminalError == "" {
			state.TerminalError = "shell operation " + string(current.Status)
		}
	default:
		return "", fmt.Errorf("bash tool call %q operation %q has invalid status %q", callID, current.ID, current.Status)
	}

	var parts []string
	if state.Result != nil {
		if state.Result.Out != "" {
			parts = append(parts, state.Result.Out)
		}
		if state.Result.Err != "" {
			parts = append(parts, "Stderr:\n"+state.Result.Err)
		}
		if state.Result.ExitCode != 0 {
			parts = append(parts, fmt.Sprintf("Exit code: %d", state.Result.ExitCode))
		}
		if state.Result.TimedOut {
			parts = append(parts, fmt.Sprintf("The command was stopped at its timeout of %s.", seconds(state.Input.Timeout)))
		}
		if state.Result.Duration >= 2 {
			parts = append(parts, "Took "+seconds(state.Result.Duration)+".")
		}
	} else {
		if state.OutPath != "" {
			parts = append(parts, "Stdout capture: "+state.OutPath)
		}
		if state.ErrPath != "" {
			parts = append(parts, "Stderr capture: "+state.ErrPath)
		}
	}
	if state.TerminalError != "" {
		parts = append(parts, "Error: "+state.TerminalError)
	}
	if len(parts) == 0 {
		return "(no output)", nil
	}
	return strings.Join(parts, "\n"), nil
}

// runningText is what the model reads of a command still running: how to
// look at its output so far, and how to stop it.
func runningText(state operation.ShellState) string {
	var text strings.Builder
	text.WriteString("Command is still running")
	if !state.StartedAt.IsZero() {
		fmt.Fprintf(&text, " (%s so far)", seconds(time.Since(state.StartedAt).Seconds()))
	}
	text.WriteString(".")
	text.WriteString(runningDetail(state))
	return text.String()
}

// runningDetail is what is known of a running command beyond that it
// runs: where its output so far is, and how to stop it. It is empty until
// the command started.
func runningDetail(state operation.ShellState) string {
	var text strings.Builder
	if state.OutPath != "" {
		fmt.Fprintf(&text, " Its output so far is in %s (stdout) and %s (stderr): read them with tail.", state.OutPath, state.ErrPath)
	}
	if state.ProcessGroupID > 0 {
		fmt.Fprintf(&text, " To stop it: kill -TERM -- -%d", state.ProcessGroupID)
	}
	return text.String()
}

// seconds says a duration given in seconds.
func seconds(value float64) string {
	switch {
	case value < 10:
		return fmt.Sprintf("%.1f s", value)
	case value < 60:
		return fmt.Sprintf("%.0f s", value)
	}
	return time.Duration(value * float64(time.Second)).Round(time.Second).String()
}

// validateArguments reads a call's command, output limit and timeout in
// seconds; the timeout is -1 when the call leaves it to the translator.
func validateArguments(encoded string) (string, int, float64, error) {
	var arguments map[string]jsontext.Value
	if err := json.Unmarshal([]byte(encoded), &arguments); err != nil {
		return "", 0, -1, fmt.Errorf("decode Bash arguments: %w", err)
	}
	limit, err := tool.ParseMaxOutputLength(arguments["max_output_length"])
	if err != nil {
		return "", 0, -1, fmt.Errorf("bash argument: %w", err)
	}
	timeout := -1.0
	if encodedTimeout, exists := arguments["timeout"]; exists && len(encodedTimeout) != 0 && string(encodedTimeout) != "null" {
		if err := json.Unmarshal(encodedTimeout, &timeout); err != nil {
			return "", limit, -1, fmt.Errorf(`decode Bash argument "timeout": %w`, err)
		}
		if timeout < 0 || timeout > operation.MaxShellTimeout {
			return "", limit, -1, fmt.Errorf(`bash argument "timeout" must be between 0 and %d seconds`, operation.MaxShellTimeout)
		}
	}
	encodedCommand, exists := arguments["command"]
	if !exists {
		return "", limit, timeout, errors.New(`bash argument "command" must be set`)
	}
	var command *string
	if err := json.Unmarshal(encodedCommand, &command); err != nil {
		return "", limit, timeout, fmt.Errorf(`decode Bash argument "command": %w`, err)
	}
	if command == nil {
		return "", limit, timeout, errors.New(`bash argument "command" must be a string`)
	}
	if offset := strings.IndexByte(*command, 0); offset >= 0 {
		return "", limit, timeout, fmt.Errorf(`bash argument "command" contains a NUL byte at offset %d`, offset)
	}
	return *command, limit, timeout, nil
}

func (translator *translator) buildOperation(command string, limit int, timeout float64) (operation.Spec, error) {
	spec, err := operation.NewShellSpec(operation.ShellInput{
		Command:   command,
		Shell:     translator.config.Shell,
		Directory: translator.config.Directory,
		Timeout:   timeout,
	}, translator.config.BaseDirectory, limit)
	if err != nil {
		return operation.Spec{}, fmt.Errorf("build Bash operation: %w", err)
	}
	return spec, nil
}
