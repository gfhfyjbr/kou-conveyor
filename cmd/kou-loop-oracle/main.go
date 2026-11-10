// Command kou-loop-oracle drives the actual Go coordinator with scripted,
// inert model/operation ports. Its store is always an owned temporary session.
package main

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/coordinator"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type config struct {
	ToolGraceMS       int64
	ToolWaitLimitMS   int64
	AutoCompactTokens int64
	ContextWindow     int64
	NoRecovery        bool
	MaxTurns          int
	MaxCompactions    int
}
type fault struct {
	Kind         string
	Message      string
	RetryAfterMS int64
	Tokens       int64
	Limit        int64
}
type action struct {
	Kind     string
	Input    inbox.Input
	Response llm.Response
	Fault    *fault
	Index    int
	Count    int
	ID       string
	Status   operation.Status
	State    jsontext.Value
	Fail     string
}
type spec struct {
	Actions  []action
	Config   config
	Model    llm.Model
	System   string
	Tools    []llm.Tool
	Verifier bool
}
type entry struct {
	Kind  string
	Value jsontext.Value
}
type trace struct {
	sync.Mutex
	entries []entry
	changed chan struct{}
}

func (t *trace) push(kind string, value any) {
	encoded, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	t.Lock()
	t.entries = append(t.entries, entry{kind, encoded})
	t.Unlock()
	select {
	case t.changed <- struct{}{}:
	default:
	}
}
func (t *trace) snapshot() []entry {
	t.Lock()
	defer t.Unlock()
	return append([]entry{}, t.entries...)
}
func (t *trace) inputRecorded(id string) bool {
	for _, item := range t.snapshot() {
		if item.Kind != "commit" {
			continue
		}
		var value struct {
			Kind string
			Data struct{ ID string }
		}
		if err := json.Unmarshal(item.Value, &value); err != nil {
			panic(err)
		}
		if value.Kind == "input" && value.Data.ID == id {
			return true
		}
	}
	return false
}

type store struct {
	sessionstore.Store
	trace *trace
	mu    sync.Mutex
	fail  string
}

func (s *store) fails(kind string, data any) error {
	s.mu.Lock()
	failure := s.fail
	s.mu.Unlock()
	if failure != kind {
		return nil
	}
	s.trace.push("write_failure", map[string]any{"Kind": kind, "Data": data})
	return fmt.Errorf("%s failed", kind)
}
func (s *store) AppendInput(ctx context.Context, id session.ID, input inbox.Input) error {
	if err := s.fails("input", input); err != nil {
		return err
	}
	return s.Store.AppendInput(ctx, id, input)
}
func (s *store) AppendTurn(ctx context.Context, id session.ID, turn session.Turn) error {
	if err := s.fails("turn", turn); err != nil {
		return err
	}
	return s.Store.AppendTurn(ctx, id, turn)
}
func (s *store) AppendModelResponse(ctx context.Context, id session.ID, response sessionstore.ModelResponse) error {
	if err := s.fails("response", response); err != nil {
		return err
	}
	return s.Store.AppendModelResponse(ctx, id, response)
}
func (s *store) AppendToolCallStatus(ctx context.Context, id session.ID, status sessionstore.ToolCallStatus) error {
	if err := s.fails("status", status); err != nil {
		return err
	}
	return s.Store.AppendToolCallStatus(ctx, id, status)
}
func (s *store) SaveOperation(ctx context.Context, id session.ID, value operation.Operation) error {
	if err := s.fails("update", value); err != nil {
		return err
	}
	if err := s.Store.SaveOperation(ctx, id, value); err != nil {
		return err
	}
	s.trace.push("update", value)
	return nil
}
func (s *store) SaveCheckpoint(ctx context.Context, id session.ID, state jsontext.Value) error {
	if err := s.fails("checkpoint", map[string]any{}); err != nil {
		return err
	}
	if err := s.Store.(sessionstore.Checkpointer).SaveCheckpoint(ctx, id, state); err != nil {
		return err
	}
	s.trace.push("checkpoint", map[string]any{})
	return nil
}

type modelResult struct {
	response llm.Response
	err      error
}
type request struct {
	ctx   context.Context
	reply chan modelResult
	done  chan struct{}
}
type model struct {
	trace    *trace
	requests chan *request
}

func (m *model) Respond(ctx context.Context, input llm.Request, options llm.RequestOptions) (llm.Response, error) {
	if err := ctx.Err(); err != nil {
		return llm.Response{}, err
	}
	call := &request{ctx: ctx, reply: make(chan modelResult, 1), done: make(chan struct{})}
	defer close(call.done)
	m.trace.push("request", map[string]any{"Request": input, "CacheKey": options.CacheKey})
	select {
	case m.requests <- call:
	case <-ctx.Done():
		return llm.Response{}, ctx.Err()
	}
	select {
	case result := <-call.reply:
		return result.response, result.err
	case <-ctx.Done():
		return llm.Response{}, ctx.Err()
	}
}

type manager struct {
	trace   *trace
	mu      sync.Mutex
	adds    []operation.Operation
	cancels []operation.ID
	changed chan struct{}
	updates chan operation.Operation
}

func (m *manager) Add(value operation.Operation) error {
	m.trace.push("dispatch", value)
	m.mu.Lock()
	m.adds = append(m.adds, value)
	m.mu.Unlock()
	select {
	case m.changed <- struct{}{}:
	default:
	}
	return nil
}
func (m *manager) Cancel(id operation.ID, reason string) error {
	m.trace.push("cancel", map[string]any{"ID": id, "Reason": reason})
	m.mu.Lock()
	m.cancels = append(m.cancels, id)
	m.mu.Unlock()
	select {
	case m.changed <- struct{}{}:
	default:
	}
	return nil
}
func (m *manager) Updates() <-chan operation.Operation { return m.updates }
func (m *manager) count(cancel bool) int {
	m.mu.Lock()
	defer m.mu.Unlock()
	if cancel {
		return len(m.cancels)
	}
	return len(m.adds)
}
func (m *manager) operation(index int) (operation.Operation, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if index < 0 || index >= len(m.adds) {
		return operation.Operation{}, errors.New("operation index out of range")
	}
	return m.adds[index], nil
}

type registry struct{ tool.Registry }
type translator struct{ name string }

func (registry) StaticDefinitions() []tool.Definition { return nil }
func (registry) Resolve(name string) (tool.Translator, bool) {
	switch name {
	case "Bash", "Write", "Wait", "Immediate", "Reject":
		return translator{name}, true
	default:
		return nil, false
	}
}
func (t translator) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	if t.name == "Immediate" {
		return tool.CallStatus{}
	}
	count, err := strconv.Atoi(call.Arguments)
	if err != nil {
		count = 1
	}
	count = min(max(count, 0), 8)
	ids := make([]operation.ID, 0, count)
	for range count {
		ids = append(ids, ctx.Submit(operation.Spec{Type: operation.TypeValue, Version: operation.VersionValue, State: []byte(`{"Value":0}`)}))
	}
	return tool.CallStatus{WaitingFor: ids}
}
func (t translator) TranslateResult(id string, status tool.CallStatus, values []operation.Operation) (llm.ToolResult, error) {
	if t.name == "Reject" {
		return llm.ToolResult{}, errors.New("result failed")
	}
	value := status.Error
	if value == "" {
		parts := make([]string, 0, len(values))
		for _, operation := range values {
			parts = append(parts, string(operation.Status))
		}
		value = strings.Join(parts, ",")
		if len(values) == 0 {
			value = "immediate"
		}
	}
	return llm.ToolResult{CallID: id, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: value}}}, nil
}
func (t translator) DescribeRunning(values []operation.Operation) string {
	parts := make([]string, 0, len(values))
	for _, operation := range values {
		parts = append(parts, string(operation.State))
	}
	return strings.Join(parts, ",")
}
func (t translator) Waits() bool { return t.name == "Wait" }

type verifier struct{}

func (verifier) Spec() (operation.Spec, bool) {
	return operation.Spec{Type: operation.TypeValue, Version: operation.VersionValue, State: []byte(`{"Value":0}`)}, true
}
func (verifier) Report(value operation.Operation) (string, bool) {
	return "check failed", value.Status == operation.StatusCompleted
}

func run(input spec) (any, error) {
	if len(input.Actions) > 256 {
		return nil, errors.New("too many oracle actions")
	}
	directory, err := os.MkdirTemp("", "kou-loop-oracle-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(directory) // This directory was created by this invocation.
	base, err := localfile.New(directory)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	const id session.ID = "loop-fixture"
	restored, err := base.Create(ctx, id)
	if err != nil {
		return nil, err
	}
	record := &trace{changed: make(chan struct{}, 256)}
	base.AddObserver(func(_ session.ID, item sessionstore.Item) {
		record.push("commit", map[string]any{"Sequence": item.Sequence, "Kind": item.Kind, "Data": item.Data})
	})
	journal := &store{Store: base, trace: record}
	inboxPort, err := inbox.New(ctx, nil)
	if err != nil {
		return nil, err
	}
	runtime := &manager{trace: record, changed: make(chan struct{}, 256), updates: make(chan operation.Operation, 256)}
	provider := &model{trace: record, requests: make(chan *request, 256)}
	builder := contextbuilder.NewBuilder()
	builder.SetSystemPrompt(input.System)
	builder.SetModel(input.Model)
	builder.SetTools(input.Tools)
	grace := time.Hour
	if input.Config.ToolGraceMS != 0 {
		grace = time.Duration(input.Config.ToolGraceMS) * time.Millisecond
	}
	dependencies := coordinator.Dependencies{
		SessionID: id, Inbox: inboxPort, Sessions: journal, Restored: sessionstore.ResumeState{Snapshot: restored},
		ContextBuilder: builder, LLM: provider, Tools: registry{}, Operations: runtime, ToolGrace: grace,
		ToolWaitLimit:     time.Duration(input.Config.ToolWaitLimitMS) * time.Millisecond,
		AutoCompactTokens: input.Config.AutoCompactTokens, ContextWindow: input.Config.ContextWindow,
		NoRecovery: input.Config.NoRecovery, MaxTurns: input.Config.MaxTurns, MaxCompactions: input.Config.MaxCompactions,
	}
	if input.Verifier {
		dependencies.Verifier = verifier{}
	}
	finished := make(chan error, 1)
	go func() { finished <- coordinator.New(dependencies).Run(ctx) }()
	// Always cancel and join Run, including malformed/failed oracle scripts.
	joined := false
	defer func() {
		cancel()
		if !joined {
			<-finished
		}
	}()
	requests := []*request{}
	replied := map[int]bool{}
	var outcome error
	wait := func(test func() bool, changed <-chan struct{}) error {
		for !test() {
			select {
			case <-changed:
			case <-ctx.Done():
				return ctx.Err()
			}
		}
		return nil
	}
	for index, step := range input.Actions {
		var failure error
		switch step.Kind {
		case "input":
			failure = inboxPort.Submit(ctx, step.Input)
		case "request":
			select {
			case call := <-provider.requests:
				requests = append(requests, call)
			case <-ctx.Done():
				failure = ctx.Err()
			}
		case "respond", "cancelled":
			if step.Index < 0 || step.Index >= len(requests) {
				failure = errors.New("request index out of range")
				break
			}
			call := requests[step.Index]
			if step.Kind == "cancelled" {
				select {
				case <-call.ctx.Done():
				case <-ctx.Done():
					failure = ctx.Err()
				}
				break
			}
			if replied[step.Index] {
				failure = errors.New("request already answered")
				break
			}
			replied[step.Index] = true
			var cause error
			if step.Fault != nil {
				cause = errors.New(step.Fault.Message)
				switch step.Fault.Kind {
				case "quota":
					cause = &llm.RateLimitError{RetryAfter: time.Duration(step.Fault.RetryAfterMS) * time.Millisecond, Err: cause}
				case "overflow":
					cause = &llm.ContextOverflowError{Tokens: step.Fault.Tokens, Limit: step.Fault.Limit, Err: cause}
				}
			}
			call.reply <- modelResult{step.Response, cause}
		case "add", "cancel_op":
			count := max(step.Count, 1)
			failure = wait(func() bool { return runtime.count(step.Kind == "cancel_op") >= count }, runtime.changed)
		case "wait_input":
			failure = wait(func() bool { return record.inputRecorded(step.ID) }, record.changed)
		case "complete":
			value, err := runtime.operation(step.Index)
			if err != nil {
				failure = err
				break
			}
			value.Status = step.Status
			if value.Status == "" {
				value.Status = operation.StatusCompleted
			}
			if len(step.State) != 0 {
				value.State = step.State
			}
			select {
			case runtime.updates <- value:
			case <-ctx.Done():
				failure = ctx.Err()
			}
		case "fail":
			journal.mu.Lock()
			journal.fail = step.Fail
			journal.mu.Unlock()
		case "abort":
			cancel()
		case "finish":
			select {
			case outcome = <-finished:
				joined = true
			case <-ctx.Done():
				if errors.Is(ctx.Err(), context.DeadlineExceeded) {
					failure = ctx.Err()
				} else {
					outcome = <-finished
					joined = true
				}
			}
		default:
			failure = fmt.Errorf("unknown action %q", step.Kind)
		}
		if failure != nil {
			return nil, fmt.Errorf("oracle action %d (%s): %w", index, step.Kind, failure)
		}
		if joined && index != len(input.Actions)-1 {
			return nil, errors.New("actions after finish")
		}
	}
	if !joined {
		return nil, errors.New("script did not finish the coordinator")
	}
	cancel()
	for _, call := range requests {
		<-call.done
	}
	errorText := ""
	if outcome != nil {
		errorText = outcome.Error()
	}
	return map[string]any{"Trace": record.snapshot(), "Error": errorText}, nil
}
func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 8<<20)
	for scanner.Scan() {
		var input spec
		if err := json.Unmarshal(scanner.Bytes(), &input); err != nil {
			panic(err)
		}
		value, err := run(input)
		if err != nil {
			value = map[string]any{"OracleError": err.Error()}
		}
		encoded, err := json.Marshal(value)
		if err != nil {
			panic(err)
		}
		fmt.Println(string(encoded))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
