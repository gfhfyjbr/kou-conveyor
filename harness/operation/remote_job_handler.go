package operation

import (
	"bytes"
	"context"
	"fmt"
	"sync"
)

type remoteJobHandlerUpdate struct {
	handlerIndex int
	operation    Operation
	closed       bool
}

type remoteJobHandlers struct {
	handlers []RemoteJobHandler
	closed   []bool
	updates  chan remoteJobHandlerUpdate
	workers  sync.WaitGroup
}

func newRemoteJobHandlers(ctx context.Context, handlers []RemoteJobHandler) *remoteJobHandlers {
	router := &remoteJobHandlers{
		handlers: append([]RemoteJobHandler(nil), handlers...),
		closed:   make([]bool, len(handlers)),
		updates:  make(chan remoteJobHandlerUpdate),
	}
	for index, handler := range router.handlers {
		if handler != nil {
			router.workers.Go(func() { router.forward(ctx, index, handler) })
		}
	}
	return router
}

func (router *remoteJobHandlers) add(current Operation) (int, error) {
	state, err := DecodeRemoteJobState(current)
	if err != nil {
		return -1, err
	}
	matched := -1
	for index, handler := range router.handlers {
		if handler == nil || handler.RemoteJobPlanType() != state.Plan.Type ||
			handler.RemoteJobPlanVersion() != state.Plan.Version {
			continue
		}
		if matched != -1 {
			return -1, fmt.Errorf(
				"local operation manager has multiple handlers for remote job plan %q version %d",
				state.Plan.Type,
				state.Plan.Version,
			)
		}
		matched = index
	}
	if matched == -1 {
		return -1, fmt.Errorf(
			"local operation manager does not support remote job plan %q version %d: %w",
			state.Plan.Type,
			state.Plan.Version,
			ErrUnsupported,
		)
	}
	if router.closed[matched] {
		return -1, fmt.Errorf("remote job handler %d stopped", matched)
	}
	if err := router.handlers[matched].AddRemoteJob(current); err != nil {
		return -1, err
	}
	return matched, nil
}

func (router *remoteJobHandlers) markClosed(index int) {
	router.closed[index] = true
}

func (router *remoteJobHandlers) cancel(index int, id ID, reason string) error {
	return router.handlers[index].CancelRemoteJob(id, reason)
}

func (router *remoteJobHandlers) forward(ctx context.Context, index int, handler RemoteJobHandler) {
	for {
		select {
		case operation, open := <-handler.RemoteJobUpdates():
			update := remoteJobHandlerUpdate{handlerIndex: index, operation: operation, closed: !open}
			select {
			case router.updates <- update:
			case <-ctx.Done():
				return
			}
			if !open {
				return
			}
		case <-ctx.Done():
			return
		}
	}
}

func validateRemoteJobUpdate(current Operation, updated Operation) error {
	if updated.MaxOutputLength != current.MaxOutputLength {
		return fmt.Errorf("remote job handler changed output limit for operation %q", current.ID)
	}
	if updated.Type != current.Type || updated.Version != current.Version {
		return fmt.Errorf(
			"remote job handler changed operation %q type or version from %q version %d to %q version %d",
			current.ID,
			current.Type,
			current.Version,
			updated.Type,
			updated.Version,
		)
	}
	currentState, err := DecodeRemoteJobState(current)
	if err != nil {
		return fmt.Errorf("validate current remote job: %w", err)
	}
	updatedState, err := DecodeRemoteJobState(updated)
	if err != nil {
		return fmt.Errorf("validate remote job handler update: %w", err)
	}
	if updatedState.Plan.Type != currentState.Plan.Type ||
		updatedState.Plan.Version != currentState.Plan.Version ||
		!bytes.Equal(updatedState.Plan.Data, currentState.Plan.Data) {
		return fmt.Errorf("remote job handler changed plan for operation %q", current.ID)
	}
	if !validRemoteJobStatusTransition(current.Status, updated.Status) {
		return fmt.Errorf(
			"remote job handler produced invalid status transition for operation %q from %q to %q",
			current.ID,
			current.Status,
			updated.Status,
		)
	}
	return nil
}

func validRemoteJobStatusTransition(current Status, updated Status) bool {
	switch current {
	case StatusReady:
		return updated == StatusAwaiting || updated == StatusCanceling ||
			updated == StatusCompleted || updated == StatusFailed || updated == StatusCanceled
	case StatusAwaiting:
		return updated == StatusAwaiting || updated == StatusCanceling ||
			updated == StatusCompleted || updated == StatusFailed || updated == StatusCanceled
	case StatusCanceling:
		return updated == StatusCanceling || updated == StatusFailed || updated == StatusCanceled
	default:
		return false
	}
}
