package terminal

import (
	"bytes"
	"encoding/binary"

	"golang.org/x/sys/unix"
)

// processName is the name a process runs as.
func processName(pid int) string {
	info, err := unix.SysctlKinfoProc("kern.proc.pid", pid)
	if err != nil {
		return ""
	}
	return unix.ByteSliceToString(info.Proc.P_comm[:])
}

// maxArgs bounds the arguments read of a process.
const maxArgs = 256

// processArgs is a process's arguments, the first the name it was run by;
// nil where the system does not say them — a process of another user's.
// kern.procargs2 gives their count, the executable's path, padding, then
// the arguments, each ended by a NUL.
func processArgs(pid int) []string {
	data, err := unix.SysctlRaw("kern.procargs2", pid)
	if err != nil || len(data) < 4 {
		return nil
	}
	count := int(binary.NativeEndian.Uint32(data[:4]))
	rest := data[4:]
	end := bytes.IndexByte(rest, 0)
	if end < 0 {
		return nil
	}
	rest = bytes.TrimLeft(rest[end:], "\x00")
	args := make([]string, 0, min(count, maxArgs))
	for len(args) < count && len(args) < maxArgs && len(rest) > 0 {
		end := bytes.IndexByte(rest, 0)
		if end < 0 {
			args = append(args, string(rest))
			break
		}
		args = append(args, string(rest[:end]))
		rest = rest[end+1:]
	}
	return args
}
