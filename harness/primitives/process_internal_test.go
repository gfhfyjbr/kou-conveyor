package primitives

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"golang.org/x/sys/unix"
)

func TestDrainProcessOutput(t *testing.T) {
	reader, writer := processTestPipe(t)
	data := []byte("buffered process output")
	if count, err := writer.Write(data); err != nil || count != len(data) {
		t.Fatalf("write buffered output = (%d, %v), want (%d, nil)", count, err, len(data))
	}

	events := make(chan PrimitiveEvent, 1)
	offset := int64(17)
	request := ProcessStartRequest{Source: "operation-1", CorrelationID: "process-1"}
	if err := drainProcessOutput(
		t.Context(),
		request,
		ProcessStderr,
		reader,
		&offset,
		events,
	); err != nil {
		t.Fatalf("drain output: %v", err)
	}

	event := <-events
	if event.Type != PrimitiveEventProcessOutput ||
		event.Source != request.Source ||
		event.CorrelationID != request.CorrelationID {
		t.Fatalf("event = %#v", event)
	}
	result := event.Result.(ProcessOutputResult)
	if result.Stream != ProcessStderr || result.Offset != 17 || string(result.Data) != string(data) {
		t.Fatalf("result = %#v", result)
	}
	if offset != 17+int64(len(data)) {
		t.Fatalf("offset = %d", offset)
	}
}

func TestDrainProcessOutputHasNoByteLimit(t *testing.T) {
	data := make([]byte, 2*1024*1024+1)
	path := filepath.Join(t.TempDir(), "output")
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("write output: %v", err)
	}
	reader, err := os.Open(path)
	if err != nil {
		t.Fatalf("open output: %v", err)
	}
	t.Cleanup(func() { closeProcessTestFile(t, reader) })

	events := make(chan PrimitiveEvent, len(data)/ProcessOutputChunkSize+1)
	offset := int64(0)
	if err := drainProcessOutput(
		t.Context(),
		ProcessStartRequest{},
		ProcessStdout,
		reader,
		&offset,
		events,
	); err != nil {
		t.Fatalf("drain output: %v", err)
	}
	if offset != int64(len(data)) {
		t.Fatalf("offset = %d, want %d", offset, len(data))
	}
}

func TestDrainProcessOutputStopsWhenPipeHasNoBufferedData(t *testing.T) {
	reader, _ := processTestPipe(t)
	offset := int64(0)
	if err := drainProcessOutput(
		t.Context(),
		ProcessStartRequest{},
		ProcessStdout,
		reader,
		&offset,
		make(chan PrimitiveEvent, 1),
	); err != nil {
		t.Fatalf("drain empty pipe: %v", err)
	}
}

func TestDrainProcessOutputStopsAtEOF(t *testing.T) {
	reader, writer := processTestPipe(t)
	closeProcessTestFile(t, writer)
	offset := int64(0)
	if err := drainProcessOutput(
		t.Context(),
		ProcessStartRequest{},
		ProcessStdout,
		reader,
		&offset,
		make(chan PrimitiveEvent, 1),
	); err != nil {
		t.Fatalf("drain closed pipe: %v", err)
	}
}

func TestDrainProcessOutputHonorsCancellation(t *testing.T) {
	reader, writer := processTestPipe(t)
	if _, err := writer.Write([]byte("output")); err != nil {
		t.Fatalf("write buffered output: %v", err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	offset := int64(0)
	err := drainProcessOutput(
		ctx,
		ProcessStartRequest{},
		ProcessStdout,
		reader,
		&offset,
		make(chan PrimitiveEvent, 1),
	)
	if err != nil {
		t.Fatalf("canceled drain: %v", err)
	}
}

func TestDrainProcessOutputReportsInvalidReader(t *testing.T) {
	reader, _ := processTestPipe(t)
	closeProcessTestFile(t, reader)
	offset := int64(0)
	err := drainProcessOutput(
		t.Context(),
		ProcessStartRequest{},
		ProcessStdout,
		reader,
		&offset,
		make(chan PrimitiveEvent, 1),
	)
	if err == nil || !strings.Contains(err.Error(), "set nonblocking") {
		t.Fatalf("error = %v", err)
	}
}

func TestStreamProcessOutputDrainsAfterDeadline(t *testing.T) {
	reader, writer := processTestPipe(t)
	data := []byte("available before the direct process exited")
	if count, err := writer.Write(data); err != nil || count != len(data) {
		t.Fatalf("write buffered output = (%d, %v), want (%d, nil)", count, err, len(data))
	}
	if err := reader.SetReadDeadline(time.Now()); err != nil {
		t.Fatalf("set read deadline: %v", err)
	}

	events := make(chan PrimitiveEvent, 1)
	request := ProcessStartRequest{Source: "operation-1", CorrelationID: "process-1"}
	output := processOutputState{stream: ProcessStdout, reader: reader}
	if err := streamProcessOutput(t.Context(), request, &output, events); err != nil {
		t.Fatalf("stream output: %v", err)
	}
	result := (<-events).Result.(ProcessOutputResult)
	if result.Offset != 0 || string(result.Data) != string(data) {
		t.Fatalf("result = %#v", result)
	}
}

func TestStreamProcessOutputReportsReadFailure(t *testing.T) {
	reader, _ := processTestPipe(t)
	closeProcessTestFile(t, reader)
	request := ProcessStartRequest{Source: "operation-1", CorrelationID: "process-1"}
	output := processOutputState{stream: ProcessStderr, reader: reader}
	events := make(chan PrimitiveEvent, 1)
	runProcessOutput(
		t.Context(),
		request,
		&output,
		events,
	)

	event := <-events
	if event.Type != PrimitiveEventProcessStreamFailed {
		t.Fatalf("event = %#v", event)
	}
	result := event.Result.(ProcessStreamFailureResult)
	if result.Stream != ProcessStderr || !strings.Contains(result.Error, "read process stderr") {
		t.Fatalf("result = %#v", result)
	}
}

func TestStreamProcessOutputPreservesFailureDuringCancellation(t *testing.T) {
	reader, err := os.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open directory: %v", err)
	}
	t.Cleanup(func() { closeProcessTestFile(t, reader) })
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	output := processOutputState{stream: ProcessStdout, reader: reader}
	err = streamProcessOutput(
		ctx,
		ProcessStartRequest{},
		&output,
		make(chan PrimitiveEvent, 1),
	)
	if err == nil || !strings.Contains(err.Error(), "read process stdout") {
		t.Fatalf("error = %v", err)
	}
}

func TestStreamProcessOutputHonorsCancellation(t *testing.T) {
	reader, writer := processTestPipe(t)
	if _, err := writer.Write([]byte("output")); err != nil {
		t.Fatalf("write output: %v", err)
	}
	closeProcessTestFile(t, writer)
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	output := processOutputState{stream: ProcessStdout, reader: reader}
	events := make(chan PrimitiveEvent, 1)
	err := streamProcessOutput(
		ctx,
		ProcessStartRequest{},
		&output,
		events,
	)
	if err != nil {
		t.Fatalf("canceled stream: %v", err)
	}
	if len(events) != 0 {
		t.Fatal("output event delivered after cancellation")
	}
}

func TestStreamProcessOutputTreatsCanceledCloseAsCompletion(t *testing.T) {
	reader, _ := processTestPipe(t)
	closeProcessTestFile(t, reader)
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	output := processOutputState{stream: ProcessStdout, reader: reader}
	if err := streamProcessOutput(
		ctx,
		ProcessStartRequest{},
		&output,
		make(chan PrimitiveEvent, 1),
	); err != nil {
		t.Fatalf("canceled stream: %v", err)
	}
}

func TestFinishProcessOutputClosesFileWhenDeadlineFails(t *testing.T) {
	directory, err := os.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open directory: %v", err)
	}
	t.Cleanup(func() { closeProcessTestFile(t, directory) })

	err = finishProcessOutput(directory, time.Now())
	if err == nil {
		t.Fatal("deadline unexpectedly supported")
	}
	if _, readErr := directory.Read(make([]byte, 1)); !errors.Is(readErr, os.ErrClosed) {
		t.Fatalf("read error = %v, want closed file", readErr)
	}
}

func TestDrainProcessOutputReportsReadFailure(t *testing.T) {
	directory, err := os.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open directory: %v", err)
	}
	t.Cleanup(func() { closeProcessTestFile(t, directory) })
	offset := int64(0)
	err = drainProcessOutput(
		t.Context(),
		ProcessStartRequest{},
		ProcessStdout,
		directory,
		&offset,
		make(chan PrimitiveEvent, 1),
	)
	if err == nil || !strings.Contains(err.Error(), "drain process stdout") {
		t.Fatalf("error = %v", err)
	}
}

func TestProcessResultErrors(t *testing.T) {
	if _, err := processResult(&exec.Cmd{Path: "/bin/sh"}, errors.New("wait failed")); err == nil {
		t.Fatal("missing process state accepted")
	}

	command := exec.Command("/bin/sh", "-c", "exit 0")
	if err := command.Run(); err != nil {
		t.Fatalf("run process: %v", err)
	}
	if _, err := processResult(command, errors.New("wait failed")); err == nil {
		t.Fatal("non-exit wait failure accepted")
	}
}

func TestNilProcessHasNotCompleted(t *testing.T) {
	if processWaitCompleted(nil) {
		t.Fatal("nil process reported as completed")
	}
}

func TestProcessControlCancellationWhileWaitingForStart(t *testing.T) {
	baseContext, cancel := context.WithCancel(t.Context())
	ctx := &observedDoneContext{Context: baseContext, checked: make(chan struct{})}
	process := &ProcessInvocation{
		ctx:   t.Context(),
		ready: make(chan struct{}),
	}
	events := make(chan PrimitiveEvent)
	process.Signal(ctx, ProcessSignalRequest{
		Source:        "operation-1",
		CorrelationID: "signal-1",
		Signal:        syscall.SIGTERM,
	}, events)
	<-ctx.checked
	cancel()

	event := <-events
	if event.Type != PrimitiveEventCanceled ||
		event.Source != "operation-1" || event.CorrelationID != "signal-1" {
		t.Fatalf("event = %#v", event)
	}
	assertPrimitiveEventChannelOpen(t, events)
}

func TestProcessControlCancellationAfterStartBecomesReady(t *testing.T) {
	baseContext, cancel := context.WithCancel(t.Context())
	ctx := &cancelOnFirstErrContext{Context: baseContext, cancel: cancel}
	ready := make(chan struct{})
	close(ready)
	process := &ProcessInvocation{
		ctx:   t.Context(),
		ready: ready,
	}
	events := make(chan PrimitiveEvent)
	process.Signal(ctx, ProcessSignalRequest{
		Source:        "operation-1",
		CorrelationID: "signal-1",
		Signal:        syscall.SIGTERM,
	}, events)

	event := <-events
	if event.Type != PrimitiveEventCanceled ||
		event.Source != "operation-1" || event.CorrelationID != "signal-1" {
		t.Fatalf("event = %#v", event)
	}
	assertPrimitiveEventChannelOpen(t, events)
}

func TestRunProcessCanceledBeforeStart(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	request := ProcessStartRequest{
		Source:        "operation-1",
		CorrelationID: "process-1",
		Path:          "/bin/sh",
	}
	events := make(chan PrimitiveEvent, 1)
	invocation := &ProcessInvocation{
		ctx:   ctx,
		ready: make(chan struct{}),
	}
	runProcess(ctx, request, invocation, events)

	event, ok := <-events
	if !ok || event.Type != PrimitiveEventCanceled ||
		event.Source != request.Source || event.CorrelationID != request.CorrelationID {
		t.Fatalf("event = %#v", event)
	}
	assertPrimitiveEventChannelOpen(t, events)
}

func TestRunProcessCanceledAfterPreparingPipes(t *testing.T) {
	baseContext, cancel := context.WithCancel(t.Context())
	ctx := &cancelAfterFirstErrContext{Context: baseContext, cancel: cancel}
	request := ProcessStartRequest{
		Source:        "operation-1",
		CorrelationID: "process-1",
		Path:          "/bin/sh",
		Pipes:         ProcessPipeAll,
	}
	events := make(chan PrimitiveEvent, 1)
	invocation := &ProcessInvocation{
		ctx:   ctx,
		ready: make(chan struct{}),
	}
	runProcess(ctx, request, invocation, events)

	event, ok := <-events
	if !ok || event.Type != PrimitiveEventCanceled || invocation.process != nil {
		t.Fatalf("event = %#v, process = %#v", event, invocation.process)
	}
	assertPrimitiveEventChannelOpen(t, events)
}

func TestPrepareProcessUsesNoPipesForZeroSelection(t *testing.T) {
	command, pipes, err := prepareProcess(ProcessStartRequest{Path: "/usr/bin/true"})
	if err != nil {
		t.Fatalf("prepare process: %v", err)
	}
	if command.Stdin != nil || command.Stdout != nil || command.Stderr != nil {
		t.Fatalf("stdio = (%#v, %#v, %#v)", command.Stdin, command.Stdout, command.Stderr)
	}
	if pipes.stdinRead != nil || pipes.stdinWrite != nil || pipes.stdoutRead != nil ||
		pipes.stdoutWrite != nil || pipes.stdoutCapture != nil || pipes.stderrRead != nil ||
		pipes.stderrWrite != nil || pipes.stderrCapture != nil {
		t.Fatalf("pipes = %#v", pipes)
	}
}

func TestProcessCaptureDescriptorsRemainOpenUntilWaitAndSync(t *testing.T) {
	directory := t.TempDir()
	stdoutPath := filepath.Join(directory, "stdout")
	stderrPath := filepath.Join(directory, "stderr")
	for _, path := range []string{stdoutPath, stderrPath} {
		if err := os.WriteFile(path, nil, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	command, pipes, err := prepareProcess(ProcessStartRequest{
		Path:       "/bin/sh",
		Arguments:  []string{"-c", "printf output; printf error >&2"},
		StdoutPath: stdoutPath,
		StderrPath: stderrPath,
	})
	if err != nil {
		t.Fatal(err)
	}
	started := false
	waited := false
	t.Cleanup(func() {
		if started && !waited {
			_ = command.Process.Kill()
			_ = command.Wait()
		}
		if err := pipes.closeAll(); err != nil {
			t.Error(err)
		}
	})
	parent := pipes.parent()
	if parent.stdoutCapture == nil || parent.stderrCapture == nil ||
		pipes.stdoutWrite != nil || pipes.stderrWrite != nil {
		t.Fatalf("capture descriptors = %#v", parent)
	}
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	started = true
	if err := pipes.closeChildEnds(); err != nil {
		t.Fatal(err)
	}
	if err := command.Wait(); err != nil {
		t.Fatal(err)
	}
	waited = true
	if err := parent.syncCaptures(); err != nil {
		t.Fatal(err)
	}
	if err := parent.closeCaptures(); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]string{stdoutPath: "output", stderrPath: "error"} {
		contents, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if string(contents) != want {
			t.Fatalf("contents of %q = %q, want %q", path, contents, want)
		}
	}
}

func TestSyncProcessCaptureReportsClosedDescriptor(t *testing.T) {
	path := filepath.Join(t.TempDir(), "capture")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	if err := syncProcessCapture(file, "stdout"); err == nil || !strings.Contains(err.Error(), "sync process stdout capture") {
		t.Fatalf("sync error = %v", err)
	}
}

func TestPrepareProcessClosesPipesAfterAllocationFailure(t *testing.T) {
	var originalLimit unix.Rlimit
	if err := unix.Getrlimit(unix.RLIMIT_NOFILE, &originalLimit); err != nil {
		t.Fatalf("get descriptor limit: %v", err)
	}
	limited := originalLimit
	limited.Cur = min(limited.Cur, 64)
	if limited.Cur < 16 {
		t.Skipf("descriptor limit %d is too small for a safe exhaustion test", limited.Cur)
	}
	if err := unix.Setrlimit(unix.RLIMIT_NOFILE, &limited); err != nil {
		t.Fatalf("lower descriptor limit: %v", err)
	}

	var descriptors []*os.File
	t.Cleanup(func() {
		cleanupErr := unix.Setrlimit(unix.RLIMIT_NOFILE, &originalLimit)
		for _, descriptor := range descriptors {
			cleanupErr = errors.Join(cleanupErr, descriptor.Close())
		}
		if cleanupErr != nil {
			t.Errorf("restore descriptor limit: %v", cleanupErr)
		}
	})

	exhaustDescriptors := func() int {
		opened := 0
		for {
			descriptor, err := os.Open(os.DevNull)
			if errors.Is(err, syscall.EMFILE) {
				return opened
			}
			if err != nil {
				t.Fatalf("exhaust descriptors: %v", err)
			}
			descriptors = append(descriptors, descriptor)
			opened++
		}
	}
	releaseDescriptors := func(count int) {
		if count > len(descriptors) {
			t.Fatalf("release %d descriptors from a pool of %d", count, len(descriptors))
		}
		for range count {
			last := len(descriptors) - 1
			if err := descriptors[last].Close(); err != nil {
				t.Fatalf("release descriptor: %v", err)
			}
			descriptors = descriptors[:last]
		}
	}

	if opened := exhaustDescriptors(); opened < 4 {
		t.Skipf("only %d descriptors were available below the reduced limit", opened)
	}
	request := ProcessStartRequest{
		Source:        "operation-1",
		CorrelationID: "process-1",
		Path:          "/bin/sh",
		Pipes:         ProcessPipeAll,
	}
	events := make(chan PrimitiveEvent, 1)
	invocation := &ProcessInvocation{
		ctx:   t.Context(),
		ready: make(chan struct{}),
	}
	runProcess(t.Context(), request, invocation, events)
	if event := <-events; event.Type != PrimitiveEventFailed ||
		!strings.Contains(event.Result.(PrimitiveFailureResult).Error, "create stdin pipe") {
		t.Fatalf("stdin allocation event = %#v", event)
	}

	releaseDescriptors(2)
	if _, _, err := prepareProcess(request); err == nil || !strings.Contains(err.Error(), "create stdout pipe") {
		t.Fatalf("stdout allocation error = %v", err)
	}
	if opened := exhaustDescriptors(); opened != 2 {
		t.Fatalf("stdout failure released %d descriptors, want 2", opened)
	}

	releaseDescriptors(4)
	if _, _, err := prepareProcess(request); err == nil || !strings.Contains(err.Error(), "create stderr pipe") {
		t.Fatalf("stderr allocation error = %v", err)
	}
	if opened := exhaustDescriptors(); opened != 4 {
		t.Fatalf("stderr failure released %d descriptors, want 4", opened)
	}
}

func TestProcessLifecycleHelpers(t *testing.T) {
	if err := closeProcessFile(nil); err != nil {
		t.Fatalf("close nil file: %v", err)
	}
	command := exec.Command("/bin/sh", "-c", "exit 0")
	if err := command.Run(); err != nil {
		t.Fatalf("run process: %v", err)
	}
	if err := terminateProcess(command.Process, processParentPipes{}, 0); err != nil {
		t.Fatalf("terminate completed process: %v", err)
	}
}

func TestTerminateProcessFallsBackToDirectPID(t *testing.T) {
	command := exec.Command("/bin/sh", "-c", "while :; do :; done")
	if err := command.Start(); err != nil {
		t.Fatalf("start process: %v", err)
	}
	waitResult := make(chan error, 1)
	go func() {
		waitResult <- command.Wait()
	}()
	reaped := false
	t.Cleanup(func() {
		if reaped {
			return
		}
		if err := command.Process.Kill(); err != nil && !errors.Is(err, os.ErrProcessDone) {
			t.Errorf("kill process: %v", err)
		}
		if err := <-waitResult; err == nil {
			t.Error("cleanup process exited without a signal")
		}
	})

	if err := terminateProcess(command.Process, processParentPipes{}, time.Second); err != nil {
		t.Fatalf("terminate process: %v", err)
	}
	waitErr := <-waitResult
	reaped = true
	if waitErr == nil {
		t.Fatal("process exited without a signal")
	}
}

func TestSignalProcessInvocationReportsInvalidSignal(t *testing.T) {
	command := exec.Command("/bin/sh", "-c", "exec sleep 30")
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := command.Start(); err != nil {
		t.Fatalf("start process: %v", err)
	}
	reaped := false
	t.Cleanup(func() {
		if reaped {
			return
		}
		if err := command.Process.Kill(); err != nil && !errors.Is(err, os.ErrProcessDone) {
			t.Errorf("kill process: %v", err)
		}
		if err := command.Wait(); err == nil {
			t.Error("cleanup process exited without a signal")
		}
	})

	if err := signalProcessInvocation(command.Process, syscall.Signal(-1)); !errors.Is(err, syscall.EINVAL) {
		t.Fatalf("signal process error = %v, want EINVAL", err)
	}
	if err := command.Process.Kill(); err != nil {
		t.Fatalf("kill process: %v", err)
	}
	if err := command.Wait(); err == nil {
		t.Fatal("process exited without a signal")
	}
	reaped = true
}

func TestTerminateProcessEscalatesAfterGracePeriod(t *testing.T) {
	marker := filepath.Join(t.TempDir(), "ready")
	command := exec.Command(
		"/bin/sh",
		"-c",
		"trap '' TERM; printf ready > \"$1\"; while :; do :; done",
		"sh",
		marker,
	)
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := command.Start(); err != nil {
		t.Fatalf("start process: %v", err)
	}
	waitForInternalFile(t, marker, "ready")

	waitResult := make(chan error, 1)
	go func() {
		waitResult <- command.Wait()
	}()
	reaped := false
	t.Cleanup(func() {
		if reaped {
			return
		}
		if err := command.Process.Kill(); err != nil && !errors.Is(err, os.ErrProcessDone) {
			t.Errorf("kill process: %v", err)
		}
		<-waitResult
	})

	gracePeriod := 50 * time.Millisecond
	startedAt := time.Now()
	if err := terminateProcess(command.Process, processParentPipes{}, gracePeriod); err != nil {
		t.Fatalf("terminate process: %v", err)
	}
	if elapsed := time.Since(startedAt); elapsed < gracePeriod {
		t.Fatalf("termination took %s, want at least %s", elapsed, gracePeriod)
	}
	waitErr := <-waitResult
	reaped = true
	var exitErr *exec.ExitError
	if !errors.As(waitErr, &exitErr) || exitErr.ProcessState.Sys().(syscall.WaitStatus).Signal() != syscall.SIGKILL {
		t.Fatalf("wait error = %v, want SIGKILL", waitErr)
	}
}

func TestTerminateProcessDoesNotRequireKilledProcessToBeReaped(t *testing.T) {
	command := exec.Command("/bin/sh", "-c", "exec sleep 30")
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	reaped := false
	t.Cleanup(func() {
		if reaped {
			return
		}
		_ = command.Process.Kill()
		_ = command.Wait()
	})

	if err := terminateProcess(command.Process, processParentPipes{}, 0); err != nil {
		t.Fatalf("terminate unreaped process: %v", err)
	}
	if err := command.Wait(); err == nil {
		t.Fatal("process exited without a signal")
	}
	reaped = true
}

func TestAwaitProcessCompletionCleansDescendantsAfterLeaderExit(t *testing.T) {
	command := exec.Command("/bin/sh", "-c", "sleep 30 &")
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	if err := command.Wait(); err != nil {
		t.Fatal(err)
	}
	processGroup := command.Process.Pid
	exists, err := processGroupExists(processGroup)
	if err != nil {
		t.Fatal(err)
	}
	if !exists {
		t.Fatal("background descendant exited before the test")
	}
	cleaned := false
	t.Cleanup(func() {
		if cleaned {
			return
		}
		if err := syscall.Kill(-processGroup, syscall.SIGKILL); err != nil && !errors.Is(err, syscall.ESRCH) {
			t.Errorf("kill process group: %v", err)
		}
	})

	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	waitCompleted := make(chan error)
	type completion struct {
		waitErr  error
		canceled bool
		err      error
	}
	completed := make(chan completion, 1)
	go func() {
		waitErr, canceled, err := awaitProcessCompletion(
			ctx,
			command.Process,
			processParentPipes{},
			waitCompleted,
			time.Second,
		)
		completed <- completion{waitErr: waitErr, canceled: canceled, err: err}
	}()

	exited, err := waitForProcessInvocation(command.Process, time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if !exited {
		t.Fatal("process group still exists after completion cleanup")
	}
	cleaned = true
	waitCompleted <- nil
	result := <-completed
	if result.waitErr != nil || result.canceled || result.err != nil {
		t.Fatalf("completion = (%v, %t, %v)", result.waitErr, result.canceled, result.err)
	}
}

func TestProcessCompletionEvent(t *testing.T) {
	request := ProcessStartRequest{Source: "operation-1", CorrelationID: "process-1"}
	exited := PrimitiveEvent{
		Type:          PrimitiveEventProcessExited,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result:        ProcessExitResult{ExitCode: 0},
	}
	if got := processCompletionEvent(request, exited, nil); got.Type != PrimitiveEventProcessExited {
		t.Fatalf("completion = %#v", got)
	}
	want := errors.New("cleanup failed")
	got := processCompletionEvent(request, exited, want)
	if got.Type != PrimitiveEventFailed ||
		!strings.Contains(got.Result.(PrimitiveFailureResult).Error, want.Error()) {
		t.Fatalf("completion = %#v", got)
	}
}

func TestCanceledProcessEventDelivery(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	events := make(chan PrimitiveEvent, 1)
	if sendProcessEvent(ctx, events, PrimitiveEvent{Type: PrimitiveEventProcessStarted}) {
		t.Fatal("event delivered after cancellation")
	}

	request := ProcessStartRequest{Source: "operation-1", CorrelationID: "process-1"}
	sendProcessTerminalEvent(events, PrimitiveEvent{
		Type:          PrimitiveEventProcessExited,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
	})
	if event := <-events; event.Type != PrimitiveEventProcessExited ||
		event.Source != request.Source || event.CorrelationID != request.CorrelationID {
		t.Fatalf("terminal event = %#v", event)
	}

	failure := processFailure(request.Source, request.CorrelationID, errors.New("failed"))
	sendProcessTerminalEvent(events, failure)
	if event := <-events; event.Type != PrimitiveEventFailed {
		t.Fatalf("failure event = %#v", event)
	}
}

func TestBlockedProcessEventDeliveryUnblocksOnCancellation(t *testing.T) {
	baseContext, cancel := context.WithCancel(t.Context())
	ctx := &observedErrContext{Context: baseContext, checked: make(chan struct{})}
	events := make(chan PrimitiveEvent, 1)
	events <- PrimitiveEvent{Type: PrimitiveEventProcessOutput}
	delivered := make(chan bool, 1)
	go func() {
		delivered <- sendProcessEvent(
			ctx,
			events,
			PrimitiveEvent{Type: PrimitiveEventProcessOutput},
		)
	}()
	<-ctx.checked
	cancel()
	if <-delivered {
		t.Fatal("blocked event delivered after cancellation")
	}
}

func TestBlockedProcessTerminalDeliveryPreservesQueuedEvent(t *testing.T) {
	events := make(chan PrimitiveEvent, 1)
	events <- PrimitiveEvent{Type: PrimitiveEventProcessOutput}
	delivered := make(chan struct{})
	go func() {
		sendProcessTerminalEvent(events, PrimitiveEvent{Type: PrimitiveEventProcessExited})
		close(delivered)
	}()
	if event := <-events; event.Type != PrimitiveEventProcessOutput {
		t.Fatalf("queued event = %#v", event)
	}
	<-delivered
	if event := <-events; event.Type != PrimitiveEventProcessExited {
		t.Fatalf("terminal event = %#v", event)
	}
}

func TestProcessTerminalDeliveryDoesNotReplaceQueuedEvent(t *testing.T) {
	events := make(chan PrimitiveEvent, 1)
	events <- PrimitiveEvent{Type: PrimitiveEventProcessOutput}
	delivered := make(chan struct{})
	go func() {
		sendProcessTerminalEvent(events, PrimitiveEvent{Type: PrimitiveEventCanceled})
		close(delivered)
	}()

	if event := <-events; event.Type != PrimitiveEventProcessOutput {
		t.Fatalf("event = %#v", event)
	}
	<-delivered
	if event := <-events; event.Type != PrimitiveEventCanceled {
		t.Fatalf("terminal event = %#v", event)
	}
}

func assertPrimitiveEventChannelOpen(t *testing.T, events <-chan PrimitiveEvent) {
	t.Helper()
	select {
	case event, ok := <-events:
		if !ok {
			t.Fatal("caller-owned event channel was closed")
		}
		t.Fatalf("unexpected event = %#v", event)
	default:
	}
}

func processTestPipe(t *testing.T) (*os.File, *os.File) {
	t.Helper()
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("create pipe: %v", err)
	}
	t.Cleanup(func() {
		closeProcessTestFile(t, reader)
		closeProcessTestFile(t, writer)
	})
	return reader, writer
}

func waitForInternalFile(t *testing.T, path string, want string) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	delay := time.Millisecond
	for time.Now().Before(deadline) {
		data, err := os.ReadFile(path)
		if err == nil && string(data) == want {
			return
		}
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("read file %s: %v", path, err)
		}
		time.Sleep(delay)
		delay = min(delay*2, 50*time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", path)
}

func closeProcessTestFile(t *testing.T, file *os.File) {
	t.Helper()
	if err := normalizeProcessCloseError(file.Close()); err != nil {
		t.Errorf("close %s: %v", file.Name(), err)
	}
}

type observedErrContext struct {
	context.Context
	checked chan struct{}
	once    sync.Once
}

type observedDoneContext struct {
	context.Context
	checked chan struct{}
	once    sync.Once
}

func (ctx *observedDoneContext) Done() <-chan struct{} {
	ctx.once.Do(func() { close(ctx.checked) })
	return ctx.Context.Done()
}

type cancelAfterFirstErrContext struct {
	context.Context
	cancel context.CancelFunc
	once   sync.Once
}

func (ctx *cancelAfterFirstErrContext) Err() error {
	err := ctx.Context.Err()
	if err == nil {
		ctx.once.Do(ctx.cancel)
	}
	return err
}

type cancelOnFirstErrContext struct {
	context.Context
	cancel context.CancelFunc
	once   sync.Once
}

func (ctx *cancelOnFirstErrContext) Err() error {
	ctx.once.Do(ctx.cancel)
	return ctx.Context.Err()
}

func (ctx *observedErrContext) Err() error {
	err := ctx.Context.Err()
	ctx.once.Do(func() { close(ctx.checked) })
	return err
}
