package primitives

import (
	"context"
	"errors"
	"time"
)

const (
	PrimitiveEventTimerFired PrimitiveEventType = "timer.fired"
	timerMaximumSleep                           = 5 * time.Second
)

type TimerRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
	Deadline      time.Time
}

type TimerResult struct {
	Deadline time.Time
	FiredAt  time.Time
}

func ScheduleTimer(ctx context.Context, request TimerRequest, events chan<- PrimitiveEvent) {
	go runTimer(ctx, request, events)
}

func runTimer(ctx context.Context, request TimerRequest, events chan<- PrimitiveEvent) {
	if err := validateTimerRequest(request); err != nil {
		events <- timerFailure(request, err)
		return
	}
	if ctx.Err() != nil {
		events <- timerCanceled(request)
		return
	}

	deadline := request.Deadline.Round(0)
	timer := time.NewTimer(timerDelay(time.Now(), deadline))
	defer timer.Stop()

	for {
		select {
		case <-timer.C:
			now := time.Now().Round(0)
			if !now.Before(deadline) {
				events <- timerFired(request, now)
				return
			}
			timer.Reset(timerDelay(now, deadline))
		case <-ctx.Done():
			events <- timerCanceled(request)
			return
		}
	}
}

// Strip monotonic readings so every bounded wait rechecks the absolute wall-clock deadline.
func timerDelay(now time.Time, deadline time.Time) time.Duration {
	remaining := deadline.Round(0).Sub(now.Round(0))
	return min(max(remaining, 0), timerMaximumSleep)
}

func validateTimerRequest(request TimerRequest) error {
	if request.Deadline.IsZero() {
		return errors.New("schedule timer: deadline must be set")
	}
	return nil
}

func timerFired(request TimerRequest, firedAt time.Time) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventTimerFired,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: TimerResult{
			Deadline: request.Deadline,
			FiredAt:  firedAt,
		},
	}
}

func timerCanceled(request TimerRequest) PrimitiveEvent {
	return primitiveCanceled(request.Source, request.CorrelationID)
}

func timerFailure(request TimerRequest, err error) PrimitiveEvent {
	return primitiveFailure(request.Source, request.CorrelationID, err)
}
