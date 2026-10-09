package primitives

import (
	"context"
	"errors"
	"fmt"
)

const (
	PrimitiveEventComputeCompleted PrimitiveEventType = "compute.completed"
	// PrimitiveEventComputeProgress carries what a computation reports on
	// its way, as a ComputeResult; the computation is not done.
	PrimitiveEventComputeProgress PrimitiveEventType = "compute.progress"
)

// Intended use: offload CPU-heavy, third-party blocking calls. Run does the
// work; Progress does it instead when set, reporting values on the way,
// each a compute.progress event.
type ComputeRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
	Run           func(context.Context) (any, error)
	Progress      func(ctx context.Context, report func(any)) (any, error)
}

type ComputeResult struct {
	Value any
}

func Compute(ctx context.Context, request ComputeRequest, events chan<- PrimitiveEvent) {
	go runCompute(ctx, request, events)
}

func runCompute(ctx context.Context, request ComputeRequest, events chan<- PrimitiveEvent) {
	if request.Run == nil && request.Progress == nil {
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
	if request.Progress != nil {
		value, err = request.Progress(ctx, func(progress any) {
			event := PrimitiveEvent{
				Type:          PrimitiveEventComputeProgress,
				Source:        request.Source,
				CorrelationID: request.CorrelationID,
				Result:        ComputeResult{Value: progress},
			}
			// A canceled computation's progress has no reader any more.
			select {
			case events <- event:
			case <-ctx.Done():
			}
		})
	} else {
		value, err = request.Run(ctx)
	}
	returned = true
}
