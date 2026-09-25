package primitives

import (
	"testing"
	"time"
)

func TestTimerDelayIsBoundedByMaximumSleep(t *testing.T) {
	now := time.Date(2026, time.August, 3, 12, 0, 0, 0, time.UTC)
	tests := []struct {
		name     string
		deadline time.Time
		want     time.Duration
	}{
		{name: "distant", deadline: now.Add(time.Hour), want: timerMaximumSleep},
		{name: "near", deadline: now.Add(2 * time.Second), want: 2 * time.Second},
		{name: "now", deadline: now, want: 0},
		{name: "past", deadline: now.Add(-time.Hour), want: 0},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := timerDelay(now, test.deadline); got != test.want {
				t.Fatalf("delay = %s, want %s", got, test.want)
			}
		})
	}
}

func TestTimerDelayUsesWallClockReadings(t *testing.T) {
	now := time.Now()
	deadline := now.Add(time.Hour).Round(0)

	if got := timerDelay(now, deadline); got != timerMaximumSleep {
		t.Fatalf("delay = %s, want %s", got, timerMaximumSleep)
	}
}
