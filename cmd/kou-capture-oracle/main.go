// Development-only native capture projection. Runs trusted fixture scripts with
// real process capabilities, not a sandbox. All artifacts are privately owned.
package main

import (
	"context"
	"encoding/hex"
	"encoding/json/v2"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"syscall"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

type spec struct {
	Code          string
	Input         *string
	Environment   *map[string]string
	CancelBefore  bool
	CancelOnReady bool
}

type output struct {
	Started   bool
	Exited    bool
	Canceled  bool
	ExitCode  *int
	Signal    *int
	StdoutHex string
	StderrHex string
	OutMode   uint32
	ErrMode   uint32
	GroupGone bool
	Error     string
}

func run(input spec) (result output, err error) {
	directory, err := os.MkdirTemp("", "kou-go-capture-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(directory)
	directory, err = filepath.EvalSymlinks(directory)
	if err != nil {
		return result, err
	}
	out, stderr := filepath.Join(directory, "out"), filepath.Join(directory, "err")
	for _, path := range []string{out, stderr} {
		if err := os.WriteFile(path, []byte("old capture"), 0o600); err != nil {
			return result, err
		}
	}
	if err := os.WriteFile(filepath.Join(directory, "owned-marker"), []byte("fixture"), 0o600); err != nil {
		return result, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if input.CancelBefore {
		cancel()
	}
	request := primitives.ProcessStartRequest{
		Source: "owned-capture", CorrelationID: "process", Path: "/bin/sh",
		Arguments: []string{"-c", input.Code, "capture-oracle", directory},
		Directory: directory, StdoutPath: out, StderrPath: stderr,
	}
	if input.Environment != nil {
		request.Environment = []string{}
		for key, value := range *input.Environment {
			request.Environment = append(request.Environment, key+"="+value)
		}
		sort.Strings(request.Environment)
	}
	if input.Input != nil {
		request.Pipes = primitives.ProcessPipeStdin
	}
	events := make(chan primitives.PrimitiveEvent, 8)
	process := primitives.StartProcess(ctx, request, events)
	var pid int
	var inputDone chan error
	var firstError error
	terminal := false
	for !terminal {
		event := <-events
		if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
			firstError = fmt.Errorf("unexpected event identity: %#v", event)
			cancel()
		}
		switch event.Type {
		case primitives.PrimitiveEventProcessStarted:
			if result.Started {
				firstError = fmt.Errorf("duplicate process.started")
				cancel()
			}
			result.Started = true
			pid = event.Result.(primitives.ProcessStartedResult).PID
			if input.Input != nil {
				inputDone = make(chan error, 1)
				go func() {
					controls := make(chan primitives.PrimitiveEvent, 1)
					process.WriteInput(ctx, primitives.ProcessWriteRequest{Data: []byte(*input.Input)}, controls)
					written := <-controls
					if written.Type != primitives.PrimitiveEventProcessInputWritten || written.Result.(primitives.ProcessWriteResult).Count != len(*input.Input) {
						inputDone <- fmt.Errorf("write fixture input: %#v", written)
						cancel()
						return
					}
					process.CloseInput(ctx, primitives.ProcessCloseInputRequest{}, controls)
					closed := <-controls
					if closed.Type != primitives.PrimitiveEventProcessInputClosed {
						inputDone <- fmt.Errorf("close fixture input: %#v", closed)
						cancel()
						return
					}
					inputDone <- nil
				}()
			}
			if input.CancelOnReady {
				deadline := time.Now().Add(3 * time.Second)
				for {
					bytes, readErr := os.ReadFile(out)
					if readErr != nil {
						firstError = readErr
						break
					}
					if string(bytes) == "ready" {
						break
					}
					if time.Now().After(deadline) {
						firstError = fmt.Errorf("fixture readiness deadline")
						break
					}
					runtime.Gosched()
				}
				cancel()
			}
		case primitives.PrimitiveEventProcessExited:
			exit := event.Result.(primitives.ProcessExitResult)
			result.Exited = true
			result.ExitCode = &exit.ExitCode
			signal := int(exit.Signal)
			result.Signal = &signal
			terminal = true
		case primitives.PrimitiveEventCanceled:
			result.Canceled = true
			terminal = true
		case primitives.PrimitiveEventFailed:
			result.Error = event.Result.(primitives.PrimitiveFailureResult).Error
			terminal = true
		default:
			firstError = fmt.Errorf("unexpected captured process event: %#v", event)
			cancel()
		}
	}
	if inputDone != nil {
		if inputErr := <-inputDone; inputErr != nil && firstError == nil {
			firstError = inputErr
		}
	}
	if firstError != nil {
		return result, firstError
	}
	// The terminal event follows wait, capture sync/close and all copier joins.
	result.GroupGone = pid == 0 || syscall.Kill(-pid, 0) == syscall.ESRCH
	for _, stream := range []struct {
		path string
		body *string
		mode *uint32
	}{
		{out, &result.StdoutHex, &result.OutMode}, {stderr, &result.StderrHex, &result.ErrMode},
	} {
		bytes, err := os.ReadFile(stream.path)
		if err != nil {
			return result, err
		}
		*stream.body = hex.EncodeToString(bytes)
		info, err := os.Stat(stream.path)
		if err != nil {
			return result, err
		}
		*stream.mode = uint32(info.Mode().Perm())
	}
	return result, nil
}

func main() {
	bytes, err := io.ReadAll(io.LimitReader(os.Stdin, 8<<20))
	if err != nil {
		panic(err)
	}
	var input spec
	if err := json.Unmarshal(bytes, &input); err != nil {
		panic(err)
	}
	result, err := run(input)
	if err != nil {
		panic(err)
	}
	bytes, err = json.Marshal(result)
	if err != nil {
		panic(err)
	}
	fmt.Println(string(bytes))
}
