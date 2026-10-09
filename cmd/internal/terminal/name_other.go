//go:build !darwin

package terminal

import (
	"os"
	"strconv"
	"strings"
)

// processName is the name a process runs as, where /proc says.
func processName(pid int) string {
	data, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/comm")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(data))
}

// maxArgs bounds the arguments read of a process.
const maxArgs = 256

// processArgs is a process's arguments, the first the name it was run by,
// where /proc says them.
func processArgs(pid int) []string {
	data, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/cmdline")
	if err != nil || len(data) == 0 {
		return nil
	}
	args := strings.Split(strings.TrimRight(string(data), "\x00"), "\x00")
	return args[:min(len(args), maxArgs)]
}
