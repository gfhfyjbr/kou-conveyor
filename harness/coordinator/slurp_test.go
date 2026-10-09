package coordinator

import (
	"context"
	"errors"
	"slices"
	"testing"
	"testing/synctest"
	"time"
)

func TestSlurpChannelPreservesOrder(t *testing.T) {
	for _, count := range []int{0, 3} {
		synctest.Test(t, func(t *testing.T) {
			output := make(chan int, count)
			var want []int
			for value := range count {
				output <- value
				want = append(want, value)
			}
			startedAt := time.Now()
			got, err := slurpChannel(t.Context(), output)
			if err != nil {
				t.Fatal(err)
			}
			if !slices.Equal(got, want) {
				t.Fatalf("slurped values = %v, want %v", got, want)
			}
			if elapsed := time.Since(startedAt); elapsed != slurpIdleTimeout {
				t.Fatalf("slurp duration = %v, want %v", elapsed, slurpIdleTimeout)
			}
		})
	}
}

func TestSlurpChannelResetsIdleTimeout(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		output := make(chan int, 1)
		output <- 1
		var values []int
		var err error
		done := make(chan struct{})
		go func() {
			values, err = slurpChannel(t.Context(), output)
			close(done)
		}()
		synctest.Wait()
		synctest.Sleep(slurpIdleTimeout / 2)
		output <- 2
		synctest.Wait()
		synctest.Sleep(slurpIdleTimeout - time.Nanosecond)
		select {
		case <-done:
			t.Fatal("slurp returned before a full idle interval after the last value")
		default:
		}
		synctest.Sleep(time.Nanosecond)
		select {
		case <-done:
		default:
			t.Fatal("slurp did not return after a full idle interval")
		}
		if err != nil || !slices.Equal(values, []int{1, 2}) {
			t.Fatalf("slurp result: values=%v error=%v", values, err)
		}
	})
}

func TestSlurpChannelCapsBatch(t *testing.T) {
	for _, count := range []int{99, 100, 101} {
		synctest.Test(t, func(t *testing.T) {
			output := make(chan int, count)
			want := make([]int, count)
			for value := range count {
				output <- value
				want[value] = value
			}
			startedAt := time.Now()
			got, err := slurpChannel(t.Context(), output)
			if err != nil {
				t.Fatal(err)
			}
			batchSize := min(count, 100)
			if !slices.Equal(got, want[:batchSize]) {
				t.Fatalf("slurped values = %v, want %v", got, want[:batchSize])
			}
			if len(output) != count-batchSize {
				t.Fatalf("remaining values = %d, want %d", len(output), count-batchSize)
			}
			for _, value := range want[batchSize:] {
				if got := <-output; got != value {
					t.Fatalf("remaining value = %d, want %d", got, value)
				}
			}
			var wantDuration time.Duration
			if count < 100 {
				wantDuration = slurpIdleTimeout
			}
			if elapsed := time.Since(startedAt); elapsed != wantDuration {
				t.Fatalf("slurp duration = %v, want %v", elapsed, wantDuration)
			}
		})
	}
}

func TestSlurpChannelReturnsAccumulatedValuesOnClosure(t *testing.T) {
	for _, buffered := range []bool{false, true} {
		synctest.Test(t, func(t *testing.T) {
			output := make(chan string, 2)
			var want []string
			if buffered {
				output <- "second"
				output <- "third"
				want = append(want, "second", "third")
			}
			close(output)
			startedAt := time.Now()
			got, err := slurpChannel(t.Context(), output)
			if err != nil || !slices.Equal(got, want) {
				t.Fatalf("slurp result = %v, %v; want %v, nil", got, err, want)
			}
			if elapsed := time.Since(startedAt); elapsed != 0 {
				t.Fatalf("closure advanced time by %v", elapsed)
			}
		})
	}
}

func TestSlurpChannelReturnsCancellationCause(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancelCause(t.Context())
		defer cancel(nil)
		cause := errors.New("session stopped")
		var err error
		go func() {
			_, err = slurpChannel(ctx, make(chan string))
		}()
		synctest.Wait()
		startedAt := time.Now()
		cancel(cause)
		synctest.Wait()
		if !errors.Is(err, cause) {
			t.Fatalf("slurp error = %v, want %v", err, cause)
		}
		if elapsed := time.Since(startedAt); elapsed != 0 {
			t.Fatalf("cancellation advanced time by %v", elapsed)
		}
	})
}
