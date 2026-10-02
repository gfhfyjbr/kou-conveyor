// Package code gives the model the Code tool of code mode: one call runs
// JavaScript in an isolated VM (codevm) whose functions are the tools.
// The call is a code operation, which keeps every tool call the code
// makes.
package code

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type Config struct {
	// Shell runs the code's shell commands, in Directory, with Environment
	// (nil inherits the runner's).
	Shell       string
	Directory   string
	Environment []string
	// Transcript is the session's transcript, for transcriptSearch().
	Transcript string
	// Image bounds the images viewImage() reads.
	Image operation.ViewImageConfig
	// MaxOutputLength bounds each stream of a shell command, and the
	// result.
	MaxOutputLength int
	// CommandTimeout and RunTimeout are in seconds; zero picks the
	// defaults.
	CommandTimeout float64
	RunTimeout     float64
	// Skills and Tools say what the code may call at the time of a call:
	// the skills by name, and the plugins' tools.
	Skills func() map[string]string
	Tools  func() []codevm.CommandTool
}

type translator struct {
	config Config
}

func New(config Config) tool.Translator {
	if config.MaxOutputLength <= 0 {
		config.MaxOutputLength = operation.DefaultMaxOutputLength
	}
	return &translator{config: config}
}

// VMConfig is what the code may do now, as the system prompt declares it.
func (translator *translator) VMConfig() codevm.Config {
	config := codevm.Config{
		Shell: translator.config.Shell, Directory: translator.config.Directory, Environment: translator.config.Environment,
		MaxOutputLength: translator.config.MaxOutputLength, CommandTimeout: translator.config.CommandTimeout, RunTimeout: translator.config.RunTimeout,
		Transcript: translator.config.Transcript,
	}
	if translator.config.Skills != nil {
		config.Skills = translator.config.Skills()
	}
	if translator.config.Tools != nil {
		config.Tools = translator.config.Tools()
	}
	return config
}

// Declarations is the API the system prompt shows for the code.
func Declarations(translator tool.Translator) string {
	if current, ok := translator.(interface{ VMConfig() codevm.Config }); ok {
		return codevm.Declarations(current.VMConfig())
	}
	return codevm.Declarations(codevm.Config{})
}

func (translator *translator) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	encoded := strings.TrimSpace(call.Arguments)
	if encoded == "" {
		encoded = "{}"
	}
	if value := jsontext.Value(encoded); value.Kind() != '{' || !value.IsValid() {
		return tool.ErrorStatus("Code arguments must be a JSON object", 0)
	}
	var arguments struct {
		Code string `json:"code"`
	}
	if err := json.Unmarshal([]byte(encoded), &arguments); err != nil {
		return tool.ErrorStatus(fmt.Sprintf("decode Code arguments: %v", err), 0)
	}
	if strings.TrimSpace(arguments.Code) == "" {
		return tool.ErrorStatus(`Code argument "code" must be set`, 0)
	}
	spec, err := operation.NewCodeSpec(operation.CodeState{
		Code: arguments.Code, Config: translator.VMConfig(), Image: translator.config.Image,
	}, translator.config.MaxOutputLength)
	if err != nil {
		return tool.ErrorStatus(fmt.Sprintf("build Code operation: %v", err), 0)
	}
	return tool.CallStatus{WaitingFor: []operation.ID{ctx.Submit(spec)}}
}

// DescribeRunning says what a running code call has done so far.
func (translator *translator) DescribeRunning(operations []operation.Operation) string {
	if len(operations) != 1 {
		return ""
	}
	state, err := operation.DecodeCodeState(operations[0])
	if err != nil || state.Result == nil || len(state.Result.Calls) == 0 {
		return ""
	}
	latest := state.Result.Calls[len(state.Result.Calls)-1]
	return fmt.Sprintf("The code has made %d tool calls so far, the latest %s.", len(state.Result.Calls), latest.Name)
}

func (translator *translator) TranslateResult(callID string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	text := func(value string) llm.ToolResult {
		return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: value}}}
	}
	if status.Error != "" {
		if len(operations) != 0 {
			return llm.ToolResult{}, fmt.Errorf("Code call %q has both a validation error and operations", callID)
		}
		return text("Error: " + status.Error), nil
	}
	if len(operations) != 1 {
		return llm.ToolResult{}, fmt.Errorf("Code call %q has %d operations, want 1", callID, len(operations))
	}
	current := operations[0]
	state, err := operation.DecodeCodeState(current)
	if err != nil {
		return llm.ToolResult{}, fmt.Errorf("decode Code call %q result: %w", callID, err)
	}
	switch current.Status {
	case operation.StatusReady, operation.StatusAwaiting, operation.StatusCanceling:
		running := "The code is still running."
		if state.Result != nil && len(state.Result.Calls) != 0 {
			running += fmt.Sprintf(" It has made %d tool calls so far, the latest %s.", len(state.Result.Calls), state.Result.Calls[len(state.Result.Calls)-1].Name)
		}
		return text(running), nil
	case operation.StatusCompleted:
		if state.Result == nil {
			return llm.ToolResult{}, fmt.Errorf("Code call %q completed operation %q has no result", callID, current.ID)
		}
		output, _ := operation.BoundOutput(state.Result.Text(), current.MaxOutputLength)
		result := text(output)
		for _, image := range state.Result.Images() {
			result.Output = append(result.Output, llm.ToolResultOutput{Kind: llm.ToolResultImage, Value: image})
		}
		return result, nil
	case operation.StatusFailed, operation.StatusCanceled:
		message := state.TerminalError
		if message == "" {
			message = "code operation " + string(current.Status)
		}
		if state.Result != nil && len(state.Result.Calls) != 0 {
			partial := *state.Result
			partial.Error = message
			output, _ := operation.BoundOutput(partial.Text(), current.MaxOutputLength)
			return text(output), nil
		}
		return text("Error: " + message), nil
	}
	return llm.ToolResult{}, fmt.Errorf("Code call %q operation %q has invalid status %q", callID, current.ID, current.Status)
}
