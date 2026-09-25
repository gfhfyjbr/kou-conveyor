//go:build darwin

package primitives

import (
	"errors"
	"testing"

	"golang.org/x/sys/unix"
)

func TestFullSyncResult(t *testing.T) {
	flushErr := errors.New("flush failed")
	tests := []struct {
		name string
		err  error
		want error
	}{
		{name: "success"},
		{name: "unsupported", err: unix.ENOTSUP},
		{name: "failure", err: flushErr, want: flushErr},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if err := fullSyncResult(test.err); !errors.Is(err, test.want) {
				t.Fatalf("error = %v, want %v", err, test.want)
			}
		})
	}
}
