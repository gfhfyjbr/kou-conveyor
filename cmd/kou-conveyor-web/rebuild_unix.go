//go:build unix

package main

import (
	"errors"
	"fmt"
	"net"
	"os"
	"strings"

	"golang.org/x/sys/unix"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

// execSelf runs the program anew in this process, handing it the listening
// socket, which it takes up in place of opening its own, and env besides
// the environment.
func execSelf(exe string, listener net.Listener, env ...string) error {
	tcp, ok := listener.(*net.TCPListener)
	if !ok {
		return errors.New("the listener is not a TCP socket")
	}
	file, err := tcp.File() // a copy, closed on exec unless told otherwise
	if err != nil {
		return err
	}
	defer file.Close()
	// File.Fd switches the shared listening socket to blocking mode, leaving
	// Accept and shutdown stuck if exec fails. Control preserves its flags.
	raw, err := file.SyscallConn()
	if err != nil {
		return err
	}
	var fd uintptr
	var flagsErr error
	if err := raw.Control(func(value uintptr) {
		fd = value
		_, flagsErr = unix.FcntlInt(fd, unix.F_SETFD, 0)
	}); err != nil {
		return err
	}
	if flagsErr != nil {
		return flagsErr
	}
	environment := make([]string, 0, len(os.Environ())+1+len(env))
	for _, value := range os.Environ() {
		if !strings.HasPrefix(value, listenerEnvironment+"=") && !strings.HasPrefix(value, terminalsEnvironment+"=") {
			environment = append(environment, value)
		}
	}
	environment = append(environment, fmt.Sprintf("%s=%d", listenerEnvironment, fd))
	environment = append(environment, env...)
	return spawn.Exec(exe, os.Args, environment)
}
