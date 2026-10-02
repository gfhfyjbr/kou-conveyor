package codevm

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode/utf8"
)

// commandResult is what a shell command left: its streams, bounded, and
// how it ended.
type commandResult struct {
	Stdout   string
	Stderr   string
	ExitCode int
	TimedOut bool
	Duration time.Duration
}

const terminationGrace = 5 * time.Second

// runCommand runs shell -c command in directory, its streams captured up to
// limit characters each, for at most timeout (0 for no limit). The
// command's process group is terminated when the context ends or the
// timeout passes.
func runCommand(ctx context.Context, shell, directory, command string, environment []string, limit int, timeout time.Duration) (commandResult, error) {
	if shell == "" {
		return commandResult{}, errors.New("no shell is configured")
	}
	runContext, cancel := context.WithCancel(ctx)
	defer cancel()
	cmd := exec.Command(shell, "-c", command)
	cmd.Dir = directory
	cmd.Env = environment
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	stdout, stderr := newBoundedBuffer(limit), newBoundedBuffer(limit)
	cmd.Stdout, cmd.Stderr = stdout, stderr
	started := time.Now()
	if err := cmd.Start(); err != nil {
		return commandResult{}, fmt.Errorf("start %s: %w", shell, err)
	}
	var timedOut bool
	var once sync.Once
	terminate := func(reason bool) {
		once.Do(func() {
			timedOut = reason
			syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)
			go func() {
				select {
				case <-time.After(terminationGrace):
					syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
				case <-runContext.Done():
				}
			}()
		})
	}
	done, watched := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(watched)
		var deadline <-chan time.Time
		if timeout > 0 {
			timer := time.NewTimer(timeout)
			defer timer.Stop()
			deadline = timer.C
		}
		select {
		case <-ctx.Done():
			terminate(false)
		case <-deadline:
			terminate(true)
		case <-done:
		}
	}()
	waitErr := cmd.Wait()
	close(done)
	<-watched // timedOut is the watcher's to write
	result := commandResult{Stdout: stdout.String(), Stderr: stderr.String(), Duration: time.Since(started), TimedOut: timedOut}
	var exit *exec.ExitError
	switch {
	case waitErr == nil:
	case errors.As(waitErr, &exit):
		if status, ok := exit.Sys().(syscall.WaitStatus); ok && status.Signaled() {
			result.ExitCode = 128 + int(status.Signal())
		} else {
			result.ExitCode = exit.ExitCode()
		}
	default:
		return result, waitErr
	}
	if ctx.Err() != nil {
		return result, ctx.Err()
	}
	return result, nil
}

// boundedBuffer keeps the head and the tail of what is written to it, up
// to limit characters together, and counts the rest.
type boundedBuffer struct {
	mu    sync.Mutex
	limit int
	head  []byte
	tail  []byte // a ring of the last bytes once head is full
	at    int    // where the ring writes next
	full  bool   // the ring wrapped
	total int64
}

func newBoundedBuffer(limit int) *boundedBuffer {
	return &boundedBuffer{limit: max(limit, 16)}
}

func (buffer *boundedBuffer) Write(data []byte) (int, error) {
	buffer.mu.Lock()
	defer buffer.mu.Unlock()
	written := len(data)
	buffer.total += int64(len(data))
	if room := buffer.limit - len(buffer.head); room > 0 {
		n := min(room, len(data))
		buffer.head = append(buffer.head, data[:n]...)
		data = data[n:]
	}
	if len(data) == 0 {
		return written, nil
	}
	if buffer.tail == nil {
		buffer.tail = make([]byte, buffer.limit)
	}
	for len(data) > 0 {
		n := copy(buffer.tail[buffer.at:], data)
		data = data[n:]
		buffer.at += n
		if buffer.at == len(buffer.tail) {
			buffer.at, buffer.full = 0, true
		}
	}
	return written, nil
}

// String is what was written, cut in the middle when it was too long.
func (buffer *boundedBuffer) String() string {
	buffer.mu.Lock()
	defer buffer.mu.Unlock()
	if buffer.total <= int64(len(buffer.head)) {
		return strings.ToValidUTF8(string(buffer.head), "\uFFFD")
	}
	var tail []byte
	if buffer.full {
		tail = append(append([]byte{}, buffer.tail[buffer.at:]...), buffer.tail[:buffer.at]...)
	} else {
		tail = buffer.tail[:buffer.at]
	}
	headKeep := buffer.limit / 2
	tailKeep := buffer.limit - headKeep
	head := buffer.head[:min(headKeep, len(buffer.head))]
	if len(tail) > tailKeep {
		tail = tail[len(tail)-tailKeep:]
	}
	for len(head) > 0 && !utf8.Valid(head) {
		head = head[:len(head)-1]
	}
	for len(tail) > 0 && !utf8.RuneStart(tail[0]) {
		tail = tail[1:]
	}
	skipped := buffer.total - int64(len(head)) - int64(len(tail))
	return fmt.Sprintf("%s...%d bytes truncated...%s", head, skipped, tail)
}
