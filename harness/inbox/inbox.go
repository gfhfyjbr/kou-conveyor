// Package inbox owns input deduplication for one session.
package inbox

import (
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

type ID string

type InputKind string

const (
	InputExternal InputKind = "external"
	InputControl  InputKind = "control"
	InputCrash    InputKind = "crash"
)

type Input struct {
	ID      ID
	Kind    InputKind
	Payload jsontext.Value `json:",omitzero"`
	// Delivery says when external input reaches the model while the agent
	// works; the zero value delivers it at once.
	Delivery Delivery `json:",omitzero"`
}

// Delivery says when external input that arrives while the agent works
// reaches the model.
type Delivery string

const (
	// DeliverAtOnce starts a turn with the input right away, cutting short
	// a response the model is writing.
	DeliverAtOnce Delivery = ""
	// DeliverAfterTools lets the model finish the response it is writing and
	// the tool calls of its latest response finish, and sends the input with
	// their results, or with any request that goes out before. The
	// coordinator records the input only when it goes out, so the session
	// holds it where the model read it: after the results.
	DeliverAfterTools Delivery = "after_tools"
)

func (input Input) Validate() error {
	if input.ID == "" {
		return fmt.Errorf("input ID is empty")
	}
	switch input.Kind {
	case InputExternal, InputCrash:
	case InputControl:
		if _, err := input.DecodeControlMessage(); err != nil {
			return err
		}
	default:
		return fmt.Errorf("input %q has unsupported kind %q", input.ID, input.Kind)
	}
	if input.Payload != nil && !input.Payload.IsValid() {
		return fmt.Errorf("input %q payload is not valid JSON", input.ID)
	}
	switch input.Delivery {
	case DeliverAtOnce:
	case DeliverAfterTools:
		if input.Kind != InputExternal {
			return fmt.Errorf("input %q of kind %q cannot wait for tools", input.ID, input.Kind)
		}
	default:
		return fmt.Errorf("input %q has unsupported delivery %q", input.ID, input.Delivery)
	}
	return nil
}

type ControlMode string

const (
	StopHard       ControlMode = "hard"
	StopWhenIdle   ControlMode = "when_idle"
	Heartbeat      ControlMode = "heartbeat"
	UpdateSettings ControlMode = "settings"
	// Compact asks for the conversation to be summarized before the next
	// turn, freeing the context it takes. Reason optionally tells the summary
	// what to focus on.
	Compact ControlMode = "compact"
)

type Settings struct {
	ReasoningEffort llm.ReasoningEffort `json:",omitzero"`
}

type ControlMessage struct {
	Mode       ControlMode
	Reason     string
	Parameters any `json:",omitzero"`
}

func (input Input) DecodeControlMessage() (ControlMessage, error) {
	if input.Kind != InputControl {
		return ControlMessage{}, fmt.Errorf("control message input has kind %q", input.Kind)
	}
	var envelope struct {
		Mode       ControlMode
		Reason     string
		Parameters jsontext.Value
	}
	if err := json.Unmarshal(input.Payload, &envelope, json.RejectUnknownMembers(true)); err != nil {
		return ControlMessage{}, fmt.Errorf("decode control message: %w", err)
	}
	request := ControlMessage{Mode: envelope.Mode, Reason: envelope.Reason}
	if request.Mode != UpdateSettings && len(envelope.Parameters) != 0 {
		return ControlMessage{}, fmt.Errorf("control mode %q does not accept parameters", request.Mode)
	}
	switch request.Mode {
	case StopHard, StopWhenIdle, Compact:
	case Heartbeat:
		if request.Reason == "" {
			return ControlMessage{}, fmt.Errorf("heartbeat reason is empty")
		}
	case UpdateSettings:
		var settings Settings
		if err := json.Unmarshal(envelope.Parameters, &settings, json.RejectUnknownMembers(true)); err != nil {
			return ControlMessage{}, fmt.Errorf("decode settings parameters: %w", err)
		}
		if !settings.ReasoningEffort.Valid() {
			return ControlMessage{}, fmt.Errorf("unsupported reasoning effort %q", settings.ReasoningEffort)
		}
		request.Parameters = settings
	default:
		return ControlMessage{}, fmt.Errorf("unsupported control mode %q", request.Mode)
	}
	return request, nil
}

type Writer interface {
	Submit(context.Context, Input) error
}
