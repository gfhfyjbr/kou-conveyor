package primitives

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"strings"
	"testing"
)

func TestInspectAndStreamFileReportsStatFailure(t *testing.T) {
	statErr := errors.New("stat failed")
	closeErr := errors.New("close failed")
	events := make(chan PrimitiveEvent, 1)
	inspectAndStreamFile(
		t.Context(),
		IOReadRequest{Path: "input"},
		&statFailureReadFile{File: openReadTestFile(t, nil), statErr: statErr, closeErr: closeErr},
		events,
	)
	close(events)

	failure := singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventFailed)
	result := failure.Result.(PrimitiveFailureResult)
	if !strings.Contains(result.Error, statErr.Error()) || !strings.Contains(result.Error, closeErr.Error()) {
		t.Fatalf("error = %q, want stat and close failures", result.Error)
	}
}

func TestStreamOpenFileReportsReadFailure(t *testing.T) {
	readErr := errors.New("read failed")
	events := collectInternalEvents(startFileStream(
		t.Context(),
		IOReadRequest{Path: "input", Count: 1},
		&readFailureFile{File: openReadTestFile(t, []byte("x")), err: readErr},
	))

	failure := singleInternalEvent(t, events, PrimitiveEventFailed)
	result := failure.Result.(PrimitiveFailureResult)
	if !strings.Contains(result.Error, readErr.Error()) {
		t.Fatalf("error = %q, want %q", result.Error, readErr)
	}
}

func TestStreamOpenFileReportsCompletionCloseFailure(t *testing.T) {
	closeErr := errors.New("close failed")
	events := collectInternalEvents(startFileStream(
		t.Context(),
		IOReadRequest{Path: "input"},
		&closeFailureFile{File: openReadTestFile(t, nil), err: closeErr},
	))

	failure := singleInternalEvent(t, events, PrimitiveEventFailed)
	result := failure.Result.(PrimitiveFailureResult)
	if !strings.Contains(result.Error, closeErr.Error()) {
		t.Fatalf("error = %q, want %q", result.Error, closeErr)
	}
}

func TestStreamOpenFileCancellationWhileSendingOutput(t *testing.T) {
	for _, test := range []struct {
		name      string
		closeErr  error
		eventType PrimitiveEventType
	}{
		{name: "closed", eventType: PrimitiveEventCanceled},
		{name: "close failure", closeErr: errors.New("close failed"), eventType: PrimitiveEventFailed},
	} {
		t.Run(test.name, func(t *testing.T) {
			baseContext, cancel := context.WithCancel(t.Context())
			defer cancel()
			ctx := &observedDoneContext{Context: baseContext, checked: make(chan struct{})}
			file := &blockingCloseFile{
				File:     openReadTestFile(t, []byte("contents")),
				started:  make(chan struct{}),
				release:  make(chan struct{}),
				closeErr: test.closeErr,
			}
			events := startFileStream(ctx, IOReadRequest{Path: file.Name(), Count: 8}, file)

			// Done is evaluated in the send select. Keep output blocked until cancellation reaches Close.
			<-ctx.checked
			cancel()
			<-file.started
			close(file.release)

			var received []PrimitiveEvent
			for event := range events {
				received = append(received, event)
			}
			event := singleInternalEvent(t, received, test.eventType)
			if test.closeErr != nil && !strings.Contains(event.Result.(PrimitiveFailureResult).Error, test.closeErr.Error()) {
				t.Fatalf("failure = %#v, want close error", event.Result)
			}
			if _, err := file.Stat(); !errors.Is(err, os.ErrClosed) {
				t.Fatalf("file not closed: %v", err)
			}
		})
	}
}

func TestStreamOpenFileCancellationAfterClose(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	file := &blockingCloseFile{
		File:    openReadTestFile(t, nil),
		started: make(chan struct{}),
		release: make(chan struct{}),
	}
	events := startFileStream(ctx, IOReadRequest{Path: "input"}, file)

	<-file.started
	cancel()
	close(file.release)

	singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventCanceled)
}

func TestStreamOpenFileCancellationDuringRead(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	file := &cancelingFile{File: openReadTestFile(t, []byte("content")), cancel: cancel}
	events := startFileStream(ctx, IOReadRequest{Path: "input", Count: 7}, file)

	singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventCanceled)
}

func TestStreamOpenFileCancellationBeforeRead(t *testing.T) {
	for _, count := range []int64{0, 1} {
		t.Run(fmt.Sprintf("count=%d", count), func(t *testing.T) {
			ctx, cancel := context.WithCancel(t.Context())
			cancel()
			file := &observedReadFile{File: openReadTestFile(t, []byte("x"))}
			events := startFileStream(ctx, IOReadRequest{Path: "input", Count: count}, file)
			singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventCanceled)
			if len(file.reads) != 0 || !file.closed {
				t.Fatalf("reads = %v, closed = %t, want no reads and a closed file", file.reads, file.closed)
			}
		})
	}
}

func openReadTestFile(t *testing.T, contents []byte) *os.File {
	t.Helper()
	file, err := os.CreateTemp(t.TempDir(), "input")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = file.Close() })
	if _, err := file.Write(contents); err != nil {
		t.Fatal(err)
	}
	return file
}

type readFailureFile struct {
	*os.File
	err error
}

func (file *readFailureFile) ReadAt([]byte, int64) (int, error) {
	return 0, file.err
}

type statFailureReadFile struct {
	*os.File
	statErr  error
	closeErr error
}

func (file *statFailureReadFile) Close() error {
	return errors.Join(file.File.Close(), file.closeErr)
}

func (file *statFailureReadFile) Stat() (os.FileInfo, error) {
	return nil, file.statErr
}

type closeFailureFile struct {
	*os.File
	err error
}

func (file *closeFailureFile) Close() error {
	return errors.Join(file.File.Close(), file.err)
}

type blockingCloseFile struct {
	*os.File
	started  chan struct{}
	release  chan struct{}
	closeErr error
}

func (file *blockingCloseFile) Close() error {
	file.started <- struct{}{}
	<-file.release
	return errors.Join(file.File.Close(), file.closeErr)
}

type cancelingFile struct {
	*os.File
	cancel context.CancelFunc
}

func (file *cancelingFile) ReadAt(destination []byte, offset int64) (int, error) {
	count, err := file.File.ReadAt(destination, offset)
	file.cancel()
	return count, err
}

func startFileStream(
	ctx context.Context,
	request IOReadRequest,
	file inspectedReadFile,
) <-chan PrimitiveEvent {
	events := make(chan PrimitiveEvent)
	go func() {
		defer close(events)
		inspectAndStreamFile(ctx, request, file, events)
	}()
	return events
}

func collectInternalEvents(events <-chan PrimitiveEvent) []PrimitiveEvent {
	var collected []PrimitiveEvent
	for {
		event := <-events
		collected = append(collected, event)
		if internalPrimitiveEventIsTerminal(event.Type) {
			return collected
		}
	}
}

func internalPrimitiveEventIsTerminal(eventType PrimitiveEventType) bool {
	switch eventType {
	case PrimitiveEventFailed,
		PrimitiveEventCanceled,
		PrimitiveEventIOCreateCompleted,
		PrimitiveEventIOReadCompleted,
		PrimitiveEventProcessExited,
		PrimitiveEventProcessInputWritten,
		PrimitiveEventProcessInputWriteFailed,
		PrimitiveEventProcessInputClosed,
		PrimitiveEventProcessSignaled,
		PrimitiveEventRemoteCompleted,
		PrimitiveEventTimerFired:
		return true
	default:
		return false
	}
}

func singleInternalEvent(
	t *testing.T,
	events []PrimitiveEvent,
	eventType PrimitiveEventType,
) PrimitiveEvent {
	t.Helper()
	if len(events) != 1 {
		t.Fatalf("events = %d, want 1", len(events))
	}
	if events[0].Type != eventType {
		t.Fatalf("event type = %q, want %q", events[0].Type, eventType)
	}
	return events[0]
}

func TestIsOnlyContextCancellation(t *testing.T) {
	otherErr := errors.New("cleanup failed")
	tests := []struct {
		name string
		err  error
		want bool
	}{
		{name: "canceled", err: context.Canceled, want: true},
		{name: "deadline", err: context.DeadlineExceeded, want: true},
		{name: "wrapped", err: fmt.Errorf("read: %w", context.Canceled), want: true},
		{name: "joined cancellations", err: errors.Join(context.Canceled, context.DeadlineExceeded), want: true},
		{name: "mixed join", err: errors.Join(context.Canceled, otherErr), want: false},
		{name: "other", err: otherErr, want: false},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := isOnlyContextCancellation(test.err); got != test.want {
				t.Fatalf("isOnlyContextCancellation() = %t, want %t", got, test.want)
			}
		})
	}
}

func TestReadFileAccessesOnlyRequestedRange(t *testing.T) {
	const size = int64(IOReadChunkSize*3 + 17)
	for _, test := range []struct {
		name   string
		size   int64
		offset int64
		count  int64
		want   int64
	}{
		{name: "prefix", size: size, count: 7, want: 7},
		{name: "middle across chunks", size: size, offset: 13, count: IOReadChunkSize*2 + 3, want: IOReadChunkSize*2 + 3},
		{name: "tail", size: size, offset: size - 5, count: 5, want: 5},
		{name: "clamped overflowing range", size: size, offset: size - 7, count: math.MaxInt64, want: 7},
		{name: "zero count", size: size, offset: 11, count: 0, want: 0},
		{name: "at EOF", size: size, offset: size, count: 5, want: 0},
		{name: "beyond EOF", size: size, offset: math.MaxInt64, count: math.MaxInt64, want: 0},
		{name: "empty file", size: 0, count: math.MaxInt64, want: 0},
	} {
		t.Run(test.name, func(t *testing.T) {
			file := &observedReadFile{File: openReadTestFile(t, nil)}
			if err := file.Truncate(test.size); err != nil {
				t.Fatal(err)
			}
			request := IOReadRequest{Source: "source", CorrelationID: "read", Path: file.Name(), Offset: test.offset, Count: test.count}
			events := collectInternalEvents(startFileStream(t.Context(), request, file))
			nextOffset := test.offset
			for _, read := range file.reads {
				if read.Offset != nextOffset || read.Count <= 0 || read.Count > IOReadChunkSize {
					t.Fatalf("read = %+v, want offset %d and a bounded nonempty chunk", read, nextOffset)
				}
				nextOffset += read.Count
			}
			if nextOffset-test.offset != test.want {
				t.Fatalf("bytes requested from file = %d, want %d", nextOffset-test.offset, test.want)
			}
			if file.stats != 2 || !file.closed {
				t.Fatalf("stats = %d, closed = %t, want 2 and true", file.stats, file.closed)
			}
			nextOffset = test.offset
			for _, event := range events {
				if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
					t.Fatalf("event identity = (%q, %q)", event.Source, event.CorrelationID)
				}
				switch event.Type {
				case PrimitiveEventIOReadOutput:
					output := event.Result.(IOReadOutputResult)
					if output.Offset != nextOffset || len(output.Data) == 0 {
						t.Fatalf("invalid output at %d, expected %d", output.Offset, nextOffset)
					}
					nextOffset += int64(len(output.Data))
				case PrimitiveEventIOReadCompleted:
					if result := event.Result.(IOReadCompletedResult); result.Size != test.size {
						t.Fatalf("size = %d, want %d", result.Size, test.size)
					}
				default:
					t.Fatalf("unexpected event: %#v", event)
				}
			}
			if nextOffset-test.offset != test.want {
				t.Fatalf("output size = %d, want %d", nextOffset-test.offset, test.want)
			}
		})
	}
}

type fileReadRange struct {
	Offset int64
	Count  int64
}

type observedReadFile struct {
	*os.File
	reads  []fileReadRange
	stats  int
	closed bool
}

func (file *observedReadFile) ReadAt(data []byte, offset int64) (int, error) {
	file.reads = append(file.reads, fileReadRange{Offset: offset, Count: int64(len(data))})
	return file.File.ReadAt(data, offset)
}

func (file *observedReadFile) Stat() (os.FileInfo, error) {
	file.stats++
	return file.File.Stat()
}

func (file *observedReadFile) Close() error {
	file.closed = true
	return file.File.Close()
}

func TestStreamOpenFileRejectsPrematureEOF(t *testing.T) {
	file := openReadTestFile(t, []byte("contents"))
	info, err := file.Stat()
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Truncate(3); err != nil {
		t.Fatal(err)
	}
	events := make(chan PrimitiveEvent, 2)
	streamOpenFile(t.Context(), IOReadRequest{Path: file.Name(), Offset: 2, Count: 4}, file, info.Size(), events)
	failure := singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventFailed)
	if result := failure.Result.(PrimitiveFailureResult); !strings.Contains(result.Error, io.ErrUnexpectedEOF.Error()) {
		t.Fatalf("failure = %q, want unexpected EOF", result.Error)
	}
	if _, err := file.Stat(); !errors.Is(err, os.ErrClosed) {
		t.Fatalf("file not closed: %v", err)
	}
}

func TestStreamOpenFileDetectsSizeChangesOutsideRange(t *testing.T) {
	for _, test := range []struct {
		name string
		size int64
	}{
		{name: "growth", size: 20},
		{name: "shrinkage", size: 4},
	} {
		t.Run(test.name, func(t *testing.T) {
			file := &pausingReadFile{
				File:    openReadTestFile(t, []byte("contents")),
				read:    make(chan struct{}),
				release: make(chan struct{}),
			}
			events := startFileStream(t.Context(), IOReadRequest{Path: file.Name(), Count: 2}, file)
			<-file.read
			err := file.Truncate(test.size)
			close(file.release)
			if err != nil {
				t.Fatal(err)
			}
			output := <-events
			if output.Type != PrimitiveEventIOReadOutput || string(output.Result.(IOReadOutputResult).Data) != "co" {
				t.Fatalf("output = %#v", output)
			}
			failure := singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventFailed)
			if result := failure.Result.(PrimitiveFailureResult); !strings.Contains(result.Error, "file size changed") {
				t.Fatalf("failure = %q, want size change", result.Error)
			}
			if _, err := file.Stat(); !errors.Is(err, os.ErrClosed) {
				t.Fatalf("file not closed: %v", err)
			}
		})
	}
}

type pausingReadFile struct {
	*os.File
	read    chan struct{}
	release chan struct{}
}

func (file *pausingReadFile) ReadAt(data []byte, offset int64) (int, error) {
	count, err := file.File.ReadAt(data, offset)
	close(file.read)
	<-file.release
	return count, err
}

func TestStreamOpenFileReportsFinalStatAndCloseFailures(t *testing.T) {
	file := &statFailureReadFile{
		File:     openReadTestFile(t, nil),
		statErr:  errors.New("stat failed"),
		closeErr: errors.New("close failed"),
	}
	events := make(chan PrimitiveEvent, 1)
	streamOpenFile(t.Context(), IOReadRequest{Path: file.Name()}, file, 0, events)
	failure := singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventFailed)
	result := failure.Result.(PrimitiveFailureResult)
	for _, want := range []string{"after reading", file.statErr.Error(), file.closeErr.Error()} {
		if !strings.Contains(result.Error, want) {
			t.Fatalf("failure = %q, want %q", result.Error, want)
		}
	}
}

func TestStreamOpenFileAcceptsFullReadWithEOF(t *testing.T) {
	file := &eofReadFile{File: openReadTestFile(t, []byte("contents"))}
	events := collectInternalEvents(startFileStream(t.Context(), IOReadRequest{Path: file.Name(), Count: 8}, file))
	if len(events) != 2 || events[0].Type != PrimitiveEventIOReadOutput || events[1].Type != PrimitiveEventIOReadCompleted {
		t.Fatalf("events = %#v, want output and completion", events)
	}
	if output := events[0].Result.(IOReadOutputResult); string(output.Data) != "contents" {
		t.Fatalf("output = %q", output.Data)
	}
}

type eofReadFile struct {
	*os.File
}

func (file *eofReadFile) ReadAt(data []byte, offset int64) (int, error) {
	count, err := file.File.ReadAt(data, offset)
	if err != nil {
		return count, err
	}
	return count, io.EOF
}

func TestStreamOpenFileFailureAfterOutput(t *testing.T) {
	readErr := errors.New("read failed")
	closeErr := errors.New("close failed")
	for _, test := range []struct {
		name     string
		readErr  error
		closeErr error
		want     error
	}{
		{name: "truncated second chunk", want: io.ErrUnexpectedEOF},
		{name: "second read fails", readErr: readErr, want: readErr},
		{name: "second read and close fail", readErr: readErr, closeErr: closeErr, want: readErr},
	} {
		t.Run(test.name, func(t *testing.T) {
			contents := bytes.Repeat([]byte("x"), IOReadChunkSize+17)
			file := &interruptedReadFile{
				closeFailureFile: &closeFailureFile{File: openReadTestFile(t, contents), err: test.closeErr},
				ready:            make(chan struct{}),
				release:          make(chan struct{}),
				readErr:          test.readErr,
			}
			request := IOReadRequest{Source: "source", CorrelationID: "read", Path: file.Name(), Count: int64(len(contents))}
			events := startFileStream(t.Context(), request, file)
			first := <-events
			if first.Type != PrimitiveEventIOReadOutput {
				t.Fatalf("first event = %#v, want output", first)
			}
			output := first.Result.(IOReadOutputResult)
			if output.Offset != 0 || !bytes.Equal(output.Data, contents[:IOReadChunkSize]) {
				t.Fatalf("first output at %d does not match the first chunk", output.Offset)
			}

			<-file.ready
			var truncateErr error
			if test.readErr == nil {
				truncateErr = file.Truncate(IOReadChunkSize + 3)
			}
			close(file.release)
			var remaining []PrimitiveEvent
			for event := range events {
				remaining = append(remaining, event)
			}
			if truncateErr != nil {
				t.Fatal(truncateErr)
			}
			failure := singleInternalEvent(t, remaining, PrimitiveEventFailed)
			result := failure.Result.(PrimitiveFailureResult)
			if !strings.Contains(result.Error, test.want.Error()) {
				t.Fatalf("failure = %q, want %q", result.Error, test.want)
			}
			if test.closeErr != nil && !strings.Contains(result.Error, test.closeErr.Error()) {
				t.Fatalf("failure = %q, want close error", result.Error)
			}
			for _, event := range []PrimitiveEvent{first, failure} {
				if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
					t.Fatalf("event identity = (%q, %q)", event.Source, event.CorrelationID)
				}
			}
			if _, err := file.Stat(); !errors.Is(err, os.ErrClosed) {
				t.Fatalf("file not closed: %v", err)
			}
		})
	}
}

type interruptedReadFile struct {
	*closeFailureFile
	ready   chan struct{}
	release chan struct{}
	readErr error
}

func (file *interruptedReadFile) ReadAt(data []byte, offset int64) (int, error) {
	if offset >= IOReadChunkSize {
		close(file.ready)
		<-file.release
		if file.readErr != nil {
			return 0, file.readErr
		}
	}
	return file.File.ReadAt(data, offset)
}
