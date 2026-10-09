//go:build unix

package primitives

import (
	"errors"

	"golang.org/x/sys/unix"
)

func retryEINTR(operation func() error) error {
	for {
		err := operation()
		if !errors.Is(err, unix.EINTR) {
			return err
		}
	}
}
