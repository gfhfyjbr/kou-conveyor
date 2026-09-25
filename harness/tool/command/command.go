// Package command runs tools as commands, as plugins declare them: the call's
// arguments arrive on the command's standard input as a JSON object, and
// what it prints is the result. A call is a shell operation, so it runs in
// the background, survives a restart of the runner and stops with it.
package command

import (
	"encoding/json/jsontext"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/bash"
)

// Shell is the shell that starts the commands. It is POSIX sh whatever the
// user's shell is, as the arguments arrive through a here-document.
const Shell = "/bin/sh"

// argumentsDelimiter ends the here-document. Compact JSON is one line and a
// line of JSON is never a bare word, so the arguments cannot end it early.
const argumentsDelimiter = "KOU_CONVEYOR_TOOL_ARGUMENTS"

var variableName = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)

type Config struct {
	// Tool is the definition the model sees.
	Tool llm.Tool
	// Command is the program and its arguments.
	Command []string
	// Directory is where the command runs: the workspace.
	Directory string
	// BaseDirectory keeps the output of the calls, as for Bash.
	BaseDirectory string
	// Environment adds variables, such as the plugin's directory. The
	// command also gets KOU_CONVEYOR_TOOL_NAME and KOU_CONVEYOR_TOOL_CALL_ID.
	Environment map[string]string
	// MaxOutputLength bounds each output stream; zero is the default.
	MaxOutputLength int
}

type translator struct {
	config Config
}

// New returns the translator of a command tool.
func New(config Config) (tool.Translator, error) {
	if len(config.Command) == 0 || config.Command[0] == "" {
		return nil, fmt.Errorf("tool %q has no command", config.Tool.Name)
	}
	for name := range config.Environment {
		if !variableName.MatchString(name) {
			return nil, fmt.Errorf("tool %q: %q is not a variable name", config.Tool.Name, name)
		}
	}
	if config.MaxOutputLength == 0 {
		config.MaxOutputLength = operation.DefaultMaxOutputLength
	}
	return &translator{config: config}, nil
}

func (translator *translator) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	arguments := strings.TrimSpace(call.Arguments)
	if arguments == "" {
		arguments = "{}"
	}
	value := jsontext.Value(arguments)
	if value.Kind() != '{' || !value.IsValid() {
		return tool.ErrorStatus(fmt.Sprintf("%s arguments must be a JSON object", translator.config.Tool.Name), translator.config.MaxOutputLength)
	}
	value = value.Clone()
	if err := value.Compact(); err != nil {
		return tool.ErrorStatus(fmt.Sprintf("encode %s arguments: %v", translator.config.Tool.Name, err), translator.config.MaxOutputLength)
	}
	spec, err := operation.NewShellSpec(operation.ShellInput{
		Command:   translator.script(call, string(value)),
		Shell:     Shell,
		Directory: translator.config.Directory,
	}, translator.config.BaseDirectory, translator.config.MaxOutputLength)
	if err != nil {
		return tool.ErrorStatus(fmt.Sprintf("build %s operation: %v", translator.config.Tool.Name, err), translator.config.MaxOutputLength)
	}
	return tool.CallStatus{WaitingFor: []operation.ID{ctx.Submit(spec)}}
}

// script is the shell script that runs the command with the arguments on
// its standard input.
func (translator *translator) script(call llm.ToolCall, arguments string) string {
	environment := maps.Clone(translator.config.Environment)
	if environment == nil {
		environment = map[string]string{}
	}
	environment["KOU_CONVEYOR_TOOL_NAME"] = translator.config.Tool.Name
	environment["KOU_CONVEYOR_TOOL_CALL_ID"] = call.CallID
	var script strings.Builder
	script.WriteString("export")
	for _, name := range slices.Sorted(maps.Keys(environment)) {
		fmt.Fprintf(&script, " %s=%s", name, quote(environment[name]))
	}
	script.WriteString("\nexec")
	for _, argument := range translator.config.Command {
		script.WriteString(" " + quote(argument))
	}
	fmt.Fprintf(&script, " <<'%s'\n%s\n%s\n", argumentsDelimiter, arguments, argumentsDelimiter)
	return script.String()
}

// quote makes a word the shell reads literally.
func quote(word string) string {
	return "'" + strings.ReplaceAll(word, "'", `'\''`) + "'"
}

func (translator *translator) TranslateResult(
	callID string,
	status tool.CallStatus,
	operations []operation.Operation,
) (llm.ToolResult, error) {
	text := func(value string) llm.ToolResult {
		return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: value}}}
	}
	if status.Error != "" {
		if len(operations) != 0 {
			return llm.ToolResult{}, fmt.Errorf("%s call %q has both a validation error and operations", translator.config.Tool.Name, callID)
		}
		return text("Error: " + status.Error), nil
	}
	if len(operations) != 1 {
		return llm.ToolResult{}, fmt.Errorf("%s call %q has %d operations, want 1", translator.config.Tool.Name, callID, len(operations))
	}
	output, err := bash.FormatResult(callID, operations[0])
	if err != nil {
		return llm.ToolResult{}, err
	}
	return text(output), nil
}
