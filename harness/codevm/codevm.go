// Package codevm runs the code the model writes in code mode: JavaScript in
// an isolated VM (goja, in this process) whose only capabilities are the
// functions that run the tools. The model writes the body of an async
// function; each function returns a promise, so calls run at once with
// Promise.all and their results are values the code goes on with. Every
// call the code makes is recorded, for the model to read and the cockpits
// to show under the code.
package codevm

import (
	"context"
	"errors"
	"fmt"
	"os"
	"regexp"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/dop251/goja"

	"github.com/gfhfyjbr/kou-conveyor/harness/fileops"
)

const (
	// DefaultMaxCalls bounds the tool calls one run makes.
	DefaultMaxCalls = 200
	// DefaultRunTimeout bounds one run, in seconds.
	DefaultRunTimeout = 30 * 60
	// DefaultCommandTimeout bounds one shell command of a run, in seconds.
	DefaultCommandTimeout = 10 * 60
	// DefaultMaxOutputLength bounds each stream a shell command leaves,
	// what the code logs and the value it returns.
	DefaultMaxOutputLength = 40_000
	// callOutputLimit is what the result shows of each call's output.
	callOutputLimit = 4000
	// recordLimit bounds what the record keeps of each call's arguments,
	// output and error: the code gets them whole, but the record goes with
	// every report of the run.
	recordLimit = 16_000
	// maxCommandOutput bounds the maxOutputLength a command may ask for.
	maxCommandOutput = 1_000_000
	// maxTextBytes bounds what readText returns.
	maxTextBytes = 4 << 20
	// reportInterval is the least time between two reports of a run.
	reportInterval = 500 * time.Millisecond
)

// Config is what a run may do, serializable so an operation keeps it.
type Config struct {
	// Shell runs bash(): shell -c command, in Directory, with Environment
	// (nil inherits the process's).
	Shell       string
	Directory   string
	Environment []string `json:",omitzero"`
	// MaxOutputLength bounds each stream of a shell command, what the code
	// logs and the value it returns.
	MaxOutputLength int `json:",omitzero"`
	// CommandTimeout bounds one shell command and RunTimeout the whole
	// run, in seconds.
	CommandTimeout float64 `json:",omitzero"`
	RunTimeout     float64 `json:",omitzero"`
	// MaxCalls bounds the tool calls of one run.
	MaxCalls int `json:",omitzero"`
	// Transcript is the session's transcript file, for transcriptSearch().
	Transcript string `json:",omitzero"`
	// Skills are the skills skill() loads: their SKILL.md by name.
	Skills map[string]string `json:",omitzero"`
	// Tools are the plugins' tools, run as commands with their arguments on
	// standard input.
	Tools []CommandTool `json:",omitzero"`
	// ViewImage reads images for viewImage(); nil leaves it out.
	ViewImage func(ctx context.Context, path string) (ImageResult, error) `json:"-"`
}

// CommandTool is a plugin's tool: a command that takes the call's
// arguments as JSON on its standard input and prints the result.
type CommandTool struct {
	Name        string
	Description string
	Parameters  map[string]any `json:",omitzero"`
	Command     []string
	Environment map[string]string `json:",omitzero"`
	Directory   string            `json:",omitzero"`
}

// ImageResult is a picture viewImage() read: a data URL and a description.
type ImageResult struct {
	DataURL     string
	Description string
}

// Call is one tool call the code made. The record keeps its arguments,
// output and error up to recordLimit each, cut in the middle (Truncated
// says the output was); the code got them whole.
type Call struct {
	Index     int
	Name      string
	Arguments string
	Output    string
	Error     string `json:",omitzero"`
	ExitCode  *int   `json:",omitzero"`
	Started   time.Time
	Finished  time.Time `json:",omitzero"`
	Image     string    `json:",omitzero"`
	Running   bool      `json:",omitzero"`
	Truncated bool      `json:",omitzero"`
}

// Result is what a run did: its calls, what it logged, what it returned,
// and what went wrong.
type Result struct {
	Calls    []Call
	Logs     string `json:",omitzero"`
	Value    string `json:",omitzero"`
	Error    string `json:",omitzero"`
	Started  time.Time
	Finished time.Time `json:",omitzero"`
	Done     bool      `json:",omitzero"`
}

// Text is what the model reads of the result: a line for every call, with
// how it ended, then what the code logged, the value it returned and any
// error. A call's output shows under it when the call failed, or when the
// code neither logged nor returned anything, so that the outputs are all
// it reports; otherwise the code reported what it needed, and the outputs
// would only repeat it.
func (result Result) Text() string {
	var text strings.Builder
	duration := ""
	if !result.Finished.IsZero() {
		duration = " · " + shortDuration(result.Finished.Sub(result.Started))
	}
	fmt.Fprintf(&text, "[CODE] %s%s\n", plural(len(result.Calls), "tool call", "tool calls"), duration)
	reported := result.Error == "" && (strings.TrimSpace(result.Logs) != "" || result.Value != "")
	omitted := 0
	for _, call := range result.Calls {
		failed := call.ExitCode != nil && *call.ExitCode != 0
		var meta []string
		if failed {
			meta = append(meta, fmt.Sprintf("exit %d", *call.ExitCode))
		}
		if !call.Finished.IsZero() {
			meta = append(meta, shortDuration(call.Finished.Sub(call.Started)))
		}
		header := "- " + call.Name + ": " + argumentLine(call)
		if len(meta) != 0 {
			header += " (" + strings.Join(meta, ", ") + ")"
		}
		text.WriteString(header + "\n")
		var body string
		switch {
		case call.Running:
			body = "(still running)"
		case call.Error != "":
			body = "Error: " + call.Error
		case reported && !failed:
			omitted++
			continue
		case strings.TrimSpace(call.Output) == "":
			body = "(no output)"
		default:
			body = call.Output
		}
		for _, line := range strings.Split(strings.TrimRight(clip(body, callOutputLimit), "\n"), "\n") {
			text.WriteString("   " + line + "\n")
		}
	}
	if omitted != 0 {
		text.WriteString("(The outputs of the calls that succeeded are left out: the code logged or returned what it needed.)\n")
	}
	if result.Logs != "" {
		text.WriteString("console:\n")
		for _, line := range strings.Split(strings.TrimRight(result.Logs, "\n"), "\n") {
			text.WriteString("   " + line + "\n")
		}
	}
	if result.Value != "" {
		text.WriteString("Return value:\n" + result.Value + "\n")
	}
	if result.Error != "" {
		text.WriteString("Error: " + result.Error + "\n")
	}
	return strings.TrimRight(text.String(), "\n")
}

// argumentLine is what the line of a call shows of its arguments: their
// first line, or the files a patch changes.
func argumentLine(call Call) string {
	if call.Name == "applyPatch" {
		var files []string
		for _, line := range strings.Split(call.Arguments, "\n") {
			line = strings.TrimSpace(line)
			for _, action := range []string{"Add", "Update", "Delete"} {
				if name, ok := strings.CutPrefix(line, "*** "+action+" File: "); ok {
					files = append(files, action+" "+strings.TrimSpace(name))
				}
			}
		}
		if len(files) != 0 {
			return firstLine(strings.Join(files, ", "))
		}
	}
	return firstLine(call.Arguments)
}

// Images are the pictures the run's viewImage() calls read, in order.
func (result Result) Images() []string {
	var images []string
	for _, call := range result.Calls {
		if call.Image != "" {
			images = append(images, call.Image)
		}
	}
	return images
}

// Report hears the result as it grows: as calls start and end, at most
// every reportInterval, and without the images, which the result brings
// once, at the end.
type Report func(Result)

// runtime is one run.
type runtime struct {
	config  Config
	vm      *goja.Runtime
	parent  context.Context
	ctx     context.Context
	cancel  context.CancelFunc
	timeout time.Duration
	jobs    chan func()
	// stringify is the VM's JSON.stringify, kind names the kind of a value
	// and special shows what JSON does not (maps, sets, promises); all are
	// taken before the code runs, which cannot change them.
	stringify goja.Callable
	kind      goja.Callable
	special   goja.Callable

	mu      sync.Mutex
	result  Result
	pending int // the calls and sleeps whose promises are to settle
	logs    *boundedBuffer
	done    bool

	report    Report
	reporting sync.Mutex // one report at a time, in order
	reported  time.Time
	timer     *time.Timer
	stopped   bool
}

// Run runs code with the functions config allows and returns what it did.
// The run ends when the code's promise settles, when nothing is left that
// could settle it, when the context ends or when RunTimeout passes.
func Run(ctx context.Context, config Config, code string, report Report) Result {
	if config.MaxCalls <= 0 {
		config.MaxCalls = DefaultMaxCalls
	}
	if config.RunTimeout <= 0 {
		config.RunTimeout = DefaultRunTimeout
	}
	if config.CommandTimeout <= 0 {
		config.CommandTimeout = DefaultCommandTimeout
	}
	if config.MaxOutputLength <= 0 {
		config.MaxOutputLength = DefaultMaxOutputLength
	}
	runTimeout := seconds(config.RunTimeout)
	runContext, cancel := context.WithTimeout(ctx, runTimeout)
	defer cancel()
	current := &runtime{
		config: config, vm: goja.New(), parent: ctx, ctx: runContext, cancel: cancel, timeout: runTimeout,
		jobs: make(chan func(), 64), report: report,
		result: Result{Started: time.Now()}, logs: newBoundedBuffer(config.MaxOutputLength),
	}
	current.vm.SetMaxCallStackSize(2000)
	current.install()

	// The VM is interrupted once the run is stopped, and again while the
	// loop still has it: code the runtime calls on the way out, a toJSON
	// of the value returned say, must not hold it either.
	loopDone := make(chan struct{})
	go func() {
		select {
		case <-runContext.Done():
		case <-loopDone:
			return
		}
		ticker := time.NewTicker(100 * time.Millisecond)
		defer ticker.Stop()
		for {
			current.vm.Interrupt(current.stopMessage())
			select {
			case <-loopDone:
				return
			case <-ticker.C:
			}
		}
	}()
	var promise *goja.Promise
	current.guard(func() {
		value, err := current.vm.RunString("(async () => {" + code + "\n})()")
		if err != nil {
			current.finish("", describeError(err))
			return
		}
		promise, _ = value.Export().(*goja.Promise)
	})
	for !current.finished() {
		current.guard(func() { current.settle(promise) })
		if current.finished() {
			break
		}
		if current.stalled() {
			current.finish("", "the code waits for a promise that nothing will settle: none of the calls it made is still running")
			break
		}
		select {
		case job := <-current.jobs:
			current.guard(job)
		case <-runContext.Done():
			current.finish("", current.stopMessage())
		}
	}
	close(loopDone)
	cancel()
	current.stopReports()
	return current.snapshot()
}

// seconds is a duration given in seconds.
func seconds(value float64) time.Duration {
	return time.Duration(value * float64(time.Second))
}

// stopMessage says why the run was stopped.
func (current *runtime) stopMessage() string {
	return "the run was stopped: " + reason(current.parent, current.ctx, current.timeout)
}

func reason(parent, run context.Context, timeout time.Duration) string {
	if parent.Err() != nil {
		return "it was canceled"
	}
	if errors.Is(run.Err(), context.DeadlineExceeded) {
		return fmt.Sprintf("it ran longer than %s", timeout)
	}
	if run.Err() == nil {
		return "it ended"
	}
	return run.Err().Error()
}

// describeError is a JavaScript error as the model reads it, with the line
// numbers of its code: the wrapper shares its first line, so they match.
func describeError(err error) string {
	var exception *goja.Exception
	if errors.As(err, &exception) {
		return exception.String()
	}
	var interrupted *goja.InterruptedError
	if errors.As(err, &interrupted) {
		return fmt.Sprint(interrupted.Value())
	}
	return err.Error()
}

// guard runs f on the loop and ends the run when it panics: a fault of the
// runtime fails the run, not the process.
func (current *runtime) guard(f func()) {
	defer func() {
		if x := recover(); x != nil {
			if current.ctx.Err() != nil {
				current.finish("", current.stopMessage())
				return
			}
			current.finish("", fmt.Sprintf("internal error in the code runtime: %v", x))
		}
	}()
	f()
}

// settle ends the run once the code's promise settled: with the value the
// code returned, or with what it threw.
func (current *runtime) settle(promise *goja.Promise) {
	if promise == nil {
		return
	}
	var value, failure string
	switch promise.State() {
	case goja.PromiseStatePending:
		return
	case goja.PromiseStateFulfilled:
		if result := promise.Result(); result != nil && !goja.IsUndefined(result) {
			value = current.format(result, "  ")
		}
	case goja.PromiseStateRejected:
		failure = current.thrown(promise.Result())
	}
	if current.ctx.Err() != nil {
		// Showing what the code returned or threw ran code of its own, a
		// toJSON say, past the end of the run.
		value, failure = "", current.stopMessage()
	}
	current.finish(value, failure)
}

func (current *runtime) finished() bool {
	current.mu.Lock()
	defer current.mu.Unlock()
	return current.done
}

// stalled reports code that waits for what will not come: its promise has
// not settled, and no call it made is still running to settle it.
func (current *runtime) stalled() bool {
	current.mu.Lock()
	defer current.mu.Unlock()
	return !current.done && current.pending == 0
}

func (current *runtime) finish(value, failure string) {
	current.mu.Lock()
	if current.done {
		current.mu.Unlock()
		return
	}
	current.done = true
	result := &current.result
	result.Done = true
	result.Value = clip(value, current.config.MaxOutputLength)
	result.Error = clip(failure, current.config.MaxOutputLength)
	result.Finished = time.Now()
	result.Logs = current.logs.String()
	for index := range result.Calls {
		if result.Calls[index].Running {
			result.Calls[index].Running = false
			result.Calls[index].Error = "the run ended before it finished"
			result.Calls[index].Finished = result.Finished
		}
	}
	current.mu.Unlock()
	current.cancel()
}

func (current *runtime) snapshot() Result {
	current.mu.Lock()
	defer current.mu.Unlock()
	result := current.result
	result.Calls = slices.Clone(current.result.Calls)
	if !result.Done {
		result.Logs = current.logs.String()
	}
	return result
}

// progress is the result so far, as a report carries it: without the
// images.
func (current *runtime) progress() Result {
	result := current.snapshot()
	for index := range result.Calls {
		result.Calls[index].Image = ""
	}
	return result
}

// notify reports the result, at most every reportInterval: a report due
// sooner waits for the interval, and carries what happened meanwhile.
func (current *runtime) notify() {
	if current.report == nil {
		return
	}
	current.reporting.Lock()
	defer current.reporting.Unlock()
	if current.stopped || current.timer != nil {
		return
	}
	if wait := reportInterval - time.Since(current.reported); wait > 0 {
		current.timer = time.AfterFunc(wait, func() {
			current.reporting.Lock()
			defer current.reporting.Unlock()
			current.timer = nil
			if !current.stopped {
				current.send()
			}
		})
		return
	}
	current.send()
}

// send reports the result now; reporting is held.
func (current *runtime) send() {
	current.reported = time.Now()
	current.report(current.progress())
}

// stopReports ends the reports: none is made once it returns.
func (current *runtime) stopReports() {
	current.reporting.Lock()
	defer current.reporting.Unlock()
	current.stopped = true
	if current.timer != nil {
		current.timer.Stop()
		current.timer = nil
	}
}

// kindScript names the kind of a value, for the errors of the functions.
const kindScript = `(value) => value === null ? "null" : Array.isArray(value) ? "an array" : ({ undefined: "undefined", object: "an object", function: "a function", number: "a number", boolean: "a boolean", bigint: "a bigint", symbol: "a symbol", string: "a string" })[typeof value]`

// specialScript shows the values JSON does not, and is undefined for the
// others.
const specialScript = `((M, S, P, stringify, from) => (value) => value instanceof M ? "Map " + stringify(from(value)) : value instanceof S ? "Set " + stringify(from(value)) : value instanceof P ? "[a Promise: await it for its value]" : undefined)(Map, Set, Promise, JSON.stringify, Array.from)`

// editNames names the options in fileops.Edit's errors as edit() takes them.
var editNames = strings.NewReplacer(
	"old_string", "oldString", "new_string", "newString",
	"set replace_all", "pass { replaceAll: true }",
	"Write creates", "write() creates", "as Read shows", "as read() shows", "; use Bash", "; use bash()",
)

// install binds the functions the code may call.
func (current *runtime) install() {
	vm := current.vm
	current.stringify, _ = goja.AssertFunction(vm.Get("JSON").ToObject(vm).Get("stringify"))
	kind, _ := vm.RunString(kindScript)
	current.kind, _ = goja.AssertFunction(kind)
	special, _ := vm.RunString(specialScript)
	current.special, _ = goja.AssertFunction(special)
	console := vm.NewObject()
	log := func(call goja.FunctionCall) goja.Value {
		parts := make([]string, len(call.Arguments))
		for index, argument := range call.Arguments {
			parts[index] = current.format(argument, "")
		}
		current.logs.Write([]byte(strings.Join(parts, " ") + "\n"))
		return goja.Undefined()
	}
	for _, name := range []string{"log", "info", "warn", "error", "debug"} {
		console.Set(name, log)
	}
	vm.Set("console", console)
	vm.Set("sleep", func(call goja.FunctionCall) goja.Value {
		delay := time.Duration(call.Argument(0).ToFloat() * float64(time.Millisecond))
		return current.async("", "", func(ctx context.Context) (string, *int, string, error) {
			select {
			case <-time.After(max(delay, 0)):
			case <-ctx.Done():
				return "", nil, "", ctx.Err()
			}
			return "", nil, "", nil
		}, func(string) goja.Value { return goja.Undefined() })
	})
	vm.Set("bash", func(call goja.FunctionCall) goja.Value {
		command := current.stringArgument(call, 0, "bash", "the command")
		options := current.options(call, 1, "bash", "timeout", "maxOutputLength")
		timeout := seconds(current.config.CommandTimeout)
		if value, ok := numberOf(options, "timeout"); ok && value > 0 {
			timeout = seconds(value)
		}
		limit := current.config.MaxOutputLength
		if n, ok := numberOf(options, "maxOutputLength"); ok && n > 0 {
			limit = int(min(n, maxCommandOutput))
		}
		var last commandResult
		return current.async("bash", command, func(ctx context.Context) (string, *int, string, error) {
			result, err := runCommand(ctx, current.config.Shell, current.config.Directory, command, current.config.Environment, limit, timeout)
			if err != nil {
				return "", nil, "", err
			}
			last = result
			code := result.ExitCode
			return shellText(result), &code, "", nil
		}, func(string) goja.Value {
			object := vm.NewObject()
			object.Set("stdout", last.Stdout)
			object.Set("stderr", last.Stderr)
			object.Set("exitCode", last.ExitCode)
			object.Set("timedOut", last.TimedOut)
			return object
		})
	})
	vm.Set("read", func(call goja.FunctionCall) goja.Value {
		path := current.resolve(current.stringArgument(call, 0, "read", "the path"))
		options := current.options(call, 1, "read", "offset", "limit")
		offset, _ := numberOf(options, "offset")
		limit, _ := numberOf(options, "limit")
		return current.text("read", path, func(context.Context) (string, error) {
			result, err := fileops.Read(path, fileops.ReadOptions{Offset: int(offset), Limit: int(limit)})
			return result.Text, err
		})
	})
	vm.Set("readText", func(call goja.FunctionCall) goja.Value {
		path := current.resolve(current.stringArgument(call, 0, "readText", "the path"))
		return current.text("readText", path, func(context.Context) (string, error) {
			info, err := os.Stat(path)
			if err != nil {
				return "", err
			}
			if info.Size() > maxTextBytes {
				return "", fmt.Errorf("%s is %d bytes; readText reads up to %d", path, info.Size(), maxTextBytes)
			}
			data, err := os.ReadFile(path)
			return string(data), err
		})
	})
	vm.Set("write", func(call goja.FunctionCall) goja.Value {
		path := current.resolve(current.stringArgument(call, 0, "write", "the path"))
		content := current.stringArgument(call, 1, "write", "the content")
		return current.text("write", path, func(context.Context) (string, error) {
			result, err := fileops.Write(path, content)
			if err != nil {
				return "", err
			}
			if result.Created {
				return fmt.Sprintf("Wrote %s: %s, %d bytes.", path, plural(result.Lines, "line", "lines"), result.Bytes), nil
			}
			return fmt.Sprintf("Replaced %s (it had %s): now %s, %d bytes.", path, plural(result.PreviousLines, "line", "lines"), plural(result.Lines, "line", "lines"), result.Bytes), nil
		})
	})
	vm.Set("edit", func(call goja.FunctionCall) goja.Value {
		path := current.resolve(current.stringArgument(call, 0, "edit", "the path"))
		old := current.stringArgument(call, 1, "edit", "oldString")
		replacement := current.stringArgument(call, 2, "edit", "newString")
		replaceAll := booleanOf(current.options(call, 3, "edit", "replaceAll"), "replaceAll")
		return current.text("edit", path, func(context.Context) (string, error) {
			result, err := fileops.Edit(path, old, replacement, fileops.EditOptions{ReplaceAll: replaceAll})
			if err != nil {
				return "", errors.New(editNames.Replace(err.Error()))
			}
			replaced := fmt.Sprintf("replaced 1 occurrence at line %d", result.Line)
			if result.Replacements != 1 {
				replaced = fmt.Sprintf("replaced %d occurrences, the first at line %d", result.Replacements, result.Line)
			}
			return fmt.Sprintf("Edited %s: %s.\n%s", path, replaced, result.Snippet), nil
		})
	})
	vm.Set("applyPatch", func(call goja.FunctionCall) goja.Value {
		patch := current.stringArgument(call, 0, "applyPatch", "the patch")
		return current.text("applyPatch", patch, func(context.Context) (string, error) {
			result, err := fileops.ApplyPatch(patch, current.config.Directory)
			if err != nil {
				return "", err
			}
			return result.Summary(), nil
		})
	})
	vm.Set("skill", func(call goja.FunctionCall) goja.Value {
		name := current.stringArgument(call, 0, "skill", "the name")
		return current.text("skill", name, func(context.Context) (string, error) {
			path, ok := current.config.Skills[name]
			if !ok {
				names := make([]string, 0, len(current.config.Skills))
				for name := range current.config.Skills {
					names = append(names, name)
				}
				sort.Strings(names)
				return "", fmt.Errorf("skill %q is not registered; the skills are: %s", name, strings.Join(names, ", "))
			}
			data, err := os.ReadFile(path)
			return strings.ToValidUTF8(string(data), "\uFFFD"), err
		})
	})
	vm.Set("transcriptSearch", func(call goja.FunctionCall) goja.Value {
		query := current.stringArgument(call, 0, "transcriptSearch", "the query")
		limit, _ := numberOf(current.options(call, 1, "transcriptSearch", "limit"), "limit")
		return current.text("transcriptSearch", query, func(context.Context) (string, error) {
			if current.config.Transcript == "" {
				return "", errors.New("no transcript is available")
			}
			return SearchTranscript(current.config.Transcript, query, int(limit))
		})
	})
	vm.Set("viewImage", func(call goja.FunctionCall) goja.Value {
		path := current.resolve(current.stringArgument(call, 0, "viewImage", "the path"))
		return current.async("viewImage", path, func(ctx context.Context) (string, *int, string, error) {
			if current.config.ViewImage == nil {
				return "", nil, "", errors.New("viewImage is not available in this run")
			}
			result, err := current.config.ViewImage(ctx, path)
			if err != nil {
				return "", nil, "", err
			}
			return "Image attached to the result: " + result.Description, nil, result.DataURL, nil
		}, func(output string) goja.Value { return vm.ToValue(output) })
	})
	tools := vm.NewObject()
	for _, definition := range current.config.Tools {
		function := func(call goja.FunctionCall) goja.Value {
			arguments := "{}"
			if argument := call.Argument(0); !goja.IsUndefined(argument) && !goja.IsNull(argument) {
				object, ok := argument.(*goja.Object)
				if !ok || object.ClassName() != "Object" {
					panic(vm.NewTypeError("%s: the arguments must be an object, not %s", definition.Name, current.kindOf(argument)))
				}
				encoded, err := current.stringify(goja.Undefined(), object)
				if err != nil {
					panic(vm.NewTypeError("%s: the arguments must be JSON: %s", definition.Name, describeError(err)))
				}
				arguments = encoded.String()
			}
			return current.async(definition.Name, arguments, func(ctx context.Context) (string, *int, string, error) {
				result, err := runCommand(ctx, "/bin/sh", cmpOr(definition.Directory, current.config.Directory),
					commandScript(definition, arguments), commandEnvironment(current.config.Environment, definition), current.config.MaxOutputLength, seconds(current.config.CommandTimeout))
				if err != nil {
					return "", nil, "", err
				}
				code := result.ExitCode
				output := shellText(result)
				if result.ExitCode != 0 || result.TimedOut {
					return output, &code, "", fmt.Errorf("%s failed:\n%s", definition.Name, clip(strings.TrimSpace(output), callOutputLimit))
				}
				return output, &code, "", nil
			}, func(output string) goja.Value { return vm.ToValue(output) })
		}
		tools.Set(definition.Name, function)
		if bindable(definition.Name) {
			vm.Set(definition.Name, function)
		}
	}
	vm.Set("tools", tools)
}

var identifier = regexp.MustCompile(`^[A-Za-z_$][A-Za-z0-9_$]*$`)

// builtins are the names the runtime gives the code, and reserved the
// words of JavaScript that cannot name a function it calls.
var (
	builtins = []string{"bash", "read", "readText", "write", "edit", "applyPatch", "viewImage", "transcriptSearch", "sleep", "skill", "console", "tools"}
	reserved = strings.Fields("await break case catch class const continue debugger default delete do else enum export extends false finally for function if implements import in instanceof interface let new null package private protected public return static super switch this throw true try typeof var void while with yield")
)

// javaScriptGlobals are the names JavaScript itself defines: JSON, Math,
// Promise and the like.
var javaScriptGlobals = sync.OnceValue(func() map[string]bool {
	globals := map[string]bool{}
	for _, name := range goja.New().GlobalObject().GetOwnPropertyNames() {
		globals[name] = true
	}
	return globals
})

// bindable reports a plugin tool name the code may call as a function of
// its own: an identifier that is not a reserved word and names nothing
// else. Every tool is tools[name] too.
func bindable(name string) bool {
	return identifier.MatchString(name) && !slices.Contains(reserved, name) && !slices.Contains(builtins, name) && !javaScriptGlobals()[name]
}

// stringArgument is argument index of a call, which must be a string.
func (current *runtime) stringArgument(call goja.FunctionCall, index int, function, name string) string {
	value := call.Argument(index)
	if !goja.IsString(value) {
		panic(current.vm.NewTypeError("%s: %s must be a string, not %s", function, name, current.kindOf(value)))
	}
	return value.String()
}

// options is the options argument index of a call: none, or an object
// whose keys are known.
func (current *runtime) options(call goja.FunctionCall, index int, function string, known ...string) *goja.Object {
	value := call.Argument(index)
	if goja.IsUndefined(value) || goja.IsNull(value) {
		return nil
	}
	object, ok := value.(*goja.Object)
	if !ok || object.ClassName() != "Object" {
		panic(current.vm.NewTypeError("%s: the options must be an object, not %s", function, current.kindOf(value)))
	}
	for _, key := range object.Keys() {
		if !slices.Contains(known, key) {
			panic(current.vm.NewTypeError("%s: unknown option %q; the options are %s", function, key, strings.Join(known, ", ")))
		}
	}
	return object
}

// kindOf names the kind of a value: "a number", "an array", "undefined".
func (current *runtime) kindOf(value goja.Value) string {
	if kind, err := current.kind(goja.Undefined(), value); err == nil && goja.IsString(kind) {
		return kind.String()
	}
	return "a value of another kind"
}

// format is a value as the model reads it: a string as it is, an error
// with its stack, anything else as JSON (indented by indent), or as its
// kind when it is not JSON.
func (current *runtime) format(value goja.Value, indent string) string {
	object, isObject := value.(*goja.Object)
	if !isObject {
		if value == nil {
			return "undefined"
		}
		return value.String()
	}
	switch object.ClassName() {
	case "Error":
		return errorText(object)
	case "Function":
		return "[Function]"
	}
	if text, err := current.special(goja.Undefined(), object); err == nil && goja.IsString(text) {
		return text.String()
	}
	encoded, err := current.stringify(goja.Undefined(), object, goja.Null(), current.vm.ToValue(indent))
	switch {
	case err != nil:
		return fmt.Sprintf("[%s, not JSON: %s]", object.ClassName(), errorMessage(err))
	case encoded == nil || goja.IsUndefined(encoded):
		return "[" + object.ClassName() + "]"
	}
	return encoded.String()
}

// thrown is what the code threw, as the model reads it.
func (current *runtime) thrown(value goja.Value) string {
	if object, ok := value.(*goja.Object); ok && object.ClassName() == "Error" {
		return errorText(object)
	}
	if value != nil && goja.IsString(value) {
		return value.String()
	}
	return "uncaught " + current.format(value, "")
}

// errorText is an error as the model reads it: its stack, which starts
// with its message, without the frames of the runtime's functions; the
// error of a call is its message.
func errorText(object *goja.Object) string {
	if name := object.Get("name"); name != nil && name.String() == "GoError" {
		return object.Get("message").String()
	}
	stack := object.Get("stack")
	if stack == nil || !goja.IsString(stack) || stack.String() == "" {
		return object.String()
	}
	lines := strings.Split(stack.String(), "\n")
	kept := lines[:0]
	for _, line := range lines {
		if !strings.HasSuffix(line, "(native)") {
			kept = append(kept, line)
		}
	}
	return strings.Join(kept, "\n")
}

// errorMessage is the message of an error a JavaScript function returned.
func errorMessage(err error) string {
	var exception *goja.Exception
	if errors.As(err, &exception) {
		if object, ok := exception.Value().(*goja.Object); ok && object.ClassName() == "Error" {
			return object.String()
		}
	}
	return describeError(err)
}

// text is an async call whose result is text, or an error.
func (current *runtime) text(name, arguments string, run func(context.Context) (string, error)) goja.Value {
	return current.async(name, arguments, func(ctx context.Context) (string, *int, string, error) {
		output, err := run(ctx)
		return output, nil, "", err
	}, func(output string) goja.Value { return current.vm.ToValue(output) })
}

// async records a call, runs it in the background and returns a promise of
// its value: value makes it of the output once the call is done. A call
// without a name (a sleep) is not recorded.
func (current *runtime) async(name, arguments string, run func(context.Context) (string, *int, string, error), value func(output string) goja.Value) goja.Value {
	vm := current.vm
	promise, resolve, reject := vm.NewPromise()
	current.mu.Lock()
	if name != "" && len(current.result.Calls) >= current.config.MaxCalls {
		current.mu.Unlock()
		reject(vm.NewGoError(fmt.Errorf("the run made %d tool calls, the most a run may make", current.config.MaxCalls)))
		return vm.ToValue(promise)
	}
	index := -1
	if name != "" {
		index = len(current.result.Calls)
		current.result.Calls = append(current.result.Calls, Call{
			Index: index, Name: name, Arguments: clip(arguments, recordLimit), Started: time.Now(), Running: true,
		})
	}
	current.pending++
	current.mu.Unlock()
	if index >= 0 {
		current.notify()
	}
	go func() {
		output, exitCode, image, err := protect(current.ctx, run)
		current.mu.Lock()
		done := current.done
		if index >= 0 && !done {
			call := &current.result.Calls[index]
			call.Finished = time.Now()
			call.Running = false
			call.Output, call.Truncated = clip(output, recordLimit), len(output) > recordLimit
			call.ExitCode, call.Image = exitCode, image
			if err != nil {
				call.Error = clip(err.Error(), recordLimit)
			}
		}
		current.mu.Unlock()
		if done {
			return
		}
		if index >= 0 {
			current.notify()
		}
		// The promise settles on the loop, which the VM belongs to.
		job := func() {
			current.mu.Lock()
			current.pending--
			current.mu.Unlock()
			var failure error
			if err != nil {
				failure = reject(vm.NewGoError(err))
			} else {
				failure = resolve(value(output))
			}
			if failure != nil {
				current.finish("", describeError(failure))
			}
		}
		select {
		case current.jobs <- job:
		case <-current.ctx.Done():
		}
	}()
	return vm.ToValue(promise)
}

// protect runs the work of a call and turns a panic into its error: a
// fault of a tool fails the call, not the process.
func protect(ctx context.Context, run func(context.Context) (string, *int, string, error)) (output string, exitCode *int, image string, err error) {
	defer func() {
		if x := recover(); x != nil {
			err = fmt.Errorf("internal error: %v", x)
		}
	}()
	return run(ctx)
}

func (current *runtime) resolve(path string) string {
	if path == "" || current.config.Directory == "" || strings.HasPrefix(path, "/") {
		return path
	}
	return current.config.Directory + "/" + path
}

// shellText is what a shell command's result reads as: its streams and a
// nonzero exit code.
func shellText(result commandResult) string {
	var parts []string
	if result.Stdout != "" {
		parts = append(parts, result.Stdout)
	}
	if result.Stderr != "" {
		parts = append(parts, "Stderr:\n"+result.Stderr)
	}
	if result.ExitCode != 0 {
		parts = append(parts, fmt.Sprintf("Exit code: %d", result.ExitCode))
	}
	if result.TimedOut {
		parts = append(parts, "The command timed out and was stopped.")
	}
	return strings.Join(parts, "\n")
}

func numberOf(object *goja.Object, key string) (float64, bool) {
	if object == nil {
		return 0, false
	}
	value := object.Get(key)
	if value == nil || goja.IsUndefined(value) || goja.IsNull(value) {
		return 0, false
	}
	return value.ToFloat(), true
}

func booleanOf(object *goja.Object, key string) bool {
	if object == nil {
		return false
	}
	value := object.Get(key)
	return value != nil && value.ToBoolean()
}

func firstLine(text string) string {
	line, _, _ := strings.Cut(strings.TrimSpace(text), "\n")
	if len(line) > 160 {
		cut := 157
		for cut > 0 && !utf8.RuneStart(line[cut]) {
			cut--
		}
		line = line[:cut] + "…"
	}
	return line
}

// clip cuts text longer than limit bytes in the middle, keeping two thirds
// of limit from its start and a third from its end, whole runes.
func clip(text string, limit int) string {
	if len(text) <= limit {
		return text
	}
	head, tail := limit*2/3, len(text)-limit/3
	for head > 0 && !utf8.RuneStart(text[head]) {
		head--
	}
	for tail < len(text) && !utf8.RuneStart(text[tail]) {
		tail++
	}
	return text[:head] + fmt.Sprintf("\n[… %d bytes left out …]\n", tail-head) + text[tail:]
}

func plural(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return fmt.Sprintf("%d %s", n, many)
}

func shortDuration(d time.Duration) string {
	switch {
	case d < time.Second:
		return fmt.Sprintf("%d ms", d.Milliseconds())
	case d < time.Minute:
		return fmt.Sprintf("%.1f s", d.Seconds())
	}
	return d.Round(time.Second).String()
}

func cmpOr(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}
