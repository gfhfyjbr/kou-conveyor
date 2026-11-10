// Development-only pure operation transitions. Compute dispatches are inspected,
// never executed, so file fixtures cannot read or modify the filesystem.
package main

import (
	"bufio"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"os"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

type eventSpec struct {
	Type          primitives.PrimitiveEventType
	Source        primitives.SourceID
	CorrelationID primitives.CorrelationID
	ResultKind    string
	Result        jsontext.Value
}

type spec struct {
	Action    string
	Operation operation.Operation
	File      operation.FileState
	Path      string
	Value     jsontext.Value
	Event     *eventSpec
}

type dispatch struct {
	Type          primitives.PrimitiveDispatchType
	Source        primitives.SourceID
	CorrelationID primitives.CorrelationID
	Path          string
	Offset        int64
	Count         int64
}

type output struct {
	Value      any
	Operation  *operation.Operation
	Dispatches []dispatch
	Error      string
}

func (input *eventSpec) event() (*primitives.PrimitiveEvent, error) {
	if input == nil {
		return nil, nil
	}
	var result any
	switch input.ResultKind {
	case "file":
		var value operation.FileResult
		if err := json.Unmarshal(input.Result, &value); err != nil {
			return nil, err
		}
		result = primitives.ComputeResult{Value: value}
	case "compute":
		result = primitives.ComputeResult{Value: input.Result}
	case "read_output":
		var value primitives.IOReadOutputResult
		if err := json.Unmarshal(input.Result, &value); err != nil {
			return nil, err
		}
		result = value
	case "read_completed":
		var value primitives.IOReadCompletedResult
		if err := json.Unmarshal(input.Result, &value); err != nil {
			return nil, err
		}
		result = value
	case "failure":
		var value primitives.PrimitiveFailureResult
		if err := json.Unmarshal(input.Result, &value); err != nil {
			return nil, err
		}
		result = value
	case "", "raw":
		if input.Result != nil {
			result = input.Result
		}
	default:
		return nil, fmt.Errorf("unknown oracle result kind %q", input.ResultKind)
	}
	return &primitives.PrimitiveEvent{
		Type: input.Type, Source: input.Source, CorrelationID: input.CorrelationID, Result: result,
	}, nil
}

func run(input spec) output {
	result := output{Dispatches: []dispatch{}}
	var step operation.Step
	var err error
	switch input.Action {
	case "value_spec":
		result.Value, err = operation.NewValueSpec(input.Value)
	case "value_decode":
		result.Value, err = operation.DecodeValue(input.Operation)
	case "file_spec":
		result.Value, err = operation.NewFileSpec(input.File)
	case "file_decode":
		result.Value, err = operation.DecodeFileState(input.Operation)
	case "skill_spec":
		result.Value, err = operation.NewSkillUseSpec(input.Path)
	case "skill_decode":
		result.Value, err = operation.DecodeSkillUse(input.Operation)
	case "value_advance", "file_advance", "skill_advance":
		var event *primitives.PrimitiveEvent
		event, err = input.Event.event()
		if err != nil {
			break
		}
		switch input.Action {
		case "value_advance":
			step, err = operation.AdvanceValue(input.Operation, event)
		case "file_advance":
			step, err = operation.AdvanceFile(input.Operation, event)
		case "skill_advance":
			step, err = operation.AdvanceSkillUse(input.Operation, event)
		}
		result.Operation = step.Operation
		for _, value := range step.Dispatches {
			projected := dispatch{Type: value.Type}
			switch request := value.Data.(type) {
			case primitives.ComputeRequest:
				projected.Source, projected.CorrelationID = request.Source, request.CorrelationID
			case primitives.IOReadRequest:
				projected.Source, projected.CorrelationID = request.Source, request.CorrelationID
				projected.Path, projected.Offset, projected.Count = request.Path, request.Offset, request.Count
			default:
				panic(fmt.Sprintf("unexpected dispatch %s: %T", value.Type, value.Data))
			}
			result.Dispatches = append(result.Dispatches, projected)
		}
	default:
		err = fmt.Errorf("unknown oracle action %q", input.Action)
	}
	if err != nil {
		result.Error = err.Error()
		result.Value = nil
	}
	return result
}

func main() {
	input := bufio.NewScanner(os.Stdin)
	input.Buffer(make([]byte, 64<<10), 8<<20)
	output := bufio.NewWriter(os.Stdout)
	defer output.Flush()
	for input.Scan() {
		var value spec
		if err := json.Unmarshal(input.Bytes(), &value); err != nil {
			panic(err)
		}
		encoded, err := json.Marshal(run(value))
		if err != nil {
			panic(err)
		}
		fmt.Fprintln(output, string(encoded))
	}
	if err := input.Err(); err != nil {
		panic(err)
	}
}
