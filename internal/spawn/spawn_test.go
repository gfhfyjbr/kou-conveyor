//go:build unix

package spawn

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// With KOU_SPAWN_HELPER set, the test binary is a command for the tests
// to run: it does what helpers has under that name, and exits.
func TestMain(m *testing.M) {
	name := os.Getenv("KOU_SPAWN_HELPER")
	if name == "" {
		// Descriptors the tests were started with beyond 0, 1 and 2, which a
		// CI runner leaves open, are not theirs: os/exec's fork would pass
		// them on to every command, where the tests count descriptors.
		for fd := 3; fd < 1024; fd++ {
			syscall.CloseOnExec(fd)
		}
		os.Exit(m.Run())
	}
	helper, ok := helpers[name]
	if !ok {
		fmt.Fprintf(os.Stderr, "no helper %q\n", name)
		os.Exit(2)
	}
	helper()
	os.Exit(0)
}

var helpers = map[string]func(){
	// fds prints each open descriptor and the file at it, as fileID
	// names it.
	"fds": func() {
		var lines []string
		for fd := range 1024 {
			if id, ok := fileID(fd); ok {
				lines = append(lines, strconv.Itoa(fd)+" "+id)
			}
		}
		fmt.Println(strings.Join(lines, "\n"))
	},
}

// fileID names the file open at fd, if one is.
func fileID(fd int) (string, bool) {
	var stat syscall.Stat_t
	if syscall.Fstat(fd, &stat) != nil {
		return "", false
	}
	return fmt.Sprintf("%d:%d", stat.Dev, stat.Ino), true
}

func parseDescriptors(t *testing.T, listing string) map[int]string {
	t.Helper()
	files := map[int]string{}
	for line := range strings.Lines(listing) {
		fd, id, ok := strings.Cut(strings.TrimSpace(line), " ")
		number, err := strconv.Atoi(fd)
		if !ok || err != nil {
			t.Fatalf("a line %q in %q", line, listing)
		}
		files[number] = id
	}
	return files
}

func helperCommand(t *testing.T, name string) *Cmd {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	cmd := Command(exe)
	cmd.Env = append(os.Environ(), "KOU_SPAWN_HELPER="+name)
	return cmd
}

// eachMode runs test with processes that posix_spawn makes, where it can,
// and with those that os/exec's fork makes: what the test checks must hold
// for both.
func eachMode(t *testing.T, test func(t *testing.T)) {
	t.Helper()
	for _, fork := range []bool{false, true} {
		name := "posix_spawn"
		if fork {
			name = "fork"
		}
		t.Run(name, func(t *testing.T) {
			if !fork && !spawnable(nil) {
				t.Skip("posix_spawn does not start processes here")
			}
			forceFork = fork
			t.Cleanup(func() { forceFork = false })
			test(t)
		})
	}
}

func errorText(err error) string {
	if err == nil {
		return "<nil>"
	}
	return err.Error()
}

func writeFile(t *testing.T, name, content string, mode os.FileMode) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, []byte(content), mode); err != nil {
		t.Fatal(err)
	}
	return path
}

func openFile(t *testing.T, name string) *os.File {
	t.Helper()
	file, err := os.Open(name)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { file.Close() })
	return file
}

func TestRunReportsHowTheCommandEnded(t *testing.T) {
	for _, test := range []struct {
		script string
		want   string
		code   int
	}{
		{"exit 0", "<nil>", 0},
		{"exit 3", "exit status 3", 3},
		{"kill -TERM $$", "signal: terminated", -1},
	} {
		t.Run(test.script, func(t *testing.T) {
			eachMode(t, func(t *testing.T) {
				cmd := Command("/bin/sh", "-c", test.script)
				err := cmd.Run()
				if errorText(err) != test.want || cmd.ProcessState.ExitCode() != test.code {
					t.Errorf("Run() = %v with exit code %d; want %s, %d", err, cmd.ProcessState.ExitCode(), test.want, test.code)
				}
				var exitErr *exec.ExitError
				if err != nil && !errors.As(err, &exitErr) {
					t.Errorf("Run() = %T, want an *exec.ExitError", err)
				}
			})
		})
	}
}

func TestOutputKeepsTheStandardErrorOfAFailure(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		out, err := Command("/bin/sh", "-c", "echo out; echo err >&2; exit 1").Output()
		var exitErr *exec.ExitError
		if string(out) != "out\n" || !errors.As(err, &exitErr) || string(exitErr.Stderr) != "err\n" {
			t.Errorf("Output() = %q, %v; want \"out\\n\" and an ExitError with \"err\\n\"", out, err)
		}
	})

	// Of a long one, its start and its end, as os/exec keeps them.
	script := `head -c 100000 /dev/zero | tr '\0' x >&2; exit 1`
	_, err := exec.Command("/bin/sh", "-c", script).Output()
	var wantErr *exec.ExitError
	if !errors.As(err, &wantErr) || !bytes.Contains(wantErr.Stderr, []byte("... omitting 34464 bytes ...")) {
		t.Fatalf("os/exec: %v", err)
	}
	eachMode(t, func(t *testing.T) {
		_, err := Command("/bin/sh", "-c", script).Output()
		var exitErr *exec.ExitError
		if !errors.As(err, &exitErr) || !bytes.Equal(exitErr.Stderr, wantErr.Stderr) {
			t.Errorf("Output() = %v: the standard error differs from os/exec's", err)
		}
	})
}

func TestCombinedOutputGivesBothStreamsOnePipe(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		out, err := helperCommand(t, "fds").CombinedOutput()
		if err != nil {
			t.Fatal(err)
		}
		files := parseDescriptors(t, string(out))
		if files[1] == "" || files[1] != files[2] {
			t.Errorf("the command's 1 is %q and 2 is %q: want the same pipe", files[1], files[2])
		}
	})
	eachMode(t, func(t *testing.T) {
		out, err := Command("/bin/sh", "-c", "echo a; echo b >&2; echo c; exit 2").CombinedOutput()
		if string(out) != "a\nb\nc\n" || errorText(err) != "exit status 2" {
			t.Errorf("CombinedOutput() = %q, %v", out, err)
		}
	})
}

func TestStandardInput(t *testing.T) {
	for _, test := range []struct {
		name  string
		stdin func(t *testing.T) io.Reader
		want  string
	}{
		{"none", func(*testing.T) io.Reader { return nil }, ""},
		{"reader", func(*testing.T) io.Reader { return strings.NewReader("from a reader") }, "from a reader"},
		{"file", func(t *testing.T) io.Reader {
			return openFile(t, writeFile(t, "stdin", "from a file", 0o644))
		}, "from a file"},
	} {
		t.Run(test.name, func(t *testing.T) {
			eachMode(t, func(t *testing.T) {
				cmd := Command("/bin/cat")
				cmd.Stdin = test.stdin(t)
				if out, err := cmd.Output(); string(out) != test.want || err != nil {
					t.Errorf("Output() = %q, %v; want %q", out, err, test.want)
				}
			})
		})
	}
}

func TestStandardInputTheCommandLeavesIsNoError(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		cmd := Command("/usr/bin/true")
		cmd.Stdin = bytes.NewReader(make([]byte, 4<<20))
		if err := cmd.Run(); err != nil {
			t.Errorf("Run() = %v", err)
		}
	})
}

func TestPipes(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		cmd := Command("/bin/sh", "-c", "cat; echo to stderr >&2")
		stdin, err := cmd.StdinPipe()
		if err != nil {
			t.Fatal(err)
		}
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			t.Fatal(err)
		}
		stderr, err := cmd.StderrPipe()
		if err != nil {
			t.Fatal(err)
		}
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		if _, err := io.WriteString(stdin, "to stdin\n"); err != nil {
			t.Fatal(err)
		}
		if err := stdin.Close(); err != nil {
			t.Fatal(err)
		}
		out, err := io.ReadAll(stdout)
		if err != nil {
			t.Fatal(err)
		}
		errOut, err := io.ReadAll(stderr)
		if err != nil {
			t.Fatal(err)
		}
		if err := cmd.Wait(); err != nil {
			t.Fatal(err)
		}
		if string(out) != "to stdin\n" || string(errOut) != "to stderr\n" {
			t.Errorf("read %q and %q", out, errOut)
		}
		// Wait closed it too, as os/exec's does: the pipe is an *os.File.
		if err := stdin.Close(); !errors.Is(err, os.ErrClosed) {
			t.Errorf("closing the standard input again: %v, want %v", err, os.ErrClosed)
		}
	})
}

func TestStandardOutputToAFile(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		path := writeFile(t, "stdout", "", 0o644)
		file, err := os.OpenFile(path, os.O_WRONLY, 0)
		if err != nil {
			t.Fatal(err)
		}
		defer file.Close()
		cmd := Command("/bin/sh", "-c", "echo to stdout; echo to stderr >&2")
		cmd.Stdout = file
		cmd.Stderr = file
		if err := cmd.Run(); err != nil {
			t.Fatal(err)
		}
		if got, err := os.ReadFile(path); string(got) != "to stdout\nto stderr\n" || err != nil {
			t.Errorf("the file has %q, %v", got, err)
		}
	})
}

func TestTheCommandGetsItsDescriptorsAndNoOthers(t *testing.T) {
	stdin := openFile(t, writeFile(t, "stdin", "", 0o644))
	first := openFile(t, writeFile(t, "first", "", 0o644))
	second := openFile(t, writeFile(t, "second", "", 0o644))
	devNull := openFile(t, os.DevNull)
	want := map[int]string{}
	for fd, file := range map[int]*os.File{0: stdin, 2: devNull, 3: os.Stderr, 4: os.Stdout, 5: os.Stdin, 7: first, 8: second} {
		id, ok := fileID(int(file.Fd()))
		if !ok {
			t.Skipf("nothing is open at the test's %d", file.Fd())
		}
		want[fd] = id
	}
	eachMode(t, func(t *testing.T) {
		var out bytes.Buffer
		cmd := helperCommand(t, "fds")
		cmd.Stdin = stdin
		cmd.Stdout = &out
		cmd.Stderr = devNull // a file: the command gets it as it is
		// The test's 2, 1 and 0 go to 3, 4 and 5: each to a place above
		// its own, which the command's 0, 1 and 2 take before.
		cmd.ExtraFiles = []*os.File{os.Stderr, os.Stdout, os.Stdin, nil, first, second}
		if err := cmd.Run(); err != nil {
			t.Fatal(err)
		}
		got := parseDescriptors(t, out.String())
		if got[1] == "" {
			t.Errorf("the command's 1 is closed")
		}
		delete(got, 1) // the pipe to out
		if fmt.Sprint(got) != fmt.Sprint(want) {
			t.Errorf("the command has\n%v\nwant\n%v", got, want)
		}
	})
}

func TestADescriptorAtItsOwnPlaceStaysOpen(t *testing.T) {
	file := openFile(t, writeFile(t, "file", "", 0o644))
	fd := int(file.Fd())
	id, ok := fileID(fd)
	if fd < 3 || !ok {
		t.Fatalf("the file is at %d", fd)
	}
	eachMode(t, func(t *testing.T) {
		cmd := helperCommand(t, "fds")
		cmd.ExtraFiles = make([]*os.File, fd-2)
		cmd.ExtraFiles[fd-3] = file // the command's fd is ours
		out, err := cmd.Output()
		if err != nil {
			t.Fatal(err)
		}
		if got := parseDescriptors(t, string(out)); got[fd] != id {
			t.Errorf("the command has %v: want the file at %d", got, fd)
		}
	})
}

func TestNoDescriptorWithoutCloseOnExecLeaks(t *testing.T) {
	if !spawnable(nil) {
		t.Skip("os/exec starts processes here: it leaves such descriptors open")
	}
	file := openFile(t, writeFile(t, "file", "", 0o644))
	fd, err := syscall.Dup(int(file.Fd())) // without close-on-exec
	if err != nil {
		t.Fatal(err)
	}
	defer syscall.Close(fd)
	out, err := helperCommand(t, "fds").Output()
	if err != nil {
		t.Fatal(err)
	}
	if got := parseDescriptors(t, string(out)); len(got) != 3 {
		t.Errorf("the command has %v, want only 0, 1 and 2 (%d is ours)", got, fd)
	}
}

func TestDirAndEnvironment(t *testing.T) {
	dir := t.TempDir()
	real, err := filepath.EvalSymlinks(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "here.sh"), []byte("#!/bin/sh\necho here in $(pwd -P)\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	inDir := func(cmd *Cmd) *Cmd {
		cmd.Dir = dir
		return cmd
	}
	withEnv := func(cmd *Cmd, env ...string) *Cmd {
		cmd.Env = env
		return cmd
	}
	inherited := inDir(Command("/usr/bin/env"))
	for _, test := range []struct {
		name string
		cmd  *Cmd
		want string
	}{
		{"dir", inDir(Command("/bin/pwd", "-P")), real + "\n"},
		{"inherited with PWD", inherited, strings.Join(inherited.Environ(), "\n") + "\n"},
		{"set, without PWD", withEnv(inDir(Command("/usr/bin/env")), "A=1", "B=2", "A=3"), "B=2\nA=3\n"},
		{"relative to dir", inDir(Command("./here.sh")), "here in " + real + "\n"},
	} {
		t.Run(test.name, func(t *testing.T) {
			eachMode(t, func(t *testing.T) {
				cmd := Command(test.cmd.Path, test.cmd.Args[1:]...)
				cmd.Dir, cmd.Env = test.cmd.Dir, test.cmd.Env
				if out, err := cmd.Output(); string(out) != test.want || err != nil {
					t.Errorf("Output() = %q, %v; want %q", out, err, test.want)
				}
			})
		})
	}
	if !slices.Contains(inherited.Environ(), "PWD="+dir) {
		t.Errorf("Environ() has no PWD=%s", dir)
	}
}

func TestStartFailsAsOsExecDoes(t *testing.T) {
	notExecutable := writeFile(t, "plain", "echo hi\n", 0o644)
	notAProgram := writeFile(t, "data", "\x00\x01\x02\x03", 0o755)
	canceled, cancel := context.WithCancel(context.Background())
	cancel()
	missingDir := "/nonexistent/kou-spawn"
	for _, test := range []struct {
		name string
		cmd  func() *Cmd
		want string
	}{
		{"missing", func() *Cmd { return Command("/nonexistent/kou-spawn") },
			"fork/exec /nonexistent/kou-spawn: no such file or directory"},
		{"not executable", func() *Cmd { return Command(notExecutable) },
			"fork/exec " + notExecutable + ": permission denied"},
		{"not a program", func() *Cmd { return Command(notAProgram) },
			"fork/exec " + notAProgram + ": exec format error"},
		{"missing dir", func() *Cmd {
			cmd := Command("/bin/sh", "-c", "true")
			cmd.Dir = missingDir
			return cmd
		}, "chdir /nonexistent/kou-spawn: no such file or directory"},
		{"missing dir with attributes", func() *Cmd {
			cmd := Command("/bin/sh", "-c", "true")
			cmd.Dir = missingDir
			cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
			return cmd
		}, "fork/exec /bin/sh: no such file or directory"},
		{"NUL in an argument", func() *Cmd { return Command("/bin/echo", "a\x00b") },
			"fork/exec /bin/echo: invalid argument"},
		{"NUL in the environment", func() *Cmd {
			cmd := Command("/bin/echo")
			cmd.Env = []string{"A=\x00"}
			return cmd
		}, "exec: environment variable contains NUL"},
		{"NUL in dir", func() *Cmd {
			cmd := Command("/bin/echo")
			cmd.Dir = "/tmp\x00"
			return cmd
		}, "exec: environment variable contains NUL"},
		{"NUL in dir with an environment", func() *Cmd {
			cmd := Command("/bin/echo")
			cmd.Env = []string{}
			cmd.Dir = "/tmp\x00"
			return cmd
		}, "chdir /tmp\x00: invalid argument"},
		{"no command", func() *Cmd { return Command("") }, "exec: no command"},
		{"not in PATH", func() *Cmd { return Command("kou-spawn-no-such-command") },
			`exec: "kou-spawn-no-such-command": executable file not found in $PATH`},
		{"Cancel without a context", func() *Cmd {
			cmd := Command("/bin/echo")
			cmd.Cancel = func() error { return nil }
			return cmd
		}, "exec: command with a non-nil Cancel was not created with CommandContext"},
		{"context done", func() *Cmd { return CommandContext(canceled, "/bin/echo") }, "context canceled"},
	} {
		t.Run(test.name, func(t *testing.T) {
			eachMode(t, func(t *testing.T) {
				cmd := test.cmd()
				stdin, err := cmd.StdinPipe()
				if err != nil {
					t.Fatal(err)
				}
				if err := cmd.Start(); errorText(err) != test.want {
					t.Errorf("Start() = %v, want %s", err, test.want)
				}
				if cmd.Process != nil {
					t.Errorf("Start() failed, but Process is %v", cmd.Process)
				}
				if _, err := stdin.Write([]byte("x")); !errors.Is(err, os.ErrClosed) {
					t.Errorf("writing to the standard input of a command that did not start: %v, want it closed", err)
				}
			})
		})
	}
}

func TestMisuseFailsAsOsExecDoes(t *testing.T) {
	eachMode(t, func(t *testing.T) {
		check := func(what string, err error, want string) {
			t.Helper()
			if errorText(err) != want {
				t.Errorf("%s: %v, want %s", what, err, want)
			}
		}
		cmd := Command("/usr/bin/true")
		check("Wait before Start", cmd.Wait(), "exec: not started")
		check("Start", cmd.Start(), "<nil>")
		check("Start again", cmd.Start(), "exec: already started")
		_, err := cmd.StdinPipe()
		check("StdinPipe", err, "exec: StdinPipe after process started")
		_, err = cmd.StdoutPipe()
		check("StdoutPipe", err, "exec: StdoutPipe after process started")
		_, err = cmd.StderrPipe()
		check("StderrPipe", err, "exec: StderrPipe after process started")
		check("Wait", cmd.Wait(), "<nil>")
		check("Wait again", cmd.Wait(), "exec: Wait was already called")

		cmd = Command("/usr/bin/true")
		cmd.Stdout = io.Discard
		_, err = cmd.Output()
		check("Output with Stdout", err, "exec: Stdout already set")
		_, err = cmd.CombinedOutput()
		check("CombinedOutput with Stdout", err, "exec: Stdout already set")
		_, err = cmd.StdoutPipe()
		check("StdoutPipe with Stdout", err, "exec: Stdout already set")
		cmd = Command("/usr/bin/true")
		cmd.Stderr = io.Discard
		_, err = cmd.CombinedOutput()
		check("CombinedOutput with Stderr", err, "exec: Stderr already set")
	})
}

func TestContext(t *testing.T) {
	t.Run("kills at the deadline", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
			defer cancel()
			began := time.Now()
			err := CommandContext(ctx, "/bin/sleep", "10").Run()
			if errorText(err) != "signal: killed" || time.Since(began) > 5*time.Second {
				t.Errorf("Run() = %v after %v", err, time.Since(began))
			}
		})
	})
	t.Run("kills once WaitDelay passes after Cancel", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
			defer cancel()
			cmd := CommandContext(ctx, "/bin/sh", "-c", `trap "" TERM; exec /bin/sleep 10`)
			cmd.Cancel = func() error { return cmd.Process.Signal(syscall.SIGTERM) }
			cmd.WaitDelay = 300 * time.Millisecond
			began := time.Now()
			err := cmd.Run()
			if elapsed := time.Since(began); errorText(err) != "signal: killed" || elapsed < 400*time.Millisecond || elapsed > 5*time.Second {
				t.Errorf("Run() = %v after %v", err, elapsed)
			}
		})
	})
	t.Run("a command that ends after Cancel ends by the context", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
			defer cancel()
			cmd := CommandContext(ctx, "/bin/sleep", "0.4")
			cmd.Cancel = func() error { return nil }
			if err := cmd.Run(); !errors.Is(err, context.DeadlineExceeded) {
				t.Errorf("Run() = %v, want %v", err, context.DeadlineExceeded)
			}
		})
	})
	t.Run("reports an error of Cancel", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
			defer cancel()
			cmd := CommandContext(ctx, "/bin/sleep", "0.4")
			cmd.Cancel = func() error { return errors.New("not now") }
			if err := cmd.Run(); errorText(err) != "exec: canceling Cmd: not now" {
				t.Errorf("Run() = %v", err)
			}
		})
	})
}

func TestWaitDelay(t *testing.T) {
	// The command leaves a process that holds its standard output open:
	// once WaitDelay passes, Wait stops waiting for the output to end.
	leaveAProcess := func(t *testing.T, cmd *Cmd) *bytes.Buffer {
		var out bytes.Buffer
		cmd.Stdout = &out
		cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
		cmd.WaitDelay = 300 * time.Millisecond
		t.Cleanup(func() {
			if cmd.Process != nil {
				syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
			}
		})
		return &out
	}
	t.Run("after the command exits", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			cmd := Command("/bin/sh", "-c", "/bin/sleep 10 & echo started")
			out := leaveAProcess(t, cmd)
			began := time.Now()
			err := cmd.Run()
			if !errors.Is(err, exec.ErrWaitDelay) || out.String() != "started\n" || time.Since(began) > 5*time.Second {
				t.Errorf("Run() = %v after %v with %q", err, time.Since(began), out)
			}
		})
	})
	t.Run("after Cancel", func(t *testing.T) {
		eachMode(t, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
			defer cancel()
			cmd := CommandContext(ctx, "/bin/sh", "-c", `trap "" TERM; /bin/sleep 10 & echo started; wait`)
			out := leaveAProcess(t, cmd)
			cmd.Cancel = func() error { return cmd.Process.Signal(syscall.SIGTERM) }
			began := time.Now()
			err := cmd.Run()
			if errorText(err) != "signal: killed" || out.String() != "started\n" || time.Since(began) > 5*time.Second {
				t.Errorf("Run() = %v after %v with %q", err, time.Since(began), out)
			}
		})
	})
}
