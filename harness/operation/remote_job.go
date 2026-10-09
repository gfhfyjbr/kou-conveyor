package operation

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"time"
)

const (
	TypeRemoteJob    Type    = "remote_job"
	VersionRemoteJob Version = 2
)

type RemoteJobPlanType string

type RemoteJobPlanVersion uint32

type RemoteJobPlan struct {
	Type    RemoteJobPlanType
	Version RemoteJobPlanVersion
	Data    jsontext.Value
}

type RemoteJobHandler interface {
	RemoteJobPlanType() RemoteJobPlanType
	RemoteJobPlanVersion() RemoteJobPlanVersion
	AddRemoteJob(Operation) error
	CancelRemoteJob(ID, string) error
	RemoteJobUpdates() <-chan Operation
}

type RemoteJobState struct {
	Plan             RemoteJobPlan
	Handle           jsontext.Value `json:",omitzero"`
	NextInspectionAt time.Time      `json:",omitzero"`
	OutstandingInput jsontext.Value `json:",omitzero"`
	Subscription     jsontext.Value `json:",omitzero"`
	TerminalResult   string         `json:",omitzero"`
	TerminalError    string         `json:",omitzero"`
	ResultBytes      int            `json:",omitzero"`
	ResultTruncated  bool           `json:",omitzero"`
	ErrorBytes       int            `json:",omitzero"`
	ErrorTruncated   bool           `json:",omitzero"`
}

func NewRemoteJobSpec(plan RemoteJobPlan) (Spec, error) {
	if err := validateRemoteJobPlan(plan); err != nil {
		return Spec{}, err
	}
	encoded, err := json.Marshal(RemoteJobState{Plan: plan})
	if err != nil {
		return Spec{}, fmt.Errorf("encode remote job operation state: %w", err)
	}
	return Spec{
		MaxOutputLength: DefaultMaxOutputLength,
		Type:            TypeRemoteJob,
		Version:         VersionRemoteJob,
		State:           encoded,
	}, nil
}

func DecodeRemoteJobState(current Operation) (RemoteJobState, error) {
	if current.Type != TypeRemoteJob {
		return RemoteJobState{}, fmt.Errorf(
			"advance remote job operation %q: type %q: %w",
			current.ID,
			current.Type,
			ErrUnsupported,
		)
	}
	if current.Version != VersionRemoteJob {
		return RemoteJobState{}, fmt.Errorf(
			"advance remote job operation %q: version %d: %w",
			current.ID,
			current.Version,
			ErrUnsupported,
		)
	}

	if current.MaxOutputLength <= 0 {
		return RemoteJobState{}, errors.New("max output length must be positive")
	}
	if current.MaxOutputLength > MaxOutputLength {
		return RemoteJobState{}, fmt.Errorf("max output length must not exceed %d", MaxOutputLength)
	}

	var state RemoteJobState
	if err := json.Unmarshal(current.State, &state); err != nil {
		return RemoteJobState{}, fmt.Errorf(
			"decode remote job operation %q state: %w",
			current.ID,
			err,
		)
	}
	if err := validateRemoteJobPlan(state.Plan); err != nil {
		return RemoteJobState{}, fmt.Errorf(
			"validate remote job operation %q state: %w",
			current.ID,
			err,
		)
	}
	return state, nil
}

func UpdateRemoteJob(
	current Operation,
	state RemoteJobState,
	status Status,
	dispatches ...PrimitiveDispatch,
) (Step, error) {
	if _, err := DecodeRemoteJobState(current); err != nil {
		return Step{}, err
	}
	if err := validateRemoteJobPlan(state.Plan); err != nil {
		return Step{}, fmt.Errorf("validate remote job operation %q state: %w", current.ID, err)
	}
	current.Status = status
	prepareRemoteJobOutput(current, &state)
	encoded, err := json.Marshal(state)
	if err != nil {
		return Step{}, fmt.Errorf("encode remote job operation %q state: %w", current.ID, err)
	}
	current.State = encoded
	return Step{Operation: &current, Dispatches: dispatches}, nil
}

func FailRemoteJob(current Operation, err error) (Step, error) {
	if err == nil {
		return Step{}, errors.New("fail remote job operation: error must be set")
	}
	state, stateErr := DecodeRemoteJobState(current)
	if stateErr != nil {
		return Step{}, stateErr
	}
	state.TerminalError = err.Error()
	state.ErrorBytes, state.ErrorTruncated = 0, false
	state.TerminalResult = ""
	state.Subscription = nil
	return UpdateRemoteJob(current, state, StatusFailed)
}

func CancelRemoteJob(current Operation) (Step, error) {
	state, err := DecodeRemoteJobState(current)
	if err != nil {
		return Step{}, err
	}
	state.TerminalResult = ""
	state.Subscription = nil
	return UpdateRemoteJob(current, state, StatusCanceled)
}

func validateRemoteJobPlan(plan RemoteJobPlan) error {
	if plan.Type == "" {
		return errors.New("remote job plan type must be set")
	}
	if plan.Version == 0 {
		return errors.New("remote job plan version must be positive")
	}
	if len(plan.Data) == 0 || !plan.Data.IsValid() {
		return errors.New("remote job plan data must be valid JSON")
	}
	return nil
}
