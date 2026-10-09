//go:build unix

package canvascli

import (
	"fmt"
	"os"
	"syscall"
)

// execProgram runs a program in kou-canvas's place.
func execProgram(a *app, path string, argv []string) int {
	err := syscall.Exec(path, argv, os.Environ())
	fmt.Fprintf(a.err, "kou-canvas launch: %s: %v\n", argv[0], err)
	return 126
}
