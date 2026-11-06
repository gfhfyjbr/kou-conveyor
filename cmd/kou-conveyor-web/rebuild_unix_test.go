//go:build unix

package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"golang.org/x/sys/unix"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

func TestFailedExecSelfPreservesNonblockingListener(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	if err := execSelf(filepath.Join(t.TempDir(), "missing"), listener); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("execSelf = %v, want a missing executable", err)
	}
	raw, err := listener.(*net.TCPListener).SyscallConn()
	if err != nil {
		t.Fatal(err)
	}
	var flags int
	var flagsErr error
	if err := raw.Control(func(fd uintptr) {
		flags, flagsErr = unix.FcntlInt(fd, unix.F_GETFL, 0)
	}); err != nil {
		t.Fatal(err)
	}
	if flagsErr != nil || flags&unix.O_NONBLOCK == 0 {
		t.Fatalf("a failed exec left the listener blocking: flags %#x, error %v", flags, flagsErr)
	}
}

func TestExecSelfKeepsListener(t *testing.T) {
	switch os.Getenv("KOU_TEST_EXEC_SELF") {
	case "before":
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		defer listener.Close()
		os.Unsetenv("KOU_TEST_EXEC_SELF")
		err = execSelf(os.Args[0], listener,
			"KOU_TEST_EXEC_SELF=after",
			"KOU_TEST_EXEC_PID="+fmt.Sprint(os.Getpid()),
			"KOU_TEST_EXEC_ADDR="+listener.Addr().String())
		t.Fatalf("execSelf returned: %v", err)
	case "after":
		if fmt.Sprint(os.Getpid()) != os.Getenv("KOU_TEST_EXEC_PID") {
			t.Fatal("the replacement changed the process ID")
		}
		listener, err := inheritedListener()
		if err != nil || listener == nil {
			t.Fatalf("inheritedListener = %v, %v", listener, err)
		}
		defer listener.Close()
		if listener.Addr().String() != os.Getenv("KOU_TEST_EXEC_ADDR") {
			t.Fatal("the replacement changed the listening address")
		}
		listener.(*net.TCPListener).SetDeadline(time.Now().Add(3 * time.Second))
		go func() {
			if conn, err := net.DialTimeout("tcp", listener.Addr().String(), 3*time.Second); err == nil {
				conn.Close()
			}
		}()
		conn, err := listener.Accept()
		if err != nil {
			t.Fatalf("accept after replacement: %v", err)
		}
		conn.Close()
		fmt.Println("listener survived")
		return
	}
	ctx, cancel := context.WithTimeout(t.Context(), 15*time.Second)
	defer cancel()
	cmd := spawn.CommandContext(ctx, os.Args[0], "-test.run=^TestExecSelfKeepsListener$")
	cmd.Env = append(os.Environ(), "KOU_TEST_EXEC_SELF=before")
	out, err := cmd.CombinedOutput()
	if err != nil || !strings.Contains(string(out), "listener survived") {
		t.Fatalf("execSelf: %v\n%s", err, out)
	}
}
