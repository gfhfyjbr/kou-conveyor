package operation

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

// A code operation runs the code the model wrote in code mode (codevm) on a
// compute primitive, and keeps what the run did as it goes: every tool
// call the code makes shows in the state as it starts and as it ends, so
// the cockpits show the run's progress under the code.

const (
	TypeCode    Type    = "code"
	VersionCode Version = 1

	codeComputeCorrelation primitives.CorrelationID = "code"
)

// CodeState is the code a code operation runs, what it may do, and what it
// did.
type CodeState struct {
	Code   string
	Config codevm.Config
	// Image bounds the images viewImage() reads.
	Image         ViewImageConfig `json:",omitzero"`
	Result        *codevm.Result  `json:",omitzero"`
	TerminalError string          `json:",omitzero"`
}

func NewCodeSpec(state CodeState, maxOutputLength int) (Spec, error) {
	if err := validateCodeState(state); err != nil {
		return Spec{}, err
	}
	if maxOutputLength <= 0 {
		maxOutputLength = DefaultMaxOutputLength
	}
	if maxOutputLength > MaxOutputLength {
		return Spec{}, errors.New("max output length is out of range")
	}
	encoded, err := json.Marshal(state)
	if err != nil {
		return Spec{}, fmt.Errorf("encode code operation state: %w", err)
	}
	return Spec{MaxOutputLength: maxOutputLength, Type: TypeCode, Version: VersionCode, State: encoded}, nil
}

func validateCodeState(state CodeState) error {
	if strings.TrimSpace(state.Code) == "" {
		return errors.New("code must be set")
	}
	if state.Config.Shell == "" {
		return errors.New("the shell must be set")
	}
	return nil
}

func DecodeCodeState(current Operation) (CodeState, error) {
	if current.Type != TypeCode || current.Version != VersionCode {
		return CodeState{}, fmt.Errorf(
			"decode code operation %q: type %q version %d: %w",
			current.ID, current.Type, current.Version, ErrUnsupported,
		)
	}
	var state CodeState
	if err := json.Unmarshal(current.State, &state); err != nil {
		return CodeState{}, fmt.Errorf("decode code operation %q state: %w", current.ID, err)
	}
	if err := validateCodeState(state); err != nil {
		return CodeState{}, fmt.Errorf("validate code operation %q state: %w", current.ID, err)
	}
	return state, nil
}

// AdvanceCode runs a code operation.
func AdvanceCode(current Operation, event *primitives.PrimitiveEvent) (Step, error) {
	state, err := DecodeCodeState(current)
	if err != nil {
		return Step{}, err
	}
	switch current.Status {
	case StatusReady:
		if event != nil {
			return Step{}, fmt.Errorf("advance ready code operation %q: unexpected primitive event", current.ID)
		}
		if state.Result != nil || state.TerminalError != "" {
			return Step{}, fmt.Errorf("advance ready code operation %q: state is not initial", current.ID)
		}
		return dispatchCode(current, state)
	case StatusAwaiting:
		if event == nil {
			// Resumed after a restart: what the run did is unknown, so it
			// is not run again.
			return finishCode(current, state, StatusFailed, "the code run was interrupted by a restart before it finished; check what it did before running it again")
		}
		return codeEvent(current, state, *event)
	case StatusCanceling:
		if event != nil {
			return Step{}, fmt.Errorf("advance canceling code operation %q: unexpected primitive event", current.ID)
		}
		return finishCode(current, state, StatusCanceled, "code operation canceled")
	default:
		return Step{}, fmt.Errorf("advance code operation %q: terminal status %q", current.ID, current.Status)
	}
}

func dispatchCode(current Operation, state CodeState) (Step, error) {
	current.Status = StatusAwaiting
	step, err := codeStep(current, state)
	if err != nil {
		return Step{}, err
	}
	config := state.Config
	if config.MaxOutputLength <= 0 {
		config.MaxOutputLength = current.MaxOutputLength
	}
	image := state.Image
	config.ViewImage = func(ctx context.Context, path string) (codevm.ImageResult, error) {
		return viewImageForCode(ctx, path, image)
	}
	code := state.Code
	step.Dispatches = []PrimitiveDispatch{{
		Type: primitives.PrimitiveDispatchCompute,
		Data: primitives.ComputeRequest{
			Source: primitives.SourceID(current.ID), CorrelationID: codeComputeCorrelation,
			Progress: func(ctx context.Context, report func(any)) (any, error) {
				result := codevm.Run(ctx, config, code, func(progress codevm.Result) { report(progress) })
				return result, nil
			},
		},
	}}
	return step, nil
}

// viewImageForCode reads an image for viewImage(), as the ViewImage tool
// does.
func viewImageForCode(ctx context.Context, path string, config ViewImageConfig) (codevm.ImageResult, error) {
	if config.MaxSize <= 0 {
		config.MaxSize = 5_000_000 - 1_000
	}
	if config.MaxWidth <= 0 {
		config.MaxWidth = 2000
	}
	if config.MaxHeight <= 0 {
		config.MaxHeight = 2000
	}
	info, err := os.Stat(path)
	if err != nil {
		return codevm.ImageResult{}, err
	}
	if info.Size() > MaxViewImageSourceBytes {
		return codevm.ImageResult{}, fmt.Errorf("%s is %d bytes; an image may be %d at most", path, info.Size(), MaxViewImageSourceBytes)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return codevm.ImageResult{}, err
	}
	result, err := prepareViewImage(ctx, [][]byte{data}, config, nil)
	if err != nil {
		return codevm.ImageResult{}, err
	}
	description := fmt.Sprintf("%dx%d %s", result.OriginalWidth, result.OriginalHeight, result.OriginalMIMEType)
	if result.ScaleRatio < 1 {
		description += fmt.Sprintf(", scaled by %.2f", result.ScaleRatio)
	}
	return codevm.ImageResult{DataURL: "data:" + result.EncodedMIMEType + ";base64," + result.Content, Description: description}, nil
}

func codeEvent(current Operation, state CodeState, event primitives.PrimitiveEvent) (Step, error) {
	if event.Source != primitives.SourceID(current.ID) {
		return finishCode(current, state, StatusFailed, fmt.Sprintf("code primitive event source is %q, want %q", event.Source, current.ID))
	}
	switch event.Type {
	case primitives.PrimitiveEventCanceled:
		return finishCode(current, state, StatusCanceled, "code operation canceled")
	case primitives.PrimitiveEventFailed:
		failure, ok := event.Result.(primitives.PrimitiveFailureResult)
		message := "code run failed with an invalid result"
		if ok {
			message = failure.Error
		}
		return finishCode(current, state, StatusFailed, message)
	case primitives.PrimitiveEventComputeProgress:
		computed, ok := event.Result.(primitives.ComputeResult)
		if !ok {
			return Step{}, nil
		}
		result, ok := computed.Value.(codevm.Result)
		if !ok {
			return Step{}, nil
		}
		state.Result = &result
		return codeStep(current, state)
	case primitives.PrimitiveEventComputeCompleted:
		computed, ok := event.Result.(primitives.ComputeResult)
		if !ok {
			return finishCode(current, state, StatusFailed, "code run returned an invalid compute result")
		}
		result, ok := computed.Value.(codevm.Result)
		if !ok {
			return finishCode(current, state, StatusFailed, "code run returned an invalid result")
		}
		state.Result = &result
		current.Status = StatusCompleted
		return codeStep(current, state)
	default:
		return finishCode(current, state, StatusFailed, fmt.Sprintf("code run returned unexpected event %q", event.Type))
	}
}

func finishCode(current Operation, state CodeState, status Status, terminalError string) (Step, error) {
	state.TerminalError = terminalError
	current.Status = status
	return codeStep(current, state)
}

func codeStep(current Operation, state CodeState) (Step, error) {
	encoded, err := json.Marshal(state)
	if err != nil {
		return Step{}, fmt.Errorf("encode code operation %q state: %w", current.ID, err)
	}
	current.State = encoded
	return Step{Operation: &current}, nil
}

func failCode(current Operation, cause error) (Operation, bool) {
	state, err := DecodeCodeState(current)
	if err != nil {
		return Operation{}, false
	}
	step, err := finishCode(current, state, StatusFailed, cause.Error())
	if err != nil {
		return Operation{}, false
	}
	return *step.Operation, true
}
