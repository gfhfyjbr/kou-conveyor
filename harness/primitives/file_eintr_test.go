//go:build unix

package primitives

import (
	"errors"
	"testing"

	"golang.org/x/sys/unix"
)

func TestRetryEINTR(t *testing.T) {
	resultErr := errors.New("result")
	tests := []struct {
		name string
		err  error
	}{
		{name: "success"},
		{name: "error", err: resultErr},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			attempts := 0
			err := retryEINTR(func() error {
				attempts++
				if attempts < 3 {
					return unix.EINTR
				}
				return test.err
			})
			if !errors.Is(err, test.err) {
				t.Fatalf("error = %v, want %v", err, test.err)
			}
			if attempts != 3 {
				t.Fatalf("attempts = %d, want 3", attempts)
			}
		})
	}
}
