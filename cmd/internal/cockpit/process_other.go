//go:build !darwin && !linux && !freebsd && !openbsd && !netbsd && !dragonfly

package cockpit

import (
	"fmt"
	"os/exec"
)

func configureProcess(cmd *exec.Cmd) {}

// LockSession only validates id on platforms without flock; the in-process
// guards of each front-end still apply.
func LockSession(dir, id string) (func(), error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	return func() {}, nil
}

// SessionBusy cannot see other processes' runs without flock.
func SessionBusy(dir, id string) bool { return false }
