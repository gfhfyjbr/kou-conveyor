//go:build unix

package main

import (
	"os"
	"syscall"
)

// execute runs exe in the relay's stead — the same process, with the same
// arguments and environment — telling it what it took the place of.
func execute(exe, from string) error {
	env := append(withoutVariable(os.Environ(), updatedFrom), updatedFrom+"="+from)
	return syscall.Exec(exe, os.Args, env)
}
