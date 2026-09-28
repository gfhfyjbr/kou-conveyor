//go:build unix

package main

import (
	"errors"
	"fmt"
	"net"
	"os"
	"strings"
	"syscall"
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
	if _, _, errno := syscall.Syscall(syscall.SYS_FCNTL, file.Fd(), syscall.F_SETFD, 0); errno != 0 {
		file.Close()
		return errno
	}
	environment := make([]string, 0, len(os.Environ())+1+len(env))
	for _, value := range os.Environ() {
		if !strings.HasPrefix(value, listenerEnvironment+"=") && !strings.HasPrefix(value, terminalsEnvironment+"=") {
			environment = append(environment, value)
		}
	}
	environment = append(environment, fmt.Sprintf("%s=%d", listenerEnvironment, file.Fd()))
	environment = append(environment, env...)
	err = syscall.Exec(exe, os.Args, environment)
	file.Close()
	return err
}
