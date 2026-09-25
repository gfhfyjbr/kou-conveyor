package primitives

import (
	"context"
	"errors"
	"fmt"
)

const PrimitiveEventComputeCompleted PrimitiveEventType = "compute.completed"

// Intended use: offload CPU-heavy, third-party blocking calls
type ComputeRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
	Run           func(context.Context) (any, error)
}

type ComputeResult struct {
	Value any
}

func Compute(ctx context.Context, request ComputeRequest, events chan<- PrimitiveEvent) {
	go runCompute(ctx, request, events)
}

func runCompute(ctx context.Context, request ComputeRequest, events chan<- PrimitiveEvent) {
	if request.Run == nil {
		events <- primitiveFailure(request.Source, request.CorrelationID, errors.New("compute: Run must be set"))
		return
	}
	if ctx.Err() != nil {
		events <- primitiveCanceled(request.Source, request.CorrelationID)
		return
	}

	var value any
	var err error
	returned := false
	// Defer event delivery because runtime.Goexit runs defers without returning to the caller.
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("compute callback panicked: %v", recovered)
		} else if !returned {
			err = errors.New("compute callback exited without returning")
		}
		if ctx.Err() != nil {
			events <- primitiveCanceled(request.Source, request.CorrelationID)
			return
		}
		if err != nil {
			events <- primitiveFailure(request.Source, request.CorrelationID, err)
			return
		}
		events <- PrimitiveEvent{
			Type:          PrimitiveEventComputeCompleted,
			Source:        request.Source,
			CorrelationID: request.CorrelationID,
			Result:        ComputeResult{Value: value},
		}
	}()
	value, err = request.Run(ctx)
	returned = true
}
