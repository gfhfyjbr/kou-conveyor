// Pure model-call/result translation. Submitted specs are recorded, never run.
package main

import (
	"bufio"
	"encoding/json/v2"
	"fmt"
	"os"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/bash"
	toolcommand "github.com/gfhfyjbr/kou-conveyor/harness/tool/command"
	filetool "github.com/gfhfyjbr/kou-conveyor/harness/tool/file"
)

type input struct {
	Action     string
	Name       string
	Kind       filetool.Kind
	Bash       bash.Config
	File       filetool.Config
	Command    commandConfig
	Call       llm.ToolCall
	Status     tool.CallStatus
	Operations []operation.Operation
	Steps      []step
}
type commandConfig struct {
	Name            string
	Command         []string
	Directory       string
	BaseDirectory   string
	Environment     map[string]string
	MaxOutputLength int
	Waits           bool
}

type step struct {
	CallID     string
	Status     tool.CallStatus
	Operations []operation.Operation
}
type output struct {
	Status   *tool.CallStatus
	Specs    []operation.Spec
	Result   *llm.ToolResult
	Running  string
	Error    string
	Sequence []output
	Waits    bool
}

func blankOutput() output {
	return output{Specs: []operation.Spec{}, Sequence: []output{}}
}

type recording struct{ specs []operation.Spec }

func (ctx *recording) Submit(spec operation.Spec) operation.ID {
	ctx.specs = append(ctx.specs, spec)
	return operation.ID(fmt.Sprintf("submitted-%d", len(ctx.specs)))
}
func run(value input) output {
	result := blankOutput()
	var translator tool.Translator
	if value.Kind == "command" {
		var err error
		translator, err = toolcommand.New(toolcommand.Config{
			Tool: llm.Tool{Name: value.Command.Name}, Command: value.Command.Command,
			Directory: value.Command.Directory, BaseDirectory: value.Command.BaseDirectory,
			Environment: value.Command.Environment, MaxOutputLength: value.Command.MaxOutputLength,
			Waits: value.Command.Waits,
		})
		if err != nil {
			result.Error = err.Error()
			return result
		}
	} else if value.Name == "Bash" {
		translator = bash.New(value.Bash)
	} else {
		translator = filetool.New(value.Kind, value.Name, value.File)
	}
	result.Waits = tool.Waits(translator)
	switch value.Action {
	case "new":
	case "translate":
		ctx := recording{specs: []operation.Spec{}}
		status := translator.Translate(&ctx, value.Call)
		result.Status, result.Specs = &status, ctx.specs
	case "result":
		translated, err := translator.TranslateResult(value.Call.CallID, value.Status, value.Operations)
		if err != nil {
			result.Error = err.Error()
		} else {
			result.Result = &translated
		}
	case "sequence":
		for _, step := range value.Steps {
			part := blankOutput()
			part.Waits = result.Waits
			translated, err := translator.TranslateResult(step.CallID, step.Status, step.Operations)
			if err != nil {
				part.Error = err.Error()
			} else {
				part.Result = &translated
			}
			result.Sequence = append(result.Sequence, part)
		}
	case "running":
		if describer, ok := translator.(tool.RunningDescriber); ok {
			result.Running = describer.DescribeRunning(value.Operations)
		}
	default:
		result.Error = fmt.Sprintf("unknown oracle action %q", value.Action)
	}
	return result
}
func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 8<<20)
	output := bufio.NewWriter(os.Stdout)
	defer output.Flush()
	for scanner.Scan() {
		var value input
		if err := json.Unmarshal(scanner.Bytes(), &value); err != nil {
			panic(err)
		}
		encoded, err := json.Marshal(run(value))
		if err != nil {
			panic(err)
		}
		fmt.Fprintln(output, string(encoded))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
