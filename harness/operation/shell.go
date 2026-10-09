package operation

import (
	"bytes"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"time"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

const (
	TypeShell    Type    = "shell"
	VersionShell Version = 3

	ShellOutFilename = "out"
	ShellErrFilename = "err"
)

type ShellPhase string

const (
	ShellPhaseCreateDirectory ShellPhase = "create_directory"
	ShellPhaseCreateOut       ShellPhase = "create_out"
	ShellPhaseCreateErr       ShellPhase = "create_err"
	ShellPhaseProcess         ShellPhase = "process"
	ShellPhaseReadOut         ShellPhase = "read_out"
	ShellPhaseReadOutTail     ShellPhase = "read_out_tail"
	ShellPhaseReadErr         ShellPhase = "read_err"
	ShellPhaseReadErrTail     ShellPhase = "read_err_tail"

	// The timers of a command's timeout: one to terminate it, then one to
	// kill it.
	shellTimeoutCorrelation primitives.CorrelationID = "timeout"
	shellKillCorrelation    primitives.CorrelationID = "kill"
	shellKillGrace                                   = 5 * time.Second

	// MaxShellTimeout bounds a command's timeout, in seconds.
	MaxShellTimeout = 4 * 60 * 60
)

type ShellInput struct {
	Command   string
	Shell     string
	Directory string
	// Timeout is how many seconds the command may run before it is
	// terminated (then killed); 0 lets it run.
	Timeout float64 `json:",omitzero"`
}

type ShellResult struct {
	Out      string
	Err      string
	OutSize  int64
	ErrSize  int64
	ExitCode int
	// TimedOut reports a command stopped at its timeout.
	TimedOut bool `json:",omitzero"`
	// Duration is how long the command ran, in seconds.
	Duration float64 `json:",omitzero"`
}

type ShellState struct {
	Input         ShellInput
	BaseDirectory string

	Phase           ShellPhase
	ProcessGroupID  int
	StartedAt       time.Time `json:",omitzero"`
	TimedOut        bool      `json:",omitzero"`
	PendingExitCode *int
	OutSize         int64
	ErrSize         int64
	InlineOut       []byte
	InlineErr       []byte
	InlineOutTail   []byte
	InlineErrTail   []byte
	Result          *ShellResult
	TerminalError   string
	ErrorTruncated  bool
	OutTruncated    bool
	ErrTruncated    bool
	OutPath         string
	ErrPath         string
}

type Shell struct {
	current  Operation
	state    ShellState
	chunks   [][]byte
	readSize int64
}

func NewShell(current Operation) (*Shell, error) {
	state, err := shellOperationState(current)
	if err != nil {
		return nil, err
	}
	current.State = nil
	return &Shell{current: current, state: state}, nil
}

func NewShellSpec(
	input ShellInput,
	baseDirectory string,
	maxOutputLength int,
) (Spec, error) {
	if maxOutputLength <= 0 || maxOutputLength > MaxOutputLength {
		return Spec{}, errors.New("max output length is out of range")
	}
	state := ShellState{
		Input:         input,
		BaseDirectory: baseDirectory,
		OutTruncated:  true,
		ErrTruncated:  true,
	}
	if err := validateShellState(state); err != nil {
		return Spec{}, err
	}

	encoded, err := json.Marshal(state)
	if err != nil {
		return Spec{}, fmt.Errorf("encode shell operation state: %w", err)
	}
	return Spec{
		MaxOutputLength: maxOutputLength,
		Type:            TypeShell,
		Version:         VersionShell,
		State:           encoded,
	}, nil
}

func (shell *Shell) Handle(event *primitives.PrimitiveEvent) (Step, error) {
	current, state := &shell.current, &shell.state

	switch current.Status {
	case StatusReady:
		if event != nil {
			return Step{}, fmt.Errorf("advance ready shell operation %q: unexpected primitive event", current.ID)
		}
		if state.Phase != "" || state.Result != nil || state.TerminalError != "" {
			return Step{}, fmt.Errorf("advance ready shell operation %q: state is not initial", current.ID)
		}
		paths, pathErr := newShellPaths(state.BaseDirectory, current.ID)
		if pathErr != nil {
			return shell.fail(pathErr)
		}
		return shell.next(ShellPhaseCreateDirectory, paths)

	case StatusAwaiting:
		if event == nil {
			return shell.resume()
		}
		return shell.handleAwaiting(*event)

	case StatusCanceling:
		if event != nil {
			return Step{}, fmt.Errorf("advance canceling shell operation %q: unexpected primitive event", current.ID)
		}
		return shell.cancel()

	default:
		return Step{}, fmt.Errorf("advance shell operation %q: terminal status %q", current.ID, current.Status)
	}
}

func shellOperationState(current Operation) (ShellState, error) {
	state, err := DecodeShellState(current)
	if err != nil {
		return ShellState{}, err
	}
	if err := validateShellState(state); err != nil {
		return ShellState{}, fmt.Errorf("validate shell operation %q state: %w", current.ID, err)
	}
	if int64(len(state.InlineOut)+len(state.InlineOutTail)) > current.shellReadLimit() || int64(len(state.InlineErr)+len(state.InlineErrTail)) > current.shellReadLimit() {
		return ShellState{}, errors.New("inline shell result exceeds the configured limit")
	}
	return state, nil
}

func DecodeShellState(current Operation) (ShellState, error) {
	if current.Type != TypeShell {
		return ShellState{}, fmt.Errorf(
			"advance shell operation %q: unsupported type %q: %w",
			current.ID,
			current.Type,
			ErrUnsupported,
		)
	}
	if current.Version != VersionShell {
		return ShellState{}, fmt.Errorf(
			"advance shell operation %q: unsupported version %d: %w",
			current.ID,
			current.Version,
			ErrUnsupported,
		)
	}
	if current.MaxOutputLength <= 0 || current.MaxOutputLength > MaxOutputLength {
		return ShellState{}, errors.New("max output length is out of range")
	}

	var state ShellState
	if err := json.Unmarshal(current.State, &state); err != nil {
		return ShellState{}, fmt.Errorf("decode shell operation %q state: %w", current.ID, err)
	}
	return state, nil
}

func (shell *Shell) resume() (Step, error) {
	current, state := &shell.current, &shell.state

	if state.Phase == ShellPhaseProcess {
		if state.ProcessGroupID == 0 {
			return shell.fail(errors.New("shell execution outcome is unknown because process start was not recorded"))
		}
		return shell.fail(errors.New("shell execution was interrupted before an exit status was recorded"))
	}

	paths, err := newShellPaths(state.BaseDirectory, current.ID)
	if err != nil {
		return shell.fail(err)
	}
	shell.chunks, shell.readSize = nil, 0
	return shell.dispatchPhase(paths)
}

func (shell *Shell) next(phase ShellPhase, paths shellPaths) (Step, error) {
	shell.state.Phase = phase
	return shell.dispatchPhase(paths)
}

func (shell *Shell) dispatchPhase(paths shellPaths) (Step, error) {
	current, state := &shell.current, &shell.state
	correlation := primitives.CorrelationID(state.Phase)

	switch state.Phase {
	case ShellPhaseCreateDirectory:
		return shell.dispatch(createShellPath(
			current.ID,
			correlation,
			primitives.IOCreateDirectory,
			paths.directory,
			0o700,
		))
	case ShellPhaseCreateOut:
		return shell.dispatch(createShellFile(current.ID, correlation, paths.out))
	case ShellPhaseCreateErr:
		return shell.dispatch(createShellFile(current.ID, correlation, paths.err))
	case ShellPhaseProcess:
		return shell.dispatch(startShellProcess(current.ID, *state, paths))
	case ShellPhaseReadOut, ShellPhaseReadErr, ShellPhaseReadOutTail, ShellPhaseReadErrTail:
		request, err := shell.readRequest(paths)
		if err != nil {
			return shell.fail(err)
		}
		return shell.dispatch(PrimitiveDispatch{Type: primitives.PrimitiveDispatchIORead, Data: request})
	default:
		return shell.fail(fmt.Errorf("shell operation has invalid phase %q", state.Phase))
	}
}

func (shell *Shell) handleAwaiting(event primitives.PrimitiveEvent) (Step, error) {
	current, state := &shell.current, &shell.state

	if event.Source != primitives.SourceID(current.ID) {
		return shell.fail(fmt.Errorf(
			"shell primitive event source is %q, want %q",
			event.Source,
			current.ID,
		))
	}
	if event.CorrelationID == shellTimeoutCorrelation || event.CorrelationID == shellKillCorrelation {
		return shell.timerEvent(event)
	}
	if event.Type == primitives.PrimitiveEventCanceled {
		return shell.cancel()
	}
	if event.Type == primitives.PrimitiveEventFailed {
		return shell.fail(shellPrimitiveFailure(event))
	}
	if err := validateShellCorrelation(event, primitives.CorrelationID(state.Phase)); err != nil {
		return shell.fail(err)
	}
	paths, err := newShellPaths(state.BaseDirectory, current.ID)
	if err != nil {
		return shell.fail(err)
	}

	switch state.Phase {
	case ShellPhaseCreateDirectory, ShellPhaseCreateOut, ShellPhaseCreateErr:
		return shell.created(event, paths)

	case ShellPhaseProcess:
		return shell.processEvent(event, paths)

	case ShellPhaseReadOut, ShellPhaseReadErr, ShellPhaseReadOutTail, ShellPhaseReadErrTail:
		return shell.readEvent(event, paths)

	default:
		return shell.fail(fmt.Errorf("shell operation has invalid phase %q", state.Phase))
	}
}

func (shell *Shell) created(event primitives.PrimitiveEvent, paths shellPaths) (Step, error) {
	state := &shell.state
	kind := primitives.IOCreateRegularFile
	if state.Phase == ShellPhaseCreateDirectory {
		kind = primitives.IOCreateDirectory
	}
	if err := validateShellCreate(event, kind); err != nil {
		return shell.fail(err)
	}
	switch state.Phase {
	case ShellPhaseCreateDirectory:
		return shell.next(ShellPhaseCreateOut, paths)
	case ShellPhaseCreateOut:
		state.OutPath = paths.out
		return shell.next(ShellPhaseCreateErr, paths)
	case ShellPhaseCreateErr:
		state.ErrPath = paths.err
		return shell.next(ShellPhaseProcess, paths)
	default:
		return shell.fail(fmt.Errorf("shell operation has invalid create phase %q", state.Phase))
	}
}

func (shell *Shell) processEvent(event primitives.PrimitiveEvent, paths shellPaths) (Step, error) {
	state := &shell.state

	switch event.Type {
	case primitives.PrimitiveEventProcessStarted:
		started, ok := event.Result.(primitives.ProcessStartedResult)
		if !ok || started.PID <= 1 {
			return shell.fail(errors.New("start shell returned an invalid result"))
		}
		state.ProcessGroupID = started.PID
		state.StartedAt = time.Now().Round(0)
		step, err := shell.await()
		if err != nil {
			return Step{}, err
		}
		if state.Input.Timeout > 0 {
			step.Dispatches = []PrimitiveDispatch{shell.timer(shellTimeoutCorrelation, time.Duration(state.Input.Timeout*float64(time.Second)))}
		}
		return step, nil

	case primitives.PrimitiveEventProcessExited:
		exit, ok := event.Result.(primitives.ProcessExitResult)
		if !ok {
			return shell.fail(errors.New("shell returned an invalid exit result"))
		}
		exitCode := shellExitStatus(exit)
		state.ProcessGroupID = 0
		state.PendingExitCode = &exitCode
		return shell.next(ShellPhaseReadOut, paths)

	case primitives.PrimitiveEventProcessOutput:
		return shell.fail(errors.New("captured shell process produced an unexpected output event"))

	case primitives.PrimitiveEventProcessStreamFailed:
		return shell.fail(errors.New("captured shell process returned an unexpected stream failure"))

	default:
		return shell.fail(fmt.Errorf("shell process returned unexpected event %q", event.Type))
	}
}

// timer is a timer of the command's timeout that fires after delay.
func (shell *Shell) timer(correlation primitives.CorrelationID, delay time.Duration) PrimitiveDispatch {
	return PrimitiveDispatch{
		Type: primitives.PrimitiveDispatchTimerSchedule,
		Data: primitives.TimerRequest{
			Source:        primitives.SourceID(shell.current.ID),
			CorrelationID: correlation,
			Deadline:      time.Now().Add(delay),
		},
	}
}

// timerEvent answers the timers of the command's timeout: the first
// terminates the command's process group, the second kills what is left
// of it. A timer that fires once the command has exited means nothing; so
// does one canceled or failed, the command being canceled with it.
func (shell *Shell) timerEvent(event primitives.PrimitiveEvent) (Step, error) {
	state := &shell.state
	if event.Type != primitives.PrimitiveEventTimerFired || state.Phase != ShellPhaseProcess || state.ProcessGroupID == 0 {
		return Step{}, nil
	}
	switch event.CorrelationID {
	case shellTimeoutCorrelation:
		state.TimedOut = true
		syscall.Kill(-state.ProcessGroupID, syscall.SIGTERM)
		step, err := shell.checkpoint()
		if err != nil {
			return Step{}, err
		}
		step.Dispatches = []PrimitiveDispatch{shell.timer(shellKillCorrelation, shellKillGrace)}
		return step, nil
	case shellKillCorrelation:
		syscall.Kill(-state.ProcessGroupID, syscall.SIGKILL)
	}
	return Step{}, nil
}

func (shell *Shell) readRequest(paths shellPaths) (primitives.IOReadRequest, error) {
	current, state := &shell.current, &shell.state
	request := primitives.IOReadRequest{
		Source:        primitives.SourceID(current.ID),
		CorrelationID: primitives.CorrelationID(state.Phase),
		Count:         current.shellReadLimit(),
	}
	var size int64
	switch state.Phase {
	case ShellPhaseReadOut, ShellPhaseReadOutTail:
		request.Path, size = paths.out, state.OutSize
	case ShellPhaseReadErr, ShellPhaseReadErrTail:
		request.Path, size = paths.err, state.ErrSize
	default:
		return primitives.IOReadRequest{}, fmt.Errorf("shell operation has invalid read phase %q", state.Phase)
	}
	if state.Phase == ShellPhaseReadOutTail || state.Phase == ShellPhaseReadErrTail {
		if size <= current.shellReadLimit() {
			return primitives.IOReadRequest{}, errors.New("invalid shell tail read size")
		}
		request.Count = current.shellTailReadLimit()
		request.Offset = size - request.Count
	}
	return request, nil
}

func (shell *Shell) readEvent(event primitives.PrimitiveEvent, paths shellPaths) (Step, error) {
	request, err := shell.readRequest(paths)
	if err != nil {
		return shell.fail(err)
	}
	readSize := shell.readSize
	switch event.Type {
	case primitives.PrimitiveEventIOReadOutput:
		output, ok := event.Result.(primitives.IOReadOutputResult)
		if !ok || output.Offset != request.Offset+readSize {
			return shell.fail(errors.New("shell read returned invalid output"))
		}
		if int64(len(output.Data)) > request.Count-readSize {
			return shell.fail(errors.New("shell read exceeded its configured limit"))
		}
		shell.chunks = append(shell.chunks, output.Data)
		shell.readSize += int64(len(output.Data))
		return Step{}, nil

	case primitives.PrimitiveEventIOReadCompleted:
		result, ok := event.Result.(primitives.IOReadCompletedResult)
		if !ok || result.Size < request.Offset || readSize != min(request.Count, result.Size-request.Offset) {
			return shell.fail(errors.New("shell read returned an invalid completion"))
		}
		if shell.state.Phase == ShellPhaseReadOutTail || shell.state.Phase == ShellPhaseReadErrTail {
			if result.Size != request.Offset+request.Count {
				return shell.fail(errors.New("shell capture size changed while reading its tail"))
			}
		}
		data := bytes.Join(shell.chunks, nil)
		shell.chunks, shell.readSize = nil, 0
		return shell.readCompleted(data, result.Size, paths)

	default:
		return shell.fail(fmt.Errorf("shell read returned unexpected event %q", event.Type))
	}
}

func (shell *Shell) readCompleted(data []byte, size int64, paths shellPaths) (Step, error) {
	current, state := &shell.current, &shell.state
	needsTail := size > current.shellReadLimit()
	if needsTail && (state.Phase == ShellPhaseReadOut || state.Phase == ShellPhaseReadErr) {
		data = data[:current.shellReadLimit()-current.shellTailReadLimit()]
	}
	switch state.Phase {
	case ShellPhaseReadOut:
		state.InlineOut, state.OutSize, state.InlineOutTail = data, size, nil
		if needsTail {
			return shell.next(ShellPhaseReadOutTail, paths)
		}
		return shell.next(ShellPhaseReadErr, paths)
	case ShellPhaseReadOutTail:
		state.InlineOutTail = data
		return shell.next(ShellPhaseReadErr, paths)
	case ShellPhaseReadErr:
		state.InlineErr, state.ErrSize, state.InlineErrTail = data, size, nil
		if needsTail {
			return shell.next(ShellPhaseReadErrTail, paths)
		}
		return shell.finish(paths)
	case ShellPhaseReadErrTail:
		state.InlineErrTail = data
		return shell.finish(paths)
	default:
		return shell.fail(fmt.Errorf("shell operation has invalid read phase %q", state.Phase))
	}
}

func (shell *Shell) finish(paths shellPaths) (Step, error) {
	current, state := &shell.current, &shell.state

	if state.PendingExitCode == nil {
		return shell.fail(errors.New("shell execution completed without an exit status"))
	}
	state.Result = &ShellResult{
		OutSize:  state.OutSize,
		ErrSize:  state.ErrSize,
		ExitCode: *state.PendingExitCode,
		TimedOut: state.TimedOut,
	}
	if !state.StartedAt.IsZero() {
		state.Result.Duration = time.Since(state.StartedAt).Seconds()
	}
	state.Result.Out, state.OutTruncated = boundOutput(string(state.InlineOut), string(state.InlineOutTail), state.OutSize, current.MaxOutputLength, paths.out)
	state.Result.Err, state.ErrTruncated = boundOutput(string(state.InlineErr), string(state.InlineErrTail), state.ErrSize, current.MaxOutputLength, paths.err)
	state.OutPath, state.ErrPath = paths.out, paths.err
	state.PendingExitCode = nil
	state.InlineOut = nil
	state.InlineErr = nil
	state.InlineOutTail = nil
	state.InlineErrTail = nil
	state.Phase = ""
	current.Status = StatusCompleted
	return shell.checkpoint()
}

type shellPaths struct {
	directory string
	out       string
	err       string
}

func newShellPaths(baseDirectory string, id ID) (shellPaths, error) {
	name := string(id)
	if name == "" || name == "." || name == ".." || filepath.IsAbs(name) || filepath.Base(name) != name {
		return shellPaths{}, fmt.Errorf("operation ID %q is not a path component", id)
	}

	directory := filepath.Join(baseDirectory, name)
	return shellPaths{
		directory: directory,
		out:       filepath.Join(directory, ShellOutFilename),
		err:       filepath.Join(directory, ShellErrFilename),
	}, nil
}

func validateShellState(state ShellState) error {
	if state.Input.Shell == "" || !filepath.IsAbs(state.Input.Shell) {
		return errors.New("shell path must be absolute")
	}
	if state.BaseDirectory == "" || !filepath.IsAbs(state.BaseDirectory) {
		return errors.New("base directory must be absolute")
	}
	if state.ProcessGroupID < 0 || state.ProcessGroupID == 1 {
		return errors.New("shell process group ID must be zero or greater than one")
	}
	if state.OutSize < 0 || state.ErrSize < 0 {
		return errors.New("captured output sizes must not be negative")
	}
	if state.Input.Timeout < 0 || state.Input.Timeout > MaxShellTimeout {
		return fmt.Errorf("the timeout must be between 0 and %d seconds", MaxShellTimeout)
	}
	return nil
}

func createShellPath(
	id ID,
	correlation primitives.CorrelationID,
	kind primitives.IOCreateKind,
	path string,
	mode os.FileMode,
) PrimitiveDispatch {
	return PrimitiveDispatch{
		Type: primitives.PrimitiveDispatchIOCreate,
		Data: primitives.IOCreateRequest{
			Source:        primitives.SourceID(id),
			CorrelationID: correlation,
			Kind:          kind,
			Path:          path,
			Mode:          mode,
		},
	}
}

func createShellFile(id ID, correlation primitives.CorrelationID, path string) PrimitiveDispatch {
	return createShellPath(id, correlation, primitives.IOCreateRegularFile, path, 0o600)
}

func startShellProcess(id ID, state ShellState, paths shellPaths) PrimitiveDispatch {
	return PrimitiveDispatch{
		Type: primitives.PrimitiveDispatchProcessStart,
		Data: primitives.ProcessStartRequest{
			Source:        primitives.SourceID(id),
			CorrelationID: primitives.CorrelationID(ShellPhaseProcess),
			Path:          state.Input.Shell,
			Arguments:     []string{"-c", state.Input.Command},
			Directory:     state.Input.Directory,
			StdoutPath:    paths.out,
			StderrPath:    paths.err,
		},
	}
}

func shellExitStatus(exit primitives.ProcessExitResult) int {
	if exit.Signal != 0 {
		return 128 + int(exit.Signal)
	}
	return exit.ExitCode
}

func validateShellCorrelation(event primitives.PrimitiveEvent, correlation primitives.CorrelationID) error {
	if event.CorrelationID != correlation {
		return fmt.Errorf(
			"shell primitive event correlation is %q, want %q",
			event.CorrelationID,
			correlation,
		)
	}
	return nil
}

func validateShellCreate(event primitives.PrimitiveEvent, kind primitives.IOCreateKind) error {
	result, ok := event.Result.(primitives.IOCreateResult)
	if event.Type != primitives.PrimitiveEventIOCreateCompleted || !ok || result.Kind != kind {
		return errors.New("create shell artifact returned an invalid result")
	}
	return nil
}

func shellPrimitiveFailure(event primitives.PrimitiveEvent) error {
	failure, ok := event.Result.(primitives.PrimitiveFailureResult)
	if !ok {
		return errors.New("shell primitive failed with an invalid result")
	}
	return errors.New(failure.Error)
}

func (shell *Shell) dispatch(dispatch PrimitiveDispatch) (Step, error) {
	step, err := shell.await()
	if err != nil {
		return Step{}, err
	}
	step.Dispatches = []PrimitiveDispatch{dispatch}
	return step, nil
}

func (shell *Shell) await() (Step, error) {
	current := &shell.current

	current.Status = StatusAwaiting
	return shell.checkpoint()
}

func (shell *Shell) checkpoint() (Step, error) {
	current, state := &shell.current, &shell.state

	if !state.ErrorTruncated {
		state.TerminalError, state.ErrorTruncated = BoundOutput(state.TerminalError, current.MaxOutputLength)
	}
	encoded, err := json.Marshal(state)
	if err != nil {
		return Step{}, fmt.Errorf("encode shell operation %q state: %w", current.ID, err)
	}
	checkpoint := *current
	checkpoint.State = encoded
	return Step{Operation: &checkpoint}, nil
}

func (shell *Shell) fail(err error) (Step, error) {
	shell.chunks, shell.readSize = nil, 0

	current, state := &shell.current, &shell.state

	state.Phase = ""
	state.TerminalError = err.Error()
	state.ErrorTruncated = false
	current.Status = StatusFailed
	return shell.checkpoint()
}

func (shell *Shell) cancel() (Step, error) {
	shell.chunks, shell.readSize = nil, 0

	current, state := &shell.current, &shell.state

	state.Phase = ""
	state.TerminalError = "shell operation canceled"
	state.ErrorTruncated = false
	current.Status = StatusCanceled
	return shell.checkpoint()
}

func (current Operation) shellReadLimit() int64 {
	return int64(current.MaxOutputLength) * utf8.UTFMax
}

func (current Operation) shellTailReadLimit() int64 {
	limit := current.MaxOutputLength
	return int64(limit-limit/2) * utf8.UTFMax
}
