package primitives

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
)

const (
	PrimitiveEventProcessStarted      PrimitiveEventType = "process.started"
	PrimitiveEventProcessOutput       PrimitiveEventType = "process.output"
	PrimitiveEventProcessStreamFailed PrimitiveEventType = "process.stream_failed"
	PrimitiveEventProcessExited       PrimitiveEventType = "process.exited"

	PrimitiveEventProcessInputWritten     PrimitiveEventType = "process.input_written"
	PrimitiveEventProcessInputWriteFailed PrimitiveEventType = "process.input_write_failed"
	PrimitiveEventProcessInputClosed      PrimitiveEventType = "process.input_closed"
	PrimitiveEventProcessSignaled         PrimitiveEventType = "process.signaled"

	ProcessOutputChunkSize = 32 * 1024

	processTerminationGracePeriod = 5 * time.Second
)

var errProcessDone = errors.New("process invocation is done")

type ProcessStartRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
	// Path must be absolute.
	Path        string
	Arguments   []string
	Directory   string
	Environment []string
	Pipes       ProcessPipeSet
	// Output capture paths must be absolute, name existing regular files, and
	// are truncated before start. Each is mutually exclusive with its pipe.
	StdoutPath string
	StderrPath string
}

type ProcessPipeSet uint8

const (
	ProcessPipeStdin ProcessPipeSet = 1 << iota
	ProcessPipeStdout
	ProcessPipeStderr

	ProcessPipeAll = ProcessPipeStdin | ProcessPipeStdout | ProcessPipeStderr
)

type ProcessStartedResult struct {
	PID int
}

type ProcessStream int

const (
	ProcessStdout ProcessStream = 1
	ProcessStderr ProcessStream = 2
)

type ProcessOutputResult struct {
	Stream ProcessStream
	Offset int64
	Data   []byte
}

type ProcessStreamFailureResult struct {
	Stream ProcessStream
	Error  string
}

type ProcessExitResult struct {
	ExitCode int
	Signal   syscall.Signal
}

type ProcessWriteRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
	Data          []byte
}

type ProcessWriteResult struct {
	Count int
}

type ProcessWriteFailureResult struct {
	Count int
	Error string
}

type ProcessCloseInputRequest struct {
	Source        SourceID
	CorrelationID CorrelationID
}

type ProcessSignalRequest struct {
	Source              SourceID
	CorrelationID       CorrelationID
	Signal              syscall.Signal
	PropagateToChildren bool
}

type ProcessInvocation struct {
	ctx      context.Context
	ready    chan struct{}
	process  *os.Process
	stdin    *os.File
	startErr error
}

func StartProcess(
	ctx context.Context,
	request ProcessStartRequest,
	events chan<- PrimitiveEvent,
) *ProcessInvocation {
	invocation := &ProcessInvocation{
		ctx:   ctx,
		ready: make(chan struct{}),
	}
	go runProcess(ctx, request, invocation, events)
	return invocation
}

// WriteInput honors cancellation until the pipe write starts. Once started, the write owns
// its data until it completes or the process/input lifecycle interrupts the pipe. Concurrent
// writes may interleave; callers that require ordering dispatch the next write after its event.
func (process *ProcessInvocation) WriteInput(
	ctx context.Context,
	request ProcessWriteRequest,
	events chan<- PrimitiveEvent,
) {
	go process.writeInput(ctx, request, events)
}

func (process *ProcessInvocation) writeInput(
	ctx context.Context,
	request ProcessWriteRequest,
	events chan<- PrimitiveEvent,
) {
	if err := process.prepareControl(ctx, false); err != nil {
		if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			events <- processCanceled(request.Source, request.CorrelationID)
		} else {
			events <- processInputWriteFailure(request, 0, err)
		}
		return
	}
	if process.stdin == nil {
		events <- processInputWriteFailure(request, 0, errors.New("stdin pipe is unavailable"))
		return
	}
	count, err := process.stdin.Write(request.Data)
	if err == nil {
		events <- PrimitiveEvent{
			Type:          PrimitiveEventProcessInputWritten,
			Source:        request.Source,
			CorrelationID: request.CorrelationID,
			Result:        ProcessWriteResult{Count: count},
		}
		return
	}
	events <- processInputWriteFailure(request, count, err)
}

func processInputWriteFailure(
	request ProcessWriteRequest,
	count int,
	err error,
) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventProcessInputWriteFailed,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: ProcessWriteFailureResult{
			Count: count,
			Error: fmt.Errorf("write process input: %w", err).Error(),
		},
	}
}

// CloseInput is idempotent and interrupts any active WriteInput invocation.
func (process *ProcessInvocation) CloseInput(
	ctx context.Context,
	request ProcessCloseInputRequest,
	events chan<- PrimitiveEvent,
) {
	go process.closeInput(ctx, request, events)
}

func (process *ProcessInvocation) closeInput(
	ctx context.Context,
	request ProcessCloseInputRequest,
	events chan<- PrimitiveEvent,
) {
	if err := process.prepareControl(ctx, false); err != nil {
		events <- processControlFailure(request.Source, request.CorrelationID, "close process input", err)
		return
	}
	if process.stdin == nil {
		events <- processInputClosed(request)
		return
	}
	if err := normalizeProcessCloseError(process.stdin.Close()); err != nil {
		events <- processFailure(
			request.Source,
			request.CorrelationID,
			fmt.Errorf("close process input: %w", err),
		)
		return
	}
	events <- processInputClosed(request)
}

func processInputClosed(request ProcessCloseInputRequest) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventProcessInputClosed,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
	}
}

func (process *ProcessInvocation) Signal(
	ctx context.Context,
	request ProcessSignalRequest,
	events chan<- PrimitiveEvent,
) {
	go process.signal(ctx, request, events)
}

func (process *ProcessInvocation) signal(
	ctx context.Context,
	request ProcessSignalRequest,
	events chan<- PrimitiveEvent,
) {
	if err := process.prepareControl(ctx, request.PropagateToChildren); err != nil {
		events <- processControlFailure(request.Source, request.CorrelationID, "signal process", err)
		return
	}
	var err error
	if request.PropagateToChildren {
		err = syscall.Kill(-process.process.Pid, request.Signal)
	} else {
		err = process.process.Signal(request.Signal)
	}
	if err != nil {
		events <- processFailure(
			request.Source,
			request.CorrelationID,
			fmt.Errorf("signal process: %w", err),
		)
		return
	}
	events <- PrimitiveEvent{
		Type:          PrimitiveEventProcessSignaled,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
	}
}

func (process *ProcessInvocation) prepareControl(
	ctx context.Context,
	processGroup bool,
) error {
	select {
	case <-process.ready:
	case <-ctx.Done():
		return ctx.Err()
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if process.startErr != nil {
		// A canceled start does not cancel this independently owned control invocation.
		return fmt.Errorf("%w: %v", errProcessDone, process.startErr)
	}
	if process.ctx.Err() != nil {
		return errProcessDone
	}
	if processGroup {
		exists, err := processGroupExists(process.process.Pid)
		if err != nil {
			return fmt.Errorf("inspect process group: %w", err)
		}
		if !exists {
			return errProcessDone
		}
		return nil
	}
	if processWaitCompleted(process.process) {
		return errProcessDone
	}
	return nil
}

func processControlFailure(
	source SourceID,
	correlationID CorrelationID,
	action string,
	err error,
) PrimitiveEvent {
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		return processCanceled(source, correlationID)
	}
	return processFailure(source, correlationID, fmt.Errorf("%s: %w", action, err))
}

func runProcess(
	ctx context.Context,
	request ProcessStartRequest,
	process *ProcessInvocation,
	events chan<- PrimitiveEvent,
) {
	if err := validateProcessStartRequest(request); err != nil {
		process.startErr = err
		close(process.ready)
		sendProcessTerminalEvent(events, processFailure(request.Source, request.CorrelationID, err))
		return
	}
	if ctx.Err() != nil {
		process.startErr = ctx.Err()
		close(process.ready)
		sendProcessTerminalEvent(events, processCanceled(request.Source, request.CorrelationID))
		return
	}

	command, pipes, err := prepareProcess(request)
	if err != nil {
		process.startErr = err
		close(process.ready)
		sendProcessTerminalEvent(events, processFailure(request.Source, request.CorrelationID, err))
		return
	}
	if ctx.Err() != nil {
		cleanupErr := pipes.closeAll()
		process.startErr = ctx.Err()
		close(process.ready)
		sendProcessTerminalEvent(
			events,
			processCompletionEvent(
				request,
				processCanceled(request.Source, request.CorrelationID),
				cleanupErr,
			),
		)
		return
	}
	if err := command.Start(); err != nil {
		startErr := errors.Join(
			fmt.Errorf("start process %q: %w", request.Path, err),
			pipes.closeAll(),
		)
		process.startErr = startErr
		close(process.ready)
		sendProcessTerminalEvent(events, processFailure(request.Source, request.CorrelationID, startErr))
		return
	}

	parentPipes := pipes.parent()
	process.process = command.Process
	process.stdin = parentPipes.stdin
	close(process.ready)
	childPipeCloseErr := pipes.closeChildEnds()

	sendProcessEvent(ctx, events, PrimitiveEvent{
		Type:          PrimitiveEventProcessStarted,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result:        ProcessStartedResult{PID: command.Process.Pid},
	})

	waitCompleted := make(chan error, 1)
	go func() {
		waitCompleted <- command.Wait()
	}()
	var workers sync.WaitGroup
	if parentPipes.stdout != nil {
		stdout := processOutputState{stream: ProcessStdout, reader: parentPipes.stdout}
		workers.Go(func() {
			runProcessOutput(ctx, request, &stdout, events)
		})
	}
	if parentPipes.stderr != nil {
		stderr := processOutputState{stream: ProcessStderr, reader: parentPipes.stderr}
		workers.Go(func() {
			runProcessOutput(ctx, request, &stderr, events)
		})
	}

	waitErr, canceled, completionErr := awaitProcessCompletion(
		ctx,
		command.Process,
		parentPipes,
		waitCompleted,
		processTerminationGracePeriod,
	)
	captureSyncErr := parentPipes.syncCaptures()
	captureCloseErr := parentPipes.closeCaptures()
	stdinCloseErr := closeProcessFile(parentPipes.stdin)
	outputFinishErr := error(nil)
	if !canceled {
		outputFinishErr = parentPipes.finishOutput()
	}
	workers.Wait()
	outputCloseErr := parentPipes.closeOutput()

	if canceled {
		_, waitCompletionErr := processResult(command, waitErr)
		shutdownErr := errors.Join(
			childPipeCloseErr,
			captureSyncErr,
			captureCloseErr,
			stdinCloseErr,
			outputFinishErr,
			outputCloseErr,
			completionErr,
			waitCompletionErr,
		)
		sendProcessTerminalEvent(
			events,
			processCompletionEvent(
				request,
				processCanceled(request.Source, request.CorrelationID),
				shutdownErr,
			),
		)
		return
	}

	exitResult, exitErr := processResult(command, waitErr)
	terminalErr := errors.Join(
		childPipeCloseErr,
		captureSyncErr,
		captureCloseErr,
		stdinCloseErr,
		outputFinishErr,
		outputCloseErr,
		completionErr,
		exitErr,
	)
	sendProcessTerminalEvent(
		events,
		processCompletionEvent(
			request,
			PrimitiveEvent{
				Type:          PrimitiveEventProcessExited,
				Source:        request.Source,
				CorrelationID: request.CorrelationID,
				Result:        exitResult,
			},
			terminalErr,
		),
	)
}

func awaitProcessCompletion(
	ctx context.Context,
	process *os.Process,
	pipes processParentPipes,
	waitCompleted <-chan error,
	gracePeriod time.Duration,
) (error, bool, error) {
	select {
	case waitErr := <-waitCompleted:
		return waitErr, false, terminateProcess(
			process,
			processParentPipes{},
			gracePeriod,
		)
	case <-ctx.Done():
		if processWaitCompleted(process) {
			completionErr := terminateProcess(
				process,
				processParentPipes{},
				gracePeriod,
			)
			return <-waitCompleted, false, completionErr
		}
		cancellationErr := terminateProcess(
			process,
			pipes,
			gracePeriod,
		)
		return <-waitCompleted, true, cancellationErr
	}
}

func processWaitCompleted(process *os.Process) bool {
	if process == nil {
		return false
	}
	// os.Process marks itself done at the OS wait boundary, before exec.Cmd.Wait returns.
	return errors.Is(process.Signal(syscall.Signal(0)), os.ErrProcessDone)
}

type processPipes struct {
	stdinRead     *os.File
	stdinWrite    *os.File
	stdoutRead    *os.File
	stdoutWrite   *os.File
	stdoutCapture *os.File
	stderrRead    *os.File
	stderrWrite   *os.File
	stderrCapture *os.File
}

type processParentPipes struct {
	stdin         *os.File
	stdout        *os.File
	stderr        *os.File
	stdoutCapture *os.File
	stderrCapture *os.File
}

func prepareProcess(request ProcessStartRequest) (*exec.Cmd, processPipes, error) {
	var pipes processPipes
	var stdoutCaptureInfo os.FileInfo
	var stderrCaptureInfo os.FileInfo
	if request.Pipes&ProcessPipeStdin != 0 {
		stdinRead, stdinWrite, err := os.Pipe()
		if err != nil {
			return nil, pipes, fmt.Errorf("start process %q: create stdin pipe: %w", request.Path, err)
		}
		pipes.stdinRead = stdinRead
		pipes.stdinWrite = stdinWrite
	}

	if request.Pipes&ProcessPipeStdout != 0 {
		stdoutRead, stdoutWrite, err := os.Pipe()
		if err != nil {
			return nil, pipes, errors.Join(
				fmt.Errorf("start process %q: create stdout pipe: %w", request.Path, err),
				pipes.closeAll(),
			)
		}
		pipes.stdoutRead = stdoutRead
		pipes.stdoutWrite = stdoutWrite
	} else if request.StdoutPath != "" {
		stdout, info, err := openProcessCapture(request.StdoutPath, "stdout")
		if err != nil {
			return nil, pipes, errors.Join(
				fmt.Errorf("start process %q: %w", request.Path, err),
				pipes.closeAll(),
			)
		}
		pipes.stdoutCapture = stdout
		stdoutCaptureInfo = info
	}

	if request.Pipes&ProcessPipeStderr != 0 {
		stderrRead, stderrWrite, err := os.Pipe()
		if err != nil {
			return nil, pipes, errors.Join(
				fmt.Errorf("start process %q: create stderr pipe: %w", request.Path, err),
				pipes.closeAll(),
			)
		}
		pipes.stderrRead = stderrRead
		pipes.stderrWrite = stderrWrite
	} else if request.StderrPath != "" {
		stderr, info, err := openProcessCapture(request.StderrPath, "stderr")
		if err != nil {
			return nil, pipes, errors.Join(
				fmt.Errorf("start process %q: %w", request.Path, err),
				pipes.closeAll(),
			)
		}
		pipes.stderrCapture = stderr
		stderrCaptureInfo = info
	}

	if stdoutCaptureInfo != nil && stderrCaptureInfo != nil &&
		os.SameFile(stdoutCaptureInfo, stderrCaptureInfo) {
		return nil, pipes, errors.Join(
			fmt.Errorf("start process %q: stdout and stderr capture paths identify the same file", request.Path),
			pipes.closeAll(),
		)
	}
	for _, capture := range []struct {
		file   *os.File
		path   string
		stream string
	}{
		{file: pipes.stdoutCapture, path: request.StdoutPath, stream: "stdout"},
		{file: pipes.stderrCapture, path: request.StderrPath, stream: "stderr"},
	} {
		if err := truncateProcessCapture(capture.file, capture.path, capture.stream); err != nil {
			return nil, pipes, errors.Join(
				fmt.Errorf("start process %q: %w", request.Path, err),
				pipes.closeAll(),
			)
		}
	}

	command := exec.Command(request.Path, request.Arguments...)
	command.Dir = request.Directory
	command.Env = request.Environment
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if pipes.stdinRead != nil {
		command.Stdin = pipes.stdinRead
	}
	if pipes.stdoutWrite != nil {
		command.Stdout = pipes.stdoutWrite
	} else if pipes.stdoutCapture != nil {
		command.Stdout = pipes.stdoutCapture
	}
	if pipes.stderrWrite != nil {
		command.Stderr = pipes.stderrWrite
	} else if pipes.stderrCapture != nil {
		command.Stderr = pipes.stderrCapture
	}
	return command, pipes, nil
}

func openProcessCapture(path string, stream string) (*os.File, os.FileInfo, error) {
	var descriptor int
	err := retryEINTR(func() error {
		var openErr error
		descriptor, openErr = unix.Open(
			path,
			unix.O_WRONLY|unix.O_NONBLOCK|unix.O_NOFOLLOW|unix.O_CLOEXEC,
			0,
		)
		return openErr
	})
	if err != nil {
		return nil, nil, fmt.Errorf("open process %s capture %q: %w", stream, path, err)
	}

	file := os.NewFile(uintptr(descriptor), path)
	info, err := file.Stat()
	if err != nil {
		return nil, nil, errors.Join(
			fmt.Errorf("inspect process %s capture %q: %w", stream, path, err),
			closeProcessFile(file),
		)
	}
	if !info.Mode().IsRegular() {
		return nil, nil, errors.Join(
			fmt.Errorf("open process %s capture %q: path is not a regular file", stream, path),
			closeProcessFile(file),
		)
	}
	if err := unix.SetNonblock(descriptor, false); err != nil {
		return nil, nil, errors.Join(
			fmt.Errorf("open process %s capture %q: set blocking: %w", stream, path, err),
			closeProcessFile(file),
		)
	}
	return file, info, nil
}

func truncateProcessCapture(file *os.File, path string, stream string) error {
	if file == nil {
		return nil
	}
	if err := retryEINTR(func() error { return unix.Ftruncate(int(file.Fd()), 0) }); err != nil {
		return errors.Join(
			fmt.Errorf("truncate process %s capture %q: %w", stream, path, err),
			closeProcessFile(file),
		)
	}
	return nil
}

func (pipes processPipes) parent() processParentPipes {
	return processParentPipes{
		stdin:         pipes.stdinWrite,
		stdout:        pipes.stdoutRead,
		stderr:        pipes.stderrRead,
		stdoutCapture: pipes.stdoutCapture,
		stderrCapture: pipes.stderrCapture,
	}
}

func (pipes processPipes) closeChildEnds() error {
	return errors.Join(
		closeProcessFile(pipes.stdinRead),
		closeProcessFile(pipes.stdoutWrite),
		closeProcessFile(pipes.stderrWrite),
	)
}

func (pipes processPipes) closeAll() error {
	return errors.Join(
		closeProcessFile(pipes.stdinRead),
		closeProcessFile(pipes.stdinWrite),
		closeProcessFile(pipes.stdoutRead),
		closeProcessFile(pipes.stdoutWrite),
		closeProcessFile(pipes.stdoutCapture),
		closeProcessFile(pipes.stderrRead),
		closeProcessFile(pipes.stderrWrite),
		closeProcessFile(pipes.stderrCapture),
	)
}

func (pipes processParentPipes) closeAll() error {
	return errors.Join(
		closeProcessFile(pipes.stdin),
		closeProcessFile(pipes.stdout),
		closeProcessFile(pipes.stderr),
	)
}

func (pipes processParentPipes) syncCaptures() error {
	return errors.Join(
		syncProcessCapture(pipes.stdoutCapture, "stdout"),
		syncProcessCapture(pipes.stderrCapture, "stderr"),
	)
}

func syncProcessCapture(file *os.File, stream string) error {
	if file == nil {
		return nil
	}
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync process %s capture: %w", stream, err)
	}
	return nil
}

func (pipes processParentPipes) closeCaptures() error {
	return errors.Join(
		closeProcessFile(pipes.stdoutCapture),
		closeProcessFile(pipes.stderrCapture),
	)
}

func (pipes processParentPipes) finishOutput() error {
	// Wake reads held open by descendants; each reader snapshots and drains the bytes already buffered.
	deadline := time.Now()
	return errors.Join(
		finishProcessOutput(pipes.stdout, deadline),
		finishProcessOutput(pipes.stderr, deadline),
	)
}

func finishProcessOutput(file *os.File, deadline time.Time) error {
	if file == nil {
		return nil
	}
	deadlineErr := file.SetReadDeadline(deadline)
	if deadlineErr == nil {
		return nil
	}
	return errors.Join(deadlineErr, closeProcessFile(file))
}

func (pipes processParentPipes) closeOutput() error {
	return errors.Join(closeProcessFile(pipes.stdout), closeProcessFile(pipes.stderr))
}

func terminateProcess(
	process *os.Process,
	pipes processParentPipes,
	gracePeriod time.Duration,
) error {
	termErr := signalProcessInvocation(process, syscall.SIGTERM)
	exited, waitErr := waitForProcessInvocation(process, gracePeriod)
	// Unlike Darwin's exiting-member check, this forgives EPERM only after confirmed completion.
	if exited && errors.Is(termErr, syscall.EPERM) {
		termErr = nil
	}
	var killErr error
	if !exited {
		killErr = signalProcessInvocation(process, syscall.SIGKILL)
	}
	return errors.Join(termErr, waitErr, killErr, pipes.closeAll())
}

func signalProcessInvocation(process *os.Process, signal syscall.Signal) error {
	err := syscall.Kill(-process.Pid, signal)
	err = normalizeProcessGroupSignalError(process.Pid, err)
	if err == nil {
		return nil
	}
	if !errors.Is(err, syscall.ESRCH) {
		return err
	}

	err = process.Signal(signal)
	if errors.Is(err, os.ErrProcessDone) || errors.Is(err, syscall.ESRCH) {
		return nil
	}
	return err
}

func waitForProcessInvocation(
	process *os.Process,
	gracePeriod time.Duration,
) (bool, error) {
	deadline := time.Now().Add(gracePeriod)
	delay := time.Millisecond
	for {
		groupExists, err := processGroupExists(process.Pid)
		if err != nil {
			return false, err
		}
		if !groupExists && processWaitCompleted(process) {
			return true, nil
		}

		remaining := time.Until(deadline)
		if remaining <= 0 {
			return false, nil
		}
		timer := time.NewTimer(min(delay, remaining))
		<-timer.C
		delay = min(delay*2, 50*time.Millisecond)
	}
}

func processGroupExists(processGroupID int) (bool, error) {
	err := syscall.Kill(-processGroupID, 0)
	if err == nil || errors.Is(err, syscall.EPERM) {
		return true, nil
	}
	if errors.Is(err, syscall.ESRCH) {
		return false, nil
	}
	return false, err
}

func closeProcessFile(file *os.File) error {
	if file == nil {
		return nil
	}
	return normalizeProcessCloseError(file.Close())
}

func normalizeProcessCloseError(err error) error {
	if errors.Is(err, os.ErrClosed) {
		return nil
	}
	return err
}

type processOutputState struct {
	stream ProcessStream
	reader *os.File
}

func runProcessOutput(
	ctx context.Context,
	request ProcessStartRequest,
	output *processOutputState,
	events chan<- PrimitiveEvent,
) {
	err := streamProcessOutput(ctx, request, output, events)
	if err != nil {
		sendProcessEvent(ctx, events, processStreamFailure(request, output.stream, err))
	}
}

func streamProcessOutput(
	ctx context.Context,
	request ProcessStartRequest,
	output *processOutputState,
	events chan<- PrimitiveEvent,
) error {
	buffer := make([]byte, ProcessOutputChunkSize)
	var offset int64
	for {
		count, readErr := output.reader.Read(buffer)
		if count > 0 {
			sendProcessOutput(ctx, request, output.stream, offset, buffer[:count], events)
			offset += int64(count)
		}

		if errors.Is(readErr, io.EOF) {
			return nil
		}
		if os.IsTimeout(readErr) {
			return drainProcessOutput(
				ctx,
				request,
				output.stream,
				output.reader,
				&offset,
				events,
			)
		}
		if readErr != nil {
			if ctx.Err() != nil && errors.Is(readErr, os.ErrClosed) {
				return nil
			}
			return fmt.Errorf("read process %s: %w", processStreamName(output.stream), readErr)
		}
	}
}

func drainProcessOutput(
	ctx context.Context,
	request ProcessStartRequest,
	stream ProcessStream,
	reader *os.File,
	offset *int64,
	events chan<- PrimitiveEvent,
) error {
	fileDescriptor := int(reader.Fd())
	if err := unix.SetNonblock(fileDescriptor, true); err != nil {
		return fmt.Errorf("drain process %s: set nonblocking: %w", processStreamName(stream), err)
	}

	buffer := make([]byte, ProcessOutputChunkSize)
	for {
		count, readErr := unix.Read(fileDescriptor, buffer)
		if count > 0 {
			if !sendProcessOutput(ctx, request, stream, *offset, buffer[:count], events) {
				return nil
			}
			*offset += int64(count)
		}
		if errors.Is(readErr, unix.EINTR) {
			continue
		}
		if errors.Is(readErr, unix.EAGAIN) || errors.Is(readErr, unix.EWOULDBLOCK) {
			return nil
		}
		if readErr != nil {
			return fmt.Errorf("drain process %s: %w", processStreamName(stream), readErr)
		}
		if count == 0 {
			return nil
		}
	}
}

func sendProcessOutput(
	ctx context.Context,
	request ProcessStartRequest,
	stream ProcessStream,
	offset int64,
	data []byte,
	events chan<- PrimitiveEvent,
) bool {
	return sendProcessEvent(ctx, events, PrimitiveEvent{
		Type:          PrimitiveEventProcessOutput,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: ProcessOutputResult{
			Stream: stream,
			Offset: offset,
			Data:   append([]byte(nil), data...),
		},
	})
}

func processResult(command *exec.Cmd, waitErr error) (ProcessExitResult, error) {
	if command.ProcessState == nil {
		return ProcessExitResult{}, fmt.Errorf("wait for process %q: %w", command.Path, waitErr)
	}
	var exitError *exec.ExitError
	if waitErr != nil && !errors.As(waitErr, &exitError) {
		return ProcessExitResult{}, fmt.Errorf("wait for process %q: %w", command.Path, waitErr)
	}

	result := ProcessExitResult{ExitCode: command.ProcessState.ExitCode()}
	if status, ok := command.ProcessState.Sys().(syscall.WaitStatus); ok && status.Signaled() {
		result.Signal = status.Signal()
	}
	return result, nil
}

func processStreamName(stream ProcessStream) string {
	if stream == ProcessStderr {
		return "stderr"
	}
	return "stdout"
}

func validateProcessStartRequest(request ProcessStartRequest) error {
	if request.Path == "" {
		return errors.New("start process: path must be set")
	}
	if !filepath.IsAbs(request.Path) {
		return errors.New("start process: path must be absolute")
	}
	if request.Pipes&^ProcessPipeAll != 0 {
		return fmt.Errorf("start process: unsupported pipe selection %#x", request.Pipes)
	}
	for _, output := range []struct {
		name string
		path string
		pipe ProcessPipeSet
	}{
		{name: "stdout", path: request.StdoutPath, pipe: ProcessPipeStdout},
		{name: "stderr", path: request.StderrPath, pipe: ProcessPipeStderr},
	} {
		if output.path == "" {
			continue
		}
		if !filepath.IsAbs(output.path) {
			return fmt.Errorf("start process: %s capture path must be absolute", output.name)
		}
		if request.Pipes&output.pipe != 0 {
			return fmt.Errorf("start process: %s cannot use both a pipe and a capture path", output.name)
		}
	}
	if request.StdoutPath != "" && request.StderrPath != "" &&
		filepath.Clean(request.StdoutPath) == filepath.Clean(request.StderrPath) {
		return errors.New("start process: stdout and stderr capture paths must differ")
	}
	return nil
}

func processStreamFailure(
	request ProcessStartRequest,
	stream ProcessStream,
	err error,
) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventProcessStreamFailed,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: ProcessStreamFailureResult{
			Stream: stream,
			Error:  err.Error(),
		},
	}
}

func sendProcessEvent(
	ctx context.Context,
	events chan<- PrimitiveEvent,
	event PrimitiveEvent,
) bool {
	if ctx.Err() != nil {
		return false
	}
	select {
	case events <- event:
		return true
	case <-ctx.Done():
		return false
	}
}

func sendProcessTerminalEvent(events chan<- PrimitiveEvent, event PrimitiveEvent) {
	events <- event
}

func processCompletionEvent(
	request ProcessStartRequest,
	event PrimitiveEvent,
	err error,
) PrimitiveEvent {
	if err != nil {
		return processFailure(request.Source, request.CorrelationID, err)
	}
	return event
}

func processFailure(
	source SourceID,
	correlationID CorrelationID,
	err error,
) PrimitiveEvent {
	return primitiveFailure(source, correlationID, err)
}

func processCanceled(source SourceID, correlationID CorrelationID) PrimitiveEvent {
	return primitiveCanceled(source, correlationID)
}
