//go:build !unix

package main

import "errors"

// execute cannot run a build in the relay's stead here: the relay exits,
// and a service manager starts the build now in its place.
func execute(exe, from string) error {
	return errors.New("a process cannot be replaced on this system")
}
