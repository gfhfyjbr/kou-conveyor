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
