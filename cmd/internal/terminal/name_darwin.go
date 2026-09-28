package terminal

import "golang.org/x/sys/unix"

// processName is the name a process runs as.
func processName(pid int) string {
	info, err := unix.SysctlKinfoProc("kern.proc.pid", pid)
	if err != nil {
		return ""
	}
	return unix.ByteSliceToString(info.Proc.P_comm[:])
}
