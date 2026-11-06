//go:build darwin && cgo

package main

/*
#include <pthread.h>
#include <signal.h>

static int block_preemption(void) {
	sigset_t mask;
	sigemptyset(&mask);
	sigaddset(&mask, SIGURG);
	return pthread_sigmask(SIG_BLOCK, &mask, NULL);
}
*/
import "C"

import (
	"fmt"
	"os"
	"runtime"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

func main() {
	// Native libraries can leave a thread with SIGURG blocked. An outstanding
	// Go preemption on that thread must not hold an in-place restart forever.
	runtime.LockOSThread()
	if code := C.block_preemption(); code != 0 {
		fmt.Fprintln(os.Stderr, "pthread_sigmask:", code)
		os.Exit(1)
	}
	for until := time.Now().Add(250 * time.Millisecond); time.Now().Before(until); {
	}
	fmt.Println("executing")
	if err := spawn.Exec("/bin/echo", []string{"echo", "executed"}, os.Environ()); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
