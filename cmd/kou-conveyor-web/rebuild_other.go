//go:build !unix

package main

import (
	"errors"
	"net"
)

// execSelf cannot run a program anew in place here: the server is restarted
// by hand to take the new build up.
func execSelf(string, net.Listener) error {
	return errors.New("restarting in place needs a Unix system")
}
