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
	file, err := lockSessionFile(dir, id)
	if err != nil {
		return nil, err
	}
	return func() { file.Close() }, nil
}

// lockSessionFile takes the session's lock, as LockSession does, and returns
// the file that holds it. The lock is the open file's: a process the file is
// handed to holds it too, until the last of them closes it. Never unlock it
// with flock: that would release it for all of them.
func lockSessionFile(dir, id string) (*os.File, error) {
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
			return file, nil
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

// canDetach says runners can run apart from their front-ends here: the
// session's lock goes with the runner, and Adopt sees it released.
const canDetach = true

// detachProcess starts the runner in a session of its own, which neither a
// hangup of the front-end's terminal nor a signal to the front-end's process
// group reaches. The runner leads its own process group.
func detachProcess(cmd *exec.Cmd) { cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true} }

// lockFollow takes the lock of a detached run's follower, which the Job that
// follows the run holds until it ends; ErrFollowed when another holds it.
// The file is closed on exec, so a front-end that execs itself lets go.
func lockFollow(path string) (*os.File, error) {
	file, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		file.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) {
			return nil, ErrFollowed
		}
		return nil, err
	}
	return file, nil
}

// interruptGroup interrupts a detached runner, whose process group it leads.
func interruptGroup(pid int) error { return syscall.Kill(-pid, syscall.SIGINT) }

// killGroup kills a detached runner's process group outright.
func killGroup(pid int) error { return syscall.Kill(-pid, syscall.SIGKILL) }

// detachedGone reports whether the runner pid, a detached run of the session,
// has exited. A runner this process started before it exec'd itself is its
// child: it is reaped here, and status says how it ended. Another process's
// runner, or one this process waits for itself (waited), is seen to go when
// the session's lock it held is released; known is then false.
func detachedGone(pid int, sessionDir, sessionID string) (gone bool, status error, known bool) {
	if _, waits := waited.Load(pid); !waits {
		var wait syscall.WaitStatus
		switch reaped, err := syscall.Wait4(pid, &wait, syscall.WNOHANG, nil); {
		case err == nil && reaped == pid:
			switch {
			case wait.Exited():
				return true, exitStatus(wait.ExitStatus()), true
			case wait.Signaled():
				return true, fmt.Errorf("signal: %v", wait.Signal()), true
			}
			return true, nil, false
		case err == nil, errors.Is(err, syscall.EINTR):
			return false, nil, false // a child of this process that runs
		}
	}
	if !SessionBusy(sessionDir, sessionID) {
		return true, nil, false
	}
	// The lock is taken, by the runner unless it is gone and another run
	// took the session since.
	if err := syscall.Kill(pid, 0); errors.Is(err, syscall.ESRCH) {
		return true, nil, false
	}
	return false, nil, false
}
