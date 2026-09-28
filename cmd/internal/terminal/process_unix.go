//go:build unix

package terminal

import (
	"errors"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"

	"golang.org/x/sys/unix"
)

// foreground is the process group in the foreground of the terminal: the
// program the user runs, or the shell.
func foreground(master *os.File) int {
	pgrp, err := unix.IoctlGetInt(int(master.Fd()), unix.TIOCGPGRP)
	if err != nil {
		return 0
	}
	return pgrp
}

// processDir is the working directory of a process.
func processDir(pid int) (string, error) {
	switch runtime.GOOS {
	case "linux":
		return os.Readlink("/proc/" + strconv.Itoa(pid) + "/cwd")
	case "darwin", "freebsd":
		out, err := exec.Command("lsof", "-a", "-p", strconv.Itoa(pid), "-d", "cwd", "-Fn").Output()
		if err != nil {
			return "", err
		}
		for _, line := range strings.Split(string(out), "\n") {
			if dir, ok := strings.CutPrefix(line, "n"); ok && strings.HasPrefix(dir, "/") {
				return dir, nil
			}
		}
		return "", errors.New("lsof names no directory")
	}
	return "", errors.ErrUnsupported
}

// hangUpGroup hangs up on the processes of the shell's session: it leads
// their group.
func hangUpGroup(pid int) { _ = unix.Kill(-pid, unix.SIGHUP) }

// killGroup kills them.
func killGroup(pid int) { _ = unix.Kill(-pid, unix.SIGKILL) }
