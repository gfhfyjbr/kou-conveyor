// Package spawn runs commands as os/exec does, with one difference on
// macOS: the process is made with posix_spawn, not with fork.
//
// After a fork on macOS, the pages the parent had in memory at that moment
// — its heap, live or freed but not yet given back — stay counted against
// the parent even once the Go runtime returns them: they stay in its
// footprint (the Memory column of Activity Monitor) and go to swap.
// posix_spawn copies nothing, so a heap that grew for a while and shrank
// goes back to the system. Other systems, and what posix_spawn cannot set
// up (a controlling terminal, credentials, chroot, a foreground group,
// ptrace), get os/exec as it is.
//
// Cmd embeds *exec.Cmd: set its fields as usual, but call the methods of
// Cmd — Start, Wait, Run, Output, CombinedOutput and the pipes — not those
// of the exec.Cmd inside it.
//
// One difference is left: a child that posix_spawn makes inherits the
// limit of open files that the Go runtime raised for this process, where
// os/exec gives back the limit the process started with.
package spawn

import (
	"bytes"
	"context"
	"errors"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// Cmd is a command to run; see os/exec.Cmd.
type Cmd struct {
	*exec.Cmd

	ctx     context.Context
	spawned bool // posix_spawn was to make the process; else os/exec starts it

	childFiles  []io.Closer // the child's ends of the pipes made here: closed once it has them
	parentPipes []io.Closer // ours: closed by Wait
	copiers     []func() error
	copied      <-chan error // the first error of the copiers, once all are done
	watched     <-chan watchResult
}

// watchResult is what watching the context left: an error, and the timer
// of WaitDelay when it was running as the process ended.
type watchResult struct {
	err   error
	timer *time.Timer
}

// forceFork makes every Cmd start with os/exec, for tests to compare the
// two.
var forceFork bool

// Command is exec.Command.
func Command(name string, arg ...string) *Cmd {
	return &Cmd{Cmd: exec.Command(name, arg...)}
}

// CommandContext is exec.CommandContext: Cancel kills the process when ctx
// is done, unless it is changed.
func CommandContext(ctx context.Context, name string, arg ...string) *Cmd {
	return &Cmd{Cmd: exec.CommandContext(ctx, name, arg...), ctx: ctx}
}

// Run starts the command and waits for it.
func (c *Cmd) Run() error {
	if err := c.Start(); err != nil {
		return err
	}
	return c.Wait()
}

// Start starts the command; Wait must be called after it succeeds.
func (c *Cmd) Start() error {
	if c.Process != nil {
		return errors.New("exec: already started")
	}
	if !spawnable(c.SysProcAttr) {
		err := c.Cmd.Start()
		closeAll(c.childFiles)
		c.childFiles = nil
		if err != nil {
			closeAll(c.parentPipes)
			c.parentPipes = nil
		}
		return err
	}
	c.spawned = true
	return c.spawn()
}

// spawn starts the command with posix_spawn, as exec.Cmd.Start would with
// fork: the same checks, descriptors, environment and errors.
func (c *Cmd) spawn() error {
	started := false
	defer func() {
		closeAll(c.childFiles)
		c.childFiles = nil
		if !started {
			closeAll(c.parentPipes)
			c.parentPipes = nil
			c.copiers = nil
		}
	}()

	if c.Path == "" && c.Err == nil {
		c.Err = errors.New("exec: no command")
	}
	if c.Err != nil {
		return c.Err
	}
	if c.Cancel != nil && c.ctx == nil {
		return errors.New("exec: command with a non-nil Cancel was not created with CommandContext")
	}
	if c.ctx != nil {
		select {
		case <-c.ctx.Done():
			return c.ctx.Err()
		default:
		}
	}

	stdin, err := c.childStdin()
	if err != nil {
		return err
	}
	stdout, err := c.writerFile(c.Stdout)
	if err != nil {
		return err
	}
	stderr := stdout
	if c.Stderr == nil || !interfaceEqual(c.Stderr, c.Stdout) {
		if stderr, err = c.writerFile(c.Stderr); err != nil {
			return err
		}
	}
	env, err := c.environ()
	if err != nil {
		return err
	}
	if c.SysProcAttr == nil && c.Dir != "" {
		// As os.StartProcess does: a clearer error than the child's.
		if _, err := os.Stat(c.Dir); err != nil {
			pathErr := err.(*fs.PathError)
			pathErr.Op = "chdir"
			return pathErr
		}
	}

	files := append([]*os.File{stdin, stdout, stderr}, c.ExtraFiles...)
	fds := make([]int, len(files))
	for i, file := range files {
		fds[i] = -1 // a nil file leaves the descriptor closed
		if file != nil {
			fds[i] = int(file.Fd())
		}
	}
	pid, err := posixSpawn(c.Path, c.argv(), env, c.Dir, fds, c.SysProcAttr)
	runtime.KeepAlive(files)
	if err != nil {
		return &fs.PathError{Op: "fork/exec", Path: c.Path, Err: err}
	}
	c.Process, _ = os.FindProcess(pid) // it cannot fail on unix
	started = true

	c.startCopiers()
	if (c.Cancel != nil || c.WaitDelay != 0) && c.ctx != nil && c.ctx.Done() != nil {
		watched := make(chan watchResult)
		c.watched = watched
		go c.watch(watched)
	}
	return nil
}

func (c *Cmd) argv() []string {
	if len(c.Args) > 0 {
		return c.Args
	}
	return []string{c.Path}
}

// environ is the environment of the command, as exec.Cmd.Start makes it,
// with the errors Start reports.
func (c *Cmd) environ() ([]string, error) {
	nul := false
	for _, entry := range c.Env {
		nul = nul || strings.IndexByte(entry, 0) >= 0
	}
	if c.Env == nil && c.Dir != "" {
		// Environ adds PWD for Dir.
		pwd, err := filepath.Abs(c.Dir)
		if err != nil {
			return nil, err
		}
		nul = strings.IndexByte(pwd, 0) >= 0
	}
	if nul {
		// Environ leaves such a variable out; Start fails on it.
		return nil, errors.New("exec: environment variable contains NUL")
	}
	return c.Environ(), nil
}

func (c *Cmd) childStdin() (*os.File, error) {
	if c.Stdin == nil {
		file, err := os.Open(os.DevNull)
		if err != nil {
			return nil, err
		}
		c.childFiles = append(c.childFiles, file)
		return file, nil
	}
	if file, ok := c.Stdin.(*os.File); ok {
		return file, nil
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	c.childFiles = append(c.childFiles, reader)
	c.parentPipes = append(c.parentPipes, writer)
	source := c.Stdin
	c.copiers = append(c.copiers, func() error {
		_, err := io.Copy(writer, source)
		if brokenPipe(err) {
			err = nil // the command did not read it all
		}
		if closeErr := writer.Close(); err == nil {
			err = closeErr
		}
		return err
	})
	return reader, nil
}

// brokenPipe reports a write to the stdin of a command that has stopped
// reading it, which os/exec does not count as an error.
func brokenPipe(err error) bool {
	pathErr, ok := err.(*fs.PathError)
	return ok && pathErr.Op == "write" && pathErr.Path == "|1" && pathErr.Err == syscall.EPIPE
}

// writerFile is the file the command writes to for w: /dev/null for none,
// w itself for a file, or else a pipe that a goroutine copies into w.
func (c *Cmd) writerFile(w io.Writer) (*os.File, error) {
	if w == nil {
		file, err := os.OpenFile(os.DevNull, os.O_WRONLY, 0)
		if err != nil {
			return nil, err
		}
		c.childFiles = append(c.childFiles, file)
		return file, nil
	}
	if file, ok := w.(*os.File); ok {
		return file, nil
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	c.childFiles = append(c.childFiles, writer)
	c.parentPipes = append(c.parentPipes, reader)
	c.copiers = append(c.copiers, func() error {
		_, err := io.Copy(w, reader)
		reader.Close() // in case io.Copy stopped on an error of w
		return err
	})
	return writer, nil
}

// startCopiers starts the goroutines that copy to and from the pipes; the
// first error of theirs arrives on c.copied once all are done.
func (c *Cmd) startCopiers() {
	if len(c.copiers) == 0 {
		return
	}
	copied := make(chan error, 1)
	c.copied = copied
	type status struct {
		running  int
		firstErr error
	}
	statuses := make(chan status, 1)
	statuses <- status{running: len(c.copiers)}
	for _, copier := range c.copiers {
		go func() {
			err := copier()
			current := <-statuses
			if current.firstErr == nil {
				current.firstErr = err
			}
			current.running--
			if current.running == 0 {
				copied <- current.firstErr
			} else {
				statuses <- current
			}
		}()
	}
	c.copiers = nil
}

// watch watches the context until Wait asks for its result: when it is
// done first, it cancels the command, and kills it once WaitDelay passes.
func (c *Cmd) watch(result chan<- watchResult) {
	select {
	case result <- watchResult{}:
		return
	case <-c.ctx.Done():
	}

	var err error
	if c.Cancel != nil {
		if interruptErr := c.Cancel(); interruptErr == nil {
			err = c.ctx.Err()
		} else if !errors.Is(interruptErr, os.ErrProcessDone) {
			err = wrappedError{prefix: "exec: canceling Cmd", err: interruptErr}
		}
	}
	if c.WaitDelay == 0 {
		result <- watchResult{err: err}
		return
	}

	timer := time.NewTimer(c.WaitDelay)
	select {
	case result <- watchResult{err: err, timer: timer}:
		return // Wait takes the timer from here
	case <-timer.C:
	}

	killed := false
	if killErr := c.Process.Kill(); killErr == nil {
		killed = true
	} else if !errors.Is(killErr, os.ErrProcessDone) {
		err = wrappedError{prefix: "exec: killing Cmd", err: killErr}
	}
	if c.copied != nil {
		select {
		case copyErr := <-c.copied:
			if err == nil && !killed {
				err = copyErr
			}
		default:
			// A process the command left behind may hold the pipes open.
			closeAll(c.parentPipes)
			<-c.copied
			if err == nil {
				err = exec.ErrWaitDelay
			}
		}
		c.copied = nil
	}
	result <- watchResult{err: err}
}

// Wait waits for the command to exit and for the copying to and from it to
// end; see exec.Cmd.Wait.
func (c *Cmd) Wait() error {
	if !c.spawned {
		err := c.Cmd.Wait()
		closeAll(c.parentPipes)
		c.parentPipes = nil
		return err
	}
	if c.Process == nil {
		return errors.New("exec: not started")
	}
	if c.ProcessState != nil {
		return errors.New("exec: Wait was already called")
	}

	state, err := c.Process.Wait()
	if err == nil && !state.Success() {
		err = &exec.ExitError{ProcessState: state}
	}
	c.ProcessState = state

	var timer *time.Timer
	if c.watched != nil {
		watch := <-c.watched
		timer = watch.timer
		if err == nil && watch.err != nil {
			err = watch.err
		}
	}
	if copyErr := c.awaitCopiers(timer); err == nil {
		err = copyErr
	}
	closeAll(c.parentPipes)
	c.parentPipes = nil
	return err
}

// awaitCopiers waits for the copying goroutines; once WaitDelay passes, it
// closes their pipes and reports exec.ErrWaitDelay.
func (c *Cmd) awaitCopiers(timer *time.Timer) error {
	defer func() {
		if timer != nil {
			timer.Stop()
		}
		c.copied = nil
	}()
	if c.copied == nil {
		return nil
	}
	if timer == nil {
		if c.WaitDelay == 0 {
			return <-c.copied
		}
		select {
		case err := <-c.copied:
			return err
		default:
		}
		timer = time.NewTimer(c.WaitDelay)
	}
	select {
	case <-timer.C:
		closeAll(c.parentPipes)
		<-c.copied // its error is likely the closing's
		return exec.ErrWaitDelay
	case err := <-c.copied:
		return err
	}
}

// Output runs the command and returns its standard output; see
// exec.Cmd.Output.
func (c *Cmd) Output() ([]byte, error) {
	if c.Stdout != nil {
		return nil, errors.New("exec: Stdout already set")
	}
	var stdout bytes.Buffer
	c.Stdout = &stdout
	captureErr := c.Stderr == nil
	if captureErr {
		c.Stderr = &prefixSuffixSaver{N: 32 << 10}
	}
	err := c.Run()
	if err != nil && captureErr {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitErr.Stderr = c.Stderr.(*prefixSuffixSaver).Bytes()
		}
	}
	return stdout.Bytes(), err
}

// CombinedOutput runs the command and returns its standard output and
// standard error together.
func (c *Cmd) CombinedOutput() ([]byte, error) {
	if c.Stdout != nil {
		return nil, errors.New("exec: Stdout already set")
	}
	if c.Stderr != nil {
		return nil, errors.New("exec: Stderr already set")
	}
	var output bytes.Buffer
	c.Stdout = &output
	c.Stderr = &output
	err := c.Run()
	return output.Bytes(), err
}

// StdinPipe returns a pipe to the command's standard input; Wait closes it
// once the command exits.
func (c *Cmd) StdinPipe() (io.WriteCloser, error) {
	if c.Stdin != nil {
		return nil, errors.New("exec: Stdin already set")
	}
	if c.Process != nil {
		return nil, errors.New("exec: StdinPipe after process started")
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	c.Stdin = reader
	c.childFiles = append(c.childFiles, reader)
	c.parentPipes = append(c.parentPipes, writer)
	return writer, nil
}

// StdoutPipe returns a pipe from the command's standard output; Wait
// closes it once the command exits, so read it all before calling Wait.
func (c *Cmd) StdoutPipe() (io.ReadCloser, error) {
	if c.Stdout != nil {
		return nil, errors.New("exec: Stdout already set")
	}
	if c.Process != nil {
		return nil, errors.New("exec: StdoutPipe after process started")
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	c.Stdout = writer
	c.childFiles = append(c.childFiles, writer)
	c.parentPipes = append(c.parentPipes, reader)
	return reader, nil
}

// StderrPipe returns a pipe from the command's standard error; Wait closes
// it once the command exits, so read it all before calling Wait.
func (c *Cmd) StderrPipe() (io.ReadCloser, error) {
	if c.Stderr != nil {
		return nil, errors.New("exec: Stderr already set")
	}
	if c.Process != nil {
		return nil, errors.New("exec: StderrPipe after process started")
	}
	reader, writer, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	c.Stderr = writer
	c.childFiles = append(c.childFiles, writer)
	c.parentPipes = append(c.parentPipes, reader)
	return reader, nil
}

func closeAll(closers []io.Closer) {
	for _, closer := range closers {
		closer.Close()
	}
}

// interfaceEqual compares two interfaces without panicking on values that
// cannot be compared.
func interfaceEqual(a, b any) bool {
	defer func() { recover() }()
	return a == b
}

type wrappedError struct {
	prefix string
	err    error
}

func (w wrappedError) Error() string { return w.prefix + ": " + w.err.Error() }

func (w wrappedError) Unwrap() error { return w.err }

// prefixSuffixSaver keeps the first and the last N bytes written to it, as
// os/exec's does for the standard error that Output puts in its ExitError.
type prefixSuffixSaver struct {
	N         int
	prefix    []byte
	suffix    []byte // a ring once len(suffix) == N
	suffixOff int
	skipped   int64
}

func (w *prefixSuffixSaver) Write(p []byte) (int, error) {
	written := len(p)
	p = w.fill(&w.prefix, p)
	if overage := len(p) - w.N; overage > 0 {
		p = p[overage:]
		w.skipped += int64(overage)
	}
	p = w.fill(&w.suffix, p)
	for len(p) > 0 {
		n := copy(w.suffix[w.suffixOff:], p)
		p = p[n:]
		w.skipped += int64(n)
		w.suffixOff += n
		if w.suffixOff == w.N {
			w.suffixOff = 0
		}
	}
	return written, nil
}

func (w *prefixSuffixSaver) fill(dst *[]byte, p []byte) []byte {
	if remain := w.N - len(*dst); remain > 0 {
		add := min(len(p), remain)
		*dst = append(*dst, p[:add]...)
		p = p[add:]
	}
	return p
}

func (w *prefixSuffixSaver) Bytes() []byte {
	if w.suffix == nil {
		return w.prefix
	}
	if w.skipped == 0 {
		return append(w.prefix, w.suffix...)
	}
	var buffer bytes.Buffer
	buffer.Grow(len(w.prefix) + len(w.suffix) + 50)
	buffer.Write(w.prefix)
	buffer.WriteString("\n... omitting ")
	buffer.WriteString(strconv.FormatInt(w.skipped, 10))
	buffer.WriteString(" bytes ...\n")
	buffer.Write(w.suffix[w.suffixOff:])
	buffer.Write(w.suffix[:w.suffixOff])
	return buffer.Bytes()
}
