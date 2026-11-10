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
)

type input struct {
	Action     string
	Name       string
	Bash       bash.Config
	Call       llm.ToolCall
	Status     tool.CallStatus
	Operations []operation.Operation
}
type output struct {
	Status  *tool.CallStatus
	Specs   []operation.Spec
	Result  *llm.ToolResult
	Running string
	Error   string
}
type recording struct{ specs []operation.Spec }

func (ctx *recording) Submit(spec operation.Spec) operation.ID {
	ctx.specs = append(ctx.specs, spec)
	return operation.ID(fmt.Sprintf("submitted-%d", len(ctx.specs)))
}
func run(value input) output {
	result := output{Specs: []operation.Spec{}}
	if value.Name != "Bash" {
		result.Error = fmt.Sprintf("unknown translator %q", value.Name)
		return result
	}
	translator := bash.New(value.Bash)
	switch value.Action {
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
