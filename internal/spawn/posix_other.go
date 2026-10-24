//go:build !darwin

package spawn

import (
	"errors"
	"syscall"
)

// spawnable is false: elsewhere os/exec starts the process. Its fork
// (vfork on Linux) does not keep the parent's memory as macOS's does.
func spawnable(*syscall.SysProcAttr) bool { return false }

func posixSpawn(string, []string, []string, string, []int, *syscall.SysProcAttr) (int, error) {
	return 0, errors.ErrUnsupported
}
