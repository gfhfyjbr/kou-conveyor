//go:build unix

package spawn_test

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"golang.org/x/sys/unix"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

func TestExecReplacesProcess(t *testing.T) {
	if os.Getenv("SPAWN_TEST_EXEC") == "1" {
		// TestMain marks inherited descriptors close-on-exec; hand this one over.
		if _, err := unix.FcntlInt(3, unix.F_SETFD, 0); err != nil {
			t.Fatal(err)
		}
		closed, err := os.Create(filepath.Join(os.Getenv("SPAWN_TEST_DIR"), "cloexec"))
		if err != nil {
			t.Fatal(err)
		}
		defer os.Remove(closed.Name())
		defer closed.Close()
		script := `printf '%s\n' "$$" "$SPAWN_MARKER" "$1"; /bin/cat <&3; test ! -e "/dev/fd/$SPAWN_CLOSED_FD"`
		env := append(os.Environ(), "SPAWN_MARKER=environment with spaces", fmt.Sprintf("SPAWN_CLOSED_FD=%d", closed.Fd()))
		if err := spawn.Exec("/bin/sh", []string{"sh", "-c", script, "sh", "argument with spaces"}, env); err != nil {
			t.Fatal(err)
		}
		t.Fatal("Exec returned without replacing the process")
	}
	file, err := os.CreateTemp(t.TempDir(), "preserved-*")
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if _, err := file.WriteString("inherited descriptor\n"); err != nil {
		t.Fatal(err)
	}
	if _, err := file.Seek(0, 0); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	cmd := spawn.CommandContext(ctx, os.Args[0], "-test.run=^TestExecReplacesProcess$")
	cmd.Env = append(os.Environ(), "SPAWN_TEST_EXEC=1", "SPAWN_TEST_DIR="+t.TempDir())
	cmd.ExtraFiles = []*os.File{file}
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("Exec: %v\n%s", err, out)
	}
	want := fmt.Sprintf("%d\nenvironment with spaces\nargument with spaces\ninherited descriptor\n", cmd.Process.Pid)
	if string(out) != want {
		t.Fatalf("Exec output = %q, want %q", out, want)
	}
}

func TestExecFailureLeavesProcessUsable(t *testing.T) {
	for _, tt := range []struct {
		name string
		path string
		argv []string
		env  []string
		want error
	}{
		{"missing", filepath.Join(t.TempDir(), "missing"), []string{"missing"}, nil, os.ErrNotExist},
		{"path NUL", "/bin/echo\x00", []string{"echo"}, nil, syscall.EINVAL},
		{"argument NUL", "/bin/echo", []string{"echo", "a\x00b"}, nil, syscall.EINVAL},
		{"environment NUL", "/bin/echo", []string{"echo"}, []string{"KEY=a\x00b"}, syscall.EINVAL},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if err := spawn.Exec(tt.path, tt.argv, tt.env); !errors.Is(err, tt.want) {
				t.Fatalf("Exec = %v, want %v", err, tt.want)
			}
			ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
			defer cancel()
			out, err := spawn.CommandContext(ctx, "/bin/echo", "still running").CombinedOutput()
			if err != nil || strings.TrimSpace(string(out)) != "still running" {
				t.Fatalf("process unusable after failed Exec: %v, %s", err, out)
			}
		})
	}
}
