//go:build !darwin || !cgo

package fswatch

import (
	"errors"
	"fmt"
	"runtime"
)

func watch(roots []string, changes chan struct{}) (func(), error) {
	return nil, fmt.Errorf("fswatch: no file events on %s without cgo: %w", runtime.GOOS, errors.ErrUnsupported)
}
