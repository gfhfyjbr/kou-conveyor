// Development-only shell transitions. Returned dispatches are never executed.
// Go's timer reducer can signal a process directly, so PID-bearing fired-timer
// fixtures are rejected before Handle; this oracle never signals arbitrary PIDs.
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

func (input eventSpec) event() (primitives.PrimitiveEvent, error) {
	event := primitives.PrimitiveEvent{Type: input.Type, Source: input.Source, CorrelationID: input.CorrelationID}
	var target any
	switch input.ResultKind {
	case "created":
		target = new(primitives.IOCreateResult)
	case "started":
		target = new(primitives.ProcessStartedResult)
	case "exited":
		target = new(primitives.ProcessExitResult)
	case "read_output":
		target = new(primitives.IOReadOutputResult)
	case "read_completed":
		target = new(primitives.IOReadCompletedResult)
	case "failure":
		target = new(primitives.PrimitiveFailureResult)
	default:
		event.Result = input.Result
		return event, nil
	}
	if err := json.Unmarshal(input.Result, target); err != nil {
		return event, err
	}
	switch value := target.(type) {
	case *primitives.IOCreateResult:
		event.Result = *value
	case *primitives.ProcessStartedResult:
		event.Result = *value
	case *primitives.ProcessExitResult:
		event.Result = *value
	case *primitives.IOReadOutputResult:
		event.Result = *value
	case *primitives.IOReadCompletedResult:
		event.Result = *value
	case *primitives.PrimitiveFailureResult:
		event.Result = *value
	}
	return event, nil
}

type spec struct {
	Action          string
	Operation       operation.Operation
	Input           operation.ShellInput
	BaseDirectory   string
	MaxOutputLength int
	Event           *eventSpec
	Events          []*eventSpec
}
type output struct {
	Spec  *operation.Spec
	State *operation.ShellState
	Steps []operation.Step
	Error string
}

func run(input spec) output {
	result := output{Steps: []operation.Step{}}
	var err error
	switch input.Action {
	case "shell_spec":
		var value operation.Spec
		value, err = operation.NewShellSpec(input.Input, input.BaseDirectory, input.MaxOutputLength)
		if err == nil {
			result.Spec = &value
		}
	case "shell_decode":
		var state operation.ShellState
		state, err = operation.DecodeShellState(input.Operation)
		if err == nil {
			result.State = &state
		}
	case "shell_advance":
		var shell *operation.Shell
		shell, err = operation.NewShell(input.Operation)
		if err != nil {
			break
		}
		events := input.Events
		if events == nil {
			events = []*eventSpec{input.Event}
		}
		current := input.Operation
		for _, item := range events {
			var event *primitives.PrimitiveEvent
			if item != nil {
				if item.Type == primitives.PrimitiveEventTimerFired {
					state, decodeErr := operation.DecodeShellState(current)
					if decodeErr != nil {
						err = decodeErr
						break
					}
					if state.ProcessGroupID > 1 {
						err = fmt.Errorf("fixture rejected: fired timer with an unowned process group")
						break
					}
				}
				value, eventErr := item.event()
				if eventErr != nil {
					err = eventErr
					break
				}
				event = &value
			}
			var step operation.Step
			step, err = shell.Handle(event)
			if err != nil {
				break
			}
			result.Steps = append(result.Steps, step)
			if step.Operation != nil {
				current = *step.Operation
			}
		}
	default:
		err = fmt.Errorf("unknown action %q", input.Action)
	}
	if err != nil {
		result.Error = err.Error()
	}
	return result
}
func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 16<<20)
	for scanner.Scan() {
		var input spec
		if err := json.Unmarshal(scanner.Bytes(), &input); err != nil {
			panic(err)
		}
		bytes, err := json.Marshal(run(input))
		if err != nil {
			panic(err)
		}
		fmt.Println(string(bytes))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
