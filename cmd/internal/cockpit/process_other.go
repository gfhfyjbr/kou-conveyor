//go:build !darwin && !linux && !freebsd && !openbsd && !netbsd && !dragonfly

package cockpit

import (
	"errors"
	"fmt"
	"os"
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

// lockSessionFile validates id; there is no lock to hand on.
func lockSessionFile(dir, id string) (*os.File, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	return nil, nil
}

// canDetach is false without flock: Adopt could not tell a runner that went
// from one that runs, so runners stay attached to their front-ends.
const canDetach = false

func detachProcess(cmd *exec.Cmd) {}

// lockFollow opens the follower's lock file; there is no lock to take.
func lockFollow(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o600)
}

func interruptGroup(pid int) error { return errors.ErrUnsupported }

func killGroup(pid int) error { return errors.ErrUnsupported }

func detachedGone(pid int, sessionDir, sessionID string) (gone bool, status error, known bool) {
	return true, nil, false
}
