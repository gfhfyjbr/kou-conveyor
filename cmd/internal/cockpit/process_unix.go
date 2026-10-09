//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cockpit

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"time"
)

func configureProcess(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	// Interrupt the runner's process group; the runner stops its tools, which
	// run in groups of their own, before it exits. WaitDelay bounds that.
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return os.ErrProcessDone
		}
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGINT)
	}
}

// LockSession takes an advisory lock that keeps other front-ends, in this or
// another process, from running the same session. The kernel releases it if
// the process dies; otherwise call the returned function.
func LockSession(dir, id string) (func(), error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	locks := filepath.Join(dir, ".locks")
	if err := os.MkdirAll(locks, 0o700); err != nil {
		return nil, fmt.Errorf("lock session: %w", err)
	}
	file, err := os.OpenFile(lockPath(dir, id), os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return nil, fmt.Errorf("lock session: %w", err)
	}
	for attempt := 1; ; attempt++ {
		err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return func() { file.Close() }, nil
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) {
			file.Close()
			return nil, fmt.Errorf("lock session: %w", err)
		}
		// SessionBusy holds the lock for an instant; only a lock that stays
		// taken belongs to a run.
		if attempt == 5 {
			file.Close()
			return nil, ErrSessionBusy
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// SessionBusy reports whether a run, in this or another process, holds the
// session right now.
func SessionBusy(dir, id string) bool {
	if !ValidSessionID(id) {
		return false
	}
	file, err := os.OpenFile(lockPath(dir, id), os.O_RDWR, 0)
	if err != nil {
		return false // never locked
	}
	defer file.Close()
	return errors.Is(syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB), syscall.EWOULDBLOCK)
}

func lockPath(dir, id string) string { return filepath.Join(dir, ".locks", id+".lock") }
