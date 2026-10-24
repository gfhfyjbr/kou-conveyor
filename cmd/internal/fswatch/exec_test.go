//go:build unix

package fswatch_test

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/fswatch"
)

// A process that watches can still execute a program: the web server
// restarts itself with a new build so. Go called back on a thread of the
// system's, as FSEvents' queue's are, can be left owing a preemption signal
// the thread never takes, and syscall.Exec then waits for it for ever
// (runtime_BeforeExec on darwin) — with the process's other goroutines
// stalled too, at times, so the test's binary, run again, does the watching:
// it makes changes while the collector stops the world often, then executes
// echo.
func TestExecAfterChanges(t *testing.T) {
	if dir := os.Getenv("FSWATCH_TEST_EXEC"); dir != "" {
		execAfterChanges(dir)
		return
	}
	watching(t, t.TempDir()) // skips where the system tells of no changes
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestExecAfterChanges$")
	cmd.Env = append(os.Environ(), "FSWATCH_TEST_EXEC="+t.TempDir())
	out, err := cmd.CombinedOutput()
	if ctx.Err() != nil {
		t.Fatalf("syscall.Exec still waited after 30 s\n%s", out)
	}
	if !strings.Contains(string(out), "executed") {
		t.Fatalf("%v\n%s", err, out)
	}
}

// execAfterChanges is TestExecAfterChanges's process: it ends as echo, or
// says why not. The watcher stays open, as the server's do.
func execAfterChanges(dir string) {
	w, err := fswatch.Watch(dir)
	if err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
	var stop atomic.Bool
	for range 2 {
		go func() { // garbage, for the collector to stop the world often
			var kept [][]byte
			for !stop.Load() {
				kept = append(kept, make([]byte, 64<<10))
				if len(kept) > 64 {
					kept = kept[:0]
				}
			}
		}()
	}
	for end, i := time.Now().Add(3*time.Second), 0; time.Now().Before(end); i++ {
		if err := os.WriteFile(filepath.Join(dir, "file"), []byte{byte(i)}, 0o644); err != nil {
			fmt.Println(err)
			os.Exit(1)
		}
		select {
		case <-w.Changes():
		case <-time.After(time.Second):
		}
	}
	stop.Store(true)
	err = syscall.Exec("/bin/echo", []string{"echo", "executed"}, os.Environ())
	fmt.Println("exec:", err)
	os.Exit(1)
}
