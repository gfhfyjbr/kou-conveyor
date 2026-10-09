//go:build !unix

package canvascli

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
)

// execProgram runs a program and waits for it, where a process cannot
// become another.
func execProgram(a *app, path string, argv []string) int {
	command := exec.Command(path, argv[1:]...)
	command.Stdin, command.Stdout, command.Stderr = os.Stdin, os.Stdout, os.Stderr
	err := command.Run()
	var exit *exec.ExitError
	switch {
	case errors.As(err, &exit):
		return exit.ExitCode()
	case err != nil:
		fmt.Fprintf(a.err, "kou-canvas launch: %s: %v\n", argv[0], err)
		return 126
	}
	return 0
}
