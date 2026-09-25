package coordinator

import (
	"cmp"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"slices"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

const historyPageSize = 256

const (
	slurpIdleTimeout = time.Millisecond
	slurpMaxItems    = 100
)

const toolCallRunGracePeriod = time.Second

// Automatic compaction backs off where it cannot help, as Claude Code's
// does: after compactionBreaker compactions in a row without a summary, and
// when the conversation refilled to the limit within rapidRefillTurns
// ordinary turns of the compaction before, compactionBreaker times in a row.
// That is a tool output or a file too large for the context window, and
// compacting again would only spend a request on it.
const (
	compactionBreaker = 3
	rapidRefillTurns  = 3
)

// A compaction request the provider finds too large for the context window,
// the estimate of its size having fallen short, is cut down further and sent
// again up to overflowRetries times, as Claude Code does.
const overflowRetries = 3

type coordinator struct {
	dependencies Dependencies
	state        loopState
	stop         stopState
	cancelModel  context.CancelFunc
}

type stopState struct {
	request               inbox.ControlMessage
	cancellationRequested bool // Once set, no new nonterminal operations may enter coordinator state.
}

type loopState struct {
	currentTurnID   session.TurnID
	currentTurnType session.TurnType
	// turnModels are the models the turns' requests went to, as the turns
	// record them. A session may change models from one prompt to the next.
	turnModels        map[session.TurnID]string
	toolCalls         map[toolCallKey]toolCallState
	operations        map[operation.ID]operation.Operation
	availableInputs   int
	deliveredInputs   int
	currentTurnInputs int
	callModel         bool
	grace             <-chan time.Time
	graceToolCalls    map[toolCallKey]struct{}
	// stepCalls are the tool calls of the model's latest response that are
	// still running: the step that input delivered after tools waits for.
	stepCalls map[toolCallKey]struct{}
	// deferred is external input delivered after tools (see
	// inbox.DeliverAfterTools) that waits, in the order it came, for the
	// model to finish the response it is writing and for stepCalls to
	// finish. It is recorded only when it goes out, so it follows the
	// results of the tool calls it waited for.
	deferred []inbox.Input
	// toolWait fires when deferred input has waited ToolWaitLimit for
	// stepCalls; waitedLong is set then, until the input goes out.
	toolWait   <-chan time.Time
	waitedLong bool
	// compactionTurns are the turns that summarize the conversation.
	compactionTurns map[session.TurnID]struct{}
	// compacted is set by a compaction response and cleared by an ordinary
	// one: until the model answers again, compacting once more would only
	// summarize the summary.
	compacted bool
	// compactRequest is a Compact control waiting for its turn.
	compactRequest *compactRequest
	// compactionAsked is set by a Compact control and cleared when a turn
	// begins; turnsSinceCompaction counts ordinary responses since the last
	// compaction, -1 before any. With compactionFailures and rapidRefills
	// they come from the session's items, so a resumed run backs off too.
	compactionAsked      bool
	turnsSinceCompaction int
	compactionFailures   int
	rapidRefills         int
	// sent is the request the model is answering, for when the provider
	// finds it too large for the context window; overflowBudget, when set,
	// is the budget of the compaction that makes up for such a request.
	sent           sentRequest
	overflowBudget int64
}

type compactRequest struct {
	focus string
}

// sentRequest is what a request the model is answering came from: the
// estimate of its size and, for a compaction, its focus, its budget and how
// many times it was cut down to fit the context window.
type sentRequest struct {
	estimate int64
	focus    string
	budget   int64
	retries  int
}

type toolCallState struct {
	toolCall   llm.ToolCall
	status     *tool.CallStatus
	operations map[operation.ID]struct{}
}

type toolCallKey struct {
	turnID session.TurnID
	callID string
}

type toolCallContext struct {
	operations []operation.Operation
}

type modelResponseResult struct {
	turnID   session.TurnID
	response llm.Response
	err      error
}

var _ Coordinator = (*coordinator)(nil)

func newLoopState() loopState {
	return loopState{
		toolCalls:            make(map[toolCallKey]toolCallState),
		operations:           make(map[operation.ID]operation.Operation),
		graceToolCalls:       make(map[toolCallKey]struct{}),
		stepCalls:            make(map[toolCallKey]struct{}),
		compactionTurns:      make(map[session.TurnID]struct{}),
		turnModels:           make(map[session.TurnID]string),
		turnsSinceCompaction: -1,
	}
}

func (current *coordinator) Run(ctx context.Context) error {
	if current.dependencies.ToolHeartbeatInterval < 0 {
		return fmt.Errorf("tool heartbeat interval must not be negative")
	}
	if err := current.restore(ctx); err != nil {
		return err
	}

	modelContext, cancelModels := context.WithCancel(ctx)
	defer cancelModels()
	defer current.interruptModel()
	modelResponses := make(chan modelResponseResult)

	inboxOutput := current.dependencies.Inbox.Output()
	operationUpdates := current.dependencies.Operations.Updates()
	statuses, err := current.scheduleToolCalls(ctx)
	if err != nil {
		return err
	}
	if _, err := current.reconcileToolCalls(ctx); err != nil {
		return err
	}
	if err := current.dispatchOperationsToManager(); err != nil {
		return err
	}
	if toolCallStatusesRequireModelResponse(statuses) || current.pendingInputs() > 0 {
		err = current.requestModelResponse(modelContext, modelResponses)
		if err != nil {
			return err
		}
	}

	var heartbeat <-chan time.Time
	for {
		if !current.isWaitingForOnlyToolCalls() {
			heartbeat = nil
		} else if heartbeat == nil && current.dependencies.ToolHeartbeatInterval > 0 {
			heartbeat = time.After(current.dependencies.ToolHeartbeatInterval)
		}
		select {
		case <-ctx.Done():
			return ctx.Err()

		case received, open := <-inboxOutput:
			if !open {
				return closedInputError(ctx, "inbox output")
			}
			if err := current.processInputs(ctx, []inbox.Input{received}); err != nil {
				return err
			}

		case received, open := <-operationUpdates:
			if !open {
				return closedInputError(ctx, "operation updates")
			}
			if err := current.processOperations(ctx, []operation.Operation{received}); err != nil {
				return err
			}

		case <-heartbeat:
			if err := current.postHeartbeat(ctx); err != nil {
				return err
			}

		case <-current.state.grace:
			current.clearToolGrace()

		case <-current.state.toolWait:
			current.state.toolWait, current.state.waitedLong = nil, true

		case received := <-modelResponses:
			if current.cancelModel == nil || received.turnID != current.state.currentTurnID {
				continue
			}
			recovered, err := current.recoverFromOverflow(modelContext, modelResponses, received)
			if err != nil {
				return err
			}
			if !recovered {
				if err := current.processModelResponse(ctx, received); err != nil {
					return err
				}
			}
		}

		callModel, err := current.processEvents(ctx)
		if err != nil {
			return err
		}
		if current.stop.request.Mode == inbox.StopHard {
			stopped, err := current.handleStop()
			if err != nil {
				return err
			}
			if stopped {
				return ctx.Err()
			}
			continue
		}
		if callModel && current.compacting() {
			// The compaction finishes first: the turn starts from its summary.
			callModel = false
		}
		if callModel {
			err = current.requestModelResponse(modelContext, modelResponses)
			if err != nil {
				return err
			}
			current.clearToolGrace()
			// heartbeat is cleared immediately at the top of the loop, because now we're waiting for the model response
		} else if current.compactionRequested() {
			if err := current.startTurn(modelContext, modelResponses, false); err != nil {
				return err
			}
		}
		if current.stop.request.Mode == inbox.StopWhenIdle && current.isIdle() {
			return ctx.Err()
		}
	}
}

func (current *coordinator) processEvents(ctx context.Context) (bool, error) {
	inputs, err := slurpChannel(ctx, current.dependencies.Inbox.Output())
	if err != nil {
		return false, fmt.Errorf("slurp inbox: %w", err)
	}
	if err := current.processInputs(ctx, inputs); err != nil {
		return false, err
	}
	updates, err := slurpChannel(ctx, current.dependencies.Operations.Updates())
	if err != nil {
		return false, fmt.Errorf("slurp operation updates: %w", err)
	}
	if err := current.processOperations(ctx, updates); err != nil {
		return false, err
	}

	if _, err := current.reconcileToolCalls(ctx); err != nil {
		return false, err
	}
	due := current.state.callModel || (current.pendingInputs() > 0 && current.cancelModel == nil && len(current.state.graceToolCalls) == 0)
	// Input that waits for the tools goes out with any request that is due,
	// and starts one of its own once there is nothing left to wait for. A
	// compaction in progress finishes first; a run that stops leaves it
	// unrecorded, for its sender to keep.
	if len(current.state.deferred) != 0 && current.stop.request.Mode != inbox.StopHard && !current.compacting() {
		switch {
		case due || !current.midStep() || current.state.waitedLong && current.cancelModel == nil:
			if err := current.deliverDeferred(ctx); err != nil {
				return false, err
			}
			due = true
		case current.cancelModel == nil && current.state.toolWait == nil && current.dependencies.ToolWaitLimit > 0:
			// It waits for the tools now, and not forever.
			current.state.toolWait = time.After(current.dependencies.ToolWaitLimit)
		}
	}
	return due, nil
}

// midStep reports that the model is writing a response or that tool calls of
// its latest response are still running: input delivered after tools waits.
func (current *coordinator) midStep() bool {
	return current.cancelModel != nil || len(current.state.stepCalls) != 0
}

// deliverDeferred records the input that waited for the tools, in the order
// it came, so the next request carries it after the tools' results.
func (current *coordinator) deliverDeferred(ctx context.Context) error {
	for len(current.state.deferred) != 0 {
		input := current.state.deferred[0]
		current.state.deferred[0] = inbox.Input{}
		current.state.deferred = current.state.deferred[1:]
		if err := current.handleInboxInput(ctx, input); err != nil {
			return err
		}
		current.state.callModel = true
	}
	current.state.deferred = nil
	current.state.toolWait, current.state.waitedLong = nil, false
	return nil
}

func (current *coordinator) processInputs(ctx context.Context, inputs []inbox.Input) error {
	return current.handleInboxInputs(ctx, inputs)
}

func (current *coordinator) processOperations(ctx context.Context, updates []operation.Operation) error {
	for _, update := range updates {
		if err := current.handleOperationUpdate(ctx, update); err != nil {
			return err
		}
	}
	return nil
}

func (current *coordinator) processModelResponse(ctx context.Context, modelResponse modelResponseResult) error {
	current.interruptModel()
	if modelResponse.err != nil {
		if err := ctx.Err(); err != nil {
			return err
		}
		return fmt.Errorf("call model for turn %q: %w", modelResponse.turnID, modelResponse.err)
	}
	statuses, err := current.handleModelResponse(ctx, sessionstore.ModelResponse{
		TurnID:   modelResponse.turnID,
		Response: modelResponse.response,
	})
	if err != nil {
		return err
	}
	if !current.isCompactionTurn(modelResponse.turnID) {
		// This response is the model's latest step: input delivered after
		// tools waits for the calls it made, not for older ones.
		clear(current.state.stepCalls)
		for _, status := range statuses {
			key := toolCallKey{turnID: status.TurnID, callID: status.CallID}
			if _, running := current.state.toolCalls[key]; running {
				current.state.stepCalls[key] = struct{}{}
			}
		}
	}
	for _, status := range statuses {
		for _, value := range status.Operations {
			if err := current.dispatchOperationToManager(value); err != nil {
				return err
			}
		}
	}
	if !current.state.callModel && len(statuses) > 0 {
		for _, status := range statuses {
			current.state.graceToolCalls[toolCallKey{turnID: status.TurnID, callID: status.CallID}] = struct{}{}
		}
		current.state.grace = time.After(toolCallRunGracePeriod)
	}
	return nil
}

func (current *coordinator) clearToolGrace() {
	clear(current.state.graceToolCalls)
	current.state.grace = nil
}

func (current *coordinator) handleStop() (bool, error) {
	if !current.stop.cancellationRequested {
		current.interruptModel()
		if err := current.cancelOperations(); err != nil {
			return false, err
		}
		current.stop.cancellationRequested = true
	}
	return !current.hasPendingOperations(), nil
}

// compacting reports a compaction turn waiting for its summary.
func (current *coordinator) compacting() bool {
	return current.cancelModel != nil && current.isCompactionTurn(current.state.currentTurnID)
}

func (current *coordinator) isCompactionTurn(id session.TurnID) bool {
	_, compaction := current.state.compactionTurns[id]
	return compaction
}

// compactionRequested reports a Compact control that can start its turn:
// the model is free and no tool call is in its grace period.
func (current *coordinator) compactionRequested() bool {
	return current.state.compactRequest != nil && current.cancelModel == nil && len(current.state.graceToolCalls) == 0
}

// compactionBudget is what a compaction request may take of the context
// window.
func (current *coordinator) compactionBudget() int64 {
	return summaryRoom(current.dependencies.ContextWindow)
}

// summaryRoom is what a compaction request may take of a context window of
// window tokens: the rest, up to 20,000 tokens, is the summary's. It is 0,
// no limit, for a window that is not known.
func summaryRoom(window int64) int64 {
	if window <= 0 {
		return 0
	}
	return window - min(20_000, window*15/100)
}

// compactionDue decides whether the next turn compacts the conversation, and
// with what focus and budget. A requested compaction goes first, then one
// making up for a request the context window could not hold; otherwise the
// request must have outgrown AutoCompactTokens, and not only by input the
// model has yet to answer, which the compaction would keep: the summary has
// to replace at least a quarter of the limit. There must be something to
// summarize either way: a request made when there is not is dropped.
func (current *coordinator) compactionDue(built contextbuilder.Result) (string, int64, bool) {
	request, overflow := current.state.compactRequest, current.state.overflowBudget
	current.state.compactRequest, current.state.overflowBudget = nil, 0
	budget := cmp.Or(overflow, current.compactionBudget())
	switch {
	case !built.Compactable:
		return "", 0, false
	case request != nil:
		return request.focus, budget, true
	case overflow > 0:
		return "", budget, true
	}
	limit := current.dependencies.AutoCompactTokens
	if !current.autoCompactionAllowed() || built.EstimatedTokens < limit ||
		built.EstimatedTokens-built.PendingTokens < limit/4 {
		return "", 0, false
	}
	return "", budget, true
}

// autoCompactionAllowed reports whether the conversation may be compacted
// without a request: automatic compaction is on, the model has answered
// since the last compaction, and neither breaker has tripped.
func (current *coordinator) autoCompactionAllowed() bool {
	state := &current.state
	return current.dependencies.AutoCompactTokens > 0 && !state.compacted &&
		state.compactionFailures < compactionBreaker &&
		!(current.refilledRapidly() && state.rapidRefills+1 >= compactionBreaker)
}

// recoverFromOverflow answers a provider that found the request too large
// for the context window, the estimate of its size having fallen short. A
// compaction request is cut down further and sent again, up to
// overflowRetries times; an ordinary turn gives way to a compaction when
// automatic compaction may run, and follows its summary. It reports false
// when neither applies, and the error stands.
func (current *coordinator) recoverFromOverflow(
	ctx context.Context,
	results chan<- modelResponseResult,
	received modelResponseResult,
) (bool, error) {
	overflow, ok := errors.AsType[*llm.ContextOverflowError](received.err)
	if !ok || ctx.Err() != nil {
		return false, nil
	}
	sent := current.state.sent
	if current.isCompactionTurn(received.turnID) {
		if sent.retries >= overflowRetries {
			return false, nil
		}
		budget := overflowBudget(sent.budget, sent.estimate, overflow)
		built, err := current.dependencies.ContextBuilder.BuildCompaction(sent.focus, budget)
		if err != nil {
			return false, fmt.Errorf("build compaction request: %w", err)
		}
		current.interruptModel()
		current.state.sent = sentRequest{estimate: built.EstimatedTokens, focus: sent.focus, budget: budget, retries: sent.retries + 1}
		current.send(ctx, results, received.turnID, built.Request)
		return true, nil
	}
	if !current.autoCompactionAllowed() {
		return false, nil
	}
	current.state.overflowBudget = overflowBudget(current.compactionBudget(), sent.estimate, overflow)
	if err := current.startTurn(ctx, results, false); err != nil {
		return false, err
	}
	if !current.compacting() {
		return false, nil // nothing to summarize
	}
	current.state.callModel = true
	return true, nil
}

// overflowBudget is the budget of a compaction request after the provider
// found a request estimated at estimate tokens too large for the context
// window. When the provider gave the sizes, it is the room the window leaves
// beside the summary, measured as the estimate measured that request;
// otherwise it is a fifth less than the estimate. base, when set, is the
// most it may be.
func overflowBudget(base, estimate int64, overflow *llm.ContextOverflowError) int64 {
	budget := estimate * 4 / 5
	if overflow.Limit > 0 && overflow.Tokens > overflow.Limit && estimate > 0 {
		budget = int64(float64(summaryRoom(overflow.Limit)) * float64(estimate) / float64(overflow.Tokens))
	}
	if base > 0 {
		budget = min(budget, base)
	}
	return max(budget, 1)
}

// refilledRapidly reports a conversation that reached the limit again within
// rapidRefillTurns ordinary turns of the last compaction.
func (current *coordinator) refilledRapidly() bool {
	return current.state.turnsSinceCompaction >= 0 && current.state.turnsSinceCompaction < rapidRefillTurns
}

func (current *coordinator) isIdle() bool {
	return current.cancelModel == nil && current.pendingInputs() == 0 && len(current.state.deferred) == 0 &&
		len(current.state.toolCalls) == 0 && !current.hasPendingOperations()
}

func (current *coordinator) isWaitingForOnlyToolCalls() bool {
	return current.cancelModel == nil && current.stop.request.Mode != inbox.StopHard &&
		current.pendingInputs() == 0 && len(current.state.toolCalls) != 0
}

func (current *coordinator) postHeartbeat(ctx context.Context) error {
	calls := make([]llm.ToolCall, 0, len(current.state.toolCalls))
	for _, call := range current.state.toolCalls {
		calls = append(calls, call.toolCall)
	}
	slices.SortFunc(calls, func(a, b llm.ToolCall) int {
		return cmp.Compare(a.CallID, b.CallID)
	})
	runningCalls, err := json.Marshal(calls)
	if err != nil {
		return fmt.Errorf("encode heartbeat tool calls: %w", err)
	}
	payload, err := json.Marshal(inbox.ControlMessage{
		Mode:   inbox.Heartbeat,
		Reason: fmt.Sprintf("Heartbeat: waited %g seconds for tool calls.\nRunning: %s", current.dependencies.ToolHeartbeatInterval.Seconds(), runningCalls),
	})
	if err != nil {
		return fmt.Errorf("encode heartbeat: %w", err)
	}
	return current.dependencies.Inbox.Submit(ctx, inbox.Input{
		ID: inbox.ID(uuid.New().String()), Kind: inbox.InputControl, Payload: payload,
	})
}

func (current *coordinator) interruptModel() {
	if current.cancelModel != nil {
		current.cancelModel()
		current.cancelModel = nil
	}
}

func (current *coordinator) acceptStop(request inbox.ControlMessage) {
	if current.stop.request.Mode == inbox.StopHard {
		return
	}
	current.stop.request = request
}

func (current *coordinator) pendingInputs() int {
	return current.state.availableInputs - current.state.deliveredInputs
}

func (current *coordinator) hasPendingOperations() bool {
	for _, value := range current.state.operations {
		if !operationIsTerminal(value.Status) {
			return true
		}
	}
	return false
}

func (current *coordinator) cancelOperations() error {
	var result error
	for id, value := range current.state.operations {
		if operationIsTerminal(value.Status) {
			continue
		}
		if err := current.dependencies.Operations.Cancel(id, current.stop.request.Reason); err != nil {
			result = errors.Join(result, fmt.Errorf("cancel operation %q: %w", id, err))
		}
	}
	return result
}

// requestModelResponse starts the ordinary turn that is due, or the
// compaction that has to come first.
func (current *coordinator) requestModelResponse(
	ctx context.Context,
	results chan<- modelResponseResult,
) error {
	return current.startTurn(ctx, results, true)
}

// startTurn starts a compaction turn when one was requested or the request
// has outgrown AutoCompactTokens, and otherwise, if ordinary is set, an
// ordinary turn. An ordinary turn a compaction takes the place of stays due
// and follows it, starting from the summary.
func (current *coordinator) startTurn(
	ctx context.Context,
	results chan<- modelResponseResult,
	ordinary bool,
) error {
	current.interruptModel()
	if current.dependencies.BeforeTurn != nil {
		current.dependencies.BeforeTurn()
	}
	built, err := current.dependencies.ContextBuilder.Build()
	if err != nil {
		return fmt.Errorf("build model request: %w", err)
	}
	turnType := session.TurnRegular
	sent := sentRequest{estimate: built.EstimatedTokens}
	if focus, budget, due := current.compactionDue(built); due {
		built, err = current.dependencies.ContextBuilder.BuildCompaction(focus, budget)
		if err != nil {
			return fmt.Errorf("build compaction request: %w", err)
		}
		turnType = session.TurnCompaction
		sent = sentRequest{estimate: built.EstimatedTokens, focus: focus, budget: budget}
	} else if !ordinary {
		return nil
	}
	turn := session.Turn{
		ID:             session.TurnID(uuid.New().String()),
		PreviousTurnID: current.state.currentTurnID,
		Type:           turnType,
		Model:          built.Request.Model.ID,
	}
	item, err := current.addItemToLocalState(sessionstore.Item{
		Kind: sessionstore.ItemTurn,
		Data: turn,
	})
	if err != nil {
		return err
	}
	if err := current.storeItemInSessionStore(ctx, item); err != nil {
		return err
	}

	if turnType == session.TurnRegular {
		current.state.callModel = false
	} else {
		current.state.callModel = current.state.callModel || ordinary
	}
	current.state.sent = sent
	current.send(ctx, results, turn.ID, built.Request)
	return nil
}

// send has the model answer request for the turn; the answer arrives on
// results.
func (current *coordinator) send(
	ctx context.Context,
	results chan<- modelResponseResult,
	turnID session.TurnID,
	request llm.Request,
) {
	requestContext, cancel := context.WithCancel(ctx)
	current.cancelModel = cancel
	go func() {
		response, err := current.dependencies.LLM.Respond(requestContext, request, llm.RequestOptions{
			CacheKey: string(current.dependencies.SessionID),
		})
		select {
		case results <- modelResponseResult{
			turnID:   turnID,
			response: response,
			err:      err,
		}:
		case <-ctx.Done():
		}
	}()
}

func (current *coordinator) handleInboxInput(ctx context.Context, input inbox.Input) error {
	item, err := current.addItemToLocalState(sessionstore.Item{
		Kind: sessionstore.ItemInput,
		Data: input,
	})
	if err != nil {
		return err
	}
	if err := current.storeItemInSessionStore(ctx, item); err != nil {
		return err
	}
	if input.Kind == inbox.InputControl {
		request, err := input.DecodeControlMessage()
		if err != nil {
			return err
		}
		switch request.Mode {
		case inbox.UpdateSettings:
			return nil
		case inbox.Compact:
			// Nothing waits on it: the compaction starts once the model is
			// free, and does not cut short the tools' grace period.
			current.state.compactRequest = &compactRequest{focus: request.Reason}
			return nil
		case inbox.StopHard, inbox.StopWhenIdle:
			current.acceptStop(request)
		}
	}
	current.clearToolGrace()
	return nil
}

func (current *coordinator) handleInboxInputs(ctx context.Context, inputs []inbox.Input) error {
	for _, input := range inputs {
		if input.Kind == inbox.InputExternal {
			// Input delivered after tools waits while the model works on a
			// step, and behind input that already waits.
			if input.Delivery == inbox.DeliverAfterTools && (len(current.state.deferred) != 0 || current.midStep()) {
				current.state.deferred = append(current.state.deferred, input)
				continue
			}
			// Input delivered at once starts a turn, which takes what waits
			// along, ahead of it.
			if err := current.deliverDeferred(ctx); err != nil {
				return err
			}
		}
		if err := current.handleInboxInput(ctx, input); err != nil {
			return err
		}
		if input.Kind == inbox.InputExternal {
			current.state.callModel = true
		}
	}
	return nil
}

func slurpChannel[T any](
	ctx context.Context,
	output <-chan T,
) ([]T, error) {
	var inputs []T
	idle := time.NewTimer(slurpIdleTimeout)
	defer idle.Stop()
	for len(inputs) < slurpMaxItems {
		select {
		case <-ctx.Done():
			return nil, context.Cause(ctx)
		case input, open := <-output:
			if !open {
				return inputs, nil
			}
			inputs = append(inputs, input)
			idle.Reset(slurpIdleTimeout)
		case <-idle.C:
			return inputs, nil
		}
	}
	return inputs, nil
}

func (current *coordinator) handleModelResponse(
	ctx context.Context,
	response sessionstore.ModelResponse,
) ([]sessionstore.ToolCallStatus, error) {
	item, err := current.addItemToLocalState(sessionstore.Item{
		Kind: sessionstore.ItemModelResponse,
		Data: response,
	})
	if err != nil {
		return nil, err
	}
	if err := current.storeItemInSessionStore(ctx, item); err != nil {
		return nil, err
	}
	if current.isCompactionTurn(response.TurnID) {
		return nil, nil
	}
	statuses, err := current.scheduleToolCalls(ctx)
	if err != nil {
		return nil, err
	}
	current.state.callModel = toolCallStatusesRequireModelResponse(statuses)
	return statuses, nil
}

func (current *coordinator) handleOperationUpdate(
	ctx context.Context,
	update operation.Operation,
) error {
	update = current.addOperationToLocalState(update)
	return current.storeOperationInSessionStore(ctx, update)
}

func (current *coordinator) restore(ctx context.Context) error {
	if err := current.loadHistory(ctx); err != nil {
		return err
	}
	for _, value := range current.dependencies.Restored.Operations {
		current.addOperationToLocalState(value)
	}
	return nil
}

func (current *coordinator) loadHistory(ctx context.Context) error {
	after := sessionstore.BeforeFirst
	for {
		page, err := current.dependencies.Sessions.Items(
			ctx,
			current.dependencies.SessionID,
			after,
			historyPageSize,
		)
		if err != nil {
			return fmt.Errorf("load session history after %d: %w", after, err)
		}
		for _, item := range page.Items {
			if err := current.restoreItem(item); err != nil {
				return fmt.Errorf("load session item %d: %w", item.Sequence, err)
			}
		}
		if !page.More {
			return nil
		}
		if page.NextAfter <= after {
			return fmt.Errorf("load session history did not advance after %d", after)
		}
		after = page.NextAfter
	}
}

func (current *coordinator) restoreItem(item sessionstore.Item) error {
	status, ok := item.Data.(sessionstore.ToolCallStatus)
	if item.Kind == sessionstore.ItemToolCallStatus && ok && toolCallRequiresTranslator(status) {
		call, exists := current.state.toolCalls[toolCallKey{turnID: status.TurnID, callID: status.CallID}]
		if exists {
			if _, available := current.dependencies.Tools.Resolve(call.toolCall.Name); !available {
				return fmt.Errorf("tool %q required by recorded call %q is not available", call.toolCall.Name, status.CallID)
			}
		}
	}
	_, err := current.addItemToLocalState(item)
	return err
}

func toolCallRequiresTranslator(status sessionstore.ToolCallStatus) bool {
	return status.Status.Error == "" || len(status.Status.WaitingFor) != 0 || len(status.Operations) != 0
}

func (current *coordinator) addItemToLocalState(
	item sessionstore.Item,
) (sessionstore.Item, error) {
	switch item.Kind {
	case sessionstore.ItemFork:
		if _, ok := item.Data.(sessionstore.Fork); !ok {
			return sessionstore.Item{}, fmt.Errorf(
				"fork data is %T, want sessionstore.Fork",
				item.Data,
			)
		}
		// FIXME: Forks leave inherited calls without results and retain pending-input accounting.
		clear(current.state.toolCalls)
		clear(current.state.operations)
		clear(current.state.stepCalls)
		current.clearToolGrace()

	case sessionstore.ItemInput:
		input, ok := item.Data.(inbox.Input)
		if !ok {
			return sessionstore.Item{}, fmt.Errorf(
				"input data is %T, want inbox.Input",
				item.Data,
			)
		}
		if err := input.Validate(); err != nil {
			return sessionstore.Item{}, fmt.Errorf("invalid input: %w", err)
		}
		if input.Kind == inbox.InputExternal {
			if err := current.dependencies.ContextBuilder.AddExternalInput(input); err != nil {
				return sessionstore.Item{}, fmt.Errorf(
					"add input %q to context: %w",
					input.ID,
					err,
				)
			}
			current.state.availableInputs++
		}
		if input.Kind == inbox.InputControl {
			request, err := input.DecodeControlMessage()
			if err != nil {
				return sessionstore.Item{}, err
			}
			current.dependencies.ContextBuilder.AddControlMessage(request)
			if request.Mode == inbox.Heartbeat {
				current.state.availableInputs++
			}
			if request.Mode == inbox.Compact {
				current.state.compactionAsked = true
			}
		}

	case sessionstore.ItemTurn:
		turn, ok := item.Data.(session.Turn)
		if !ok {
			return sessionstore.Item{}, fmt.Errorf(
				"turn data is %T, want session.Turn",
				item.Data,
			)
		}
		current.state.currentTurnID = turn.ID
		current.state.currentTurnType = turn.Type
		if turn.Model != "" {
			current.state.turnModels[turn.ID] = turn.Model
		}
		if turn.Type == session.TurnCompaction {
			current.state.compactionTurns[turn.ID] = struct{}{}
			// A compaction asked for is no refill.
			if !current.state.compactionAsked && current.refilledRapidly() {
				current.state.rapidRefills++
			} else {
				current.state.rapidRefills = 0
			}
		} else {
			delete(current.state.compactionTurns, turn.ID)
		}
		current.state.compactionAsked = false
		current.state.currentTurnInputs = current.state.availableInputs
		current.dependencies.ContextBuilder.Commit()

	case sessionstore.ItemModelResponse:
		response, ok := item.Data.(sessionstore.ModelResponse)
		if !ok {
			return sessionstore.Item{}, fmt.Errorf(
				"model response data is %T, want sessionstore.ModelResponse",
				item.Data,
			)
		}
		if current.isCompactionTurn(response.TurnID) {
			// The summary stands in for what its turn saw. A later turn saw
			// more, so the summary of an earlier one would lose that.
			if response.TurnID == current.state.currentTurnID {
				if current.dependencies.ContextBuilder.Compact(response.Response) {
					current.state.compactionFailures = 0
					current.state.turnsSinceCompaction = 0
				} else {
					current.state.compactionFailures++
				}
				current.state.compacted = true
			}
			return item, nil
		}
		// The complete output includes messages, reasoning, and tool calls,
		// which say which model wrote them: what one model's provider
		// attached to its output means nothing to another's.
		current.dependencies.ContextBuilder.AddModelResponse(writtenBy(response.Response, current.state.turnModels[response.TurnID]))
		current.state.compacted = false
		if current.state.turnsSinceCompaction >= 0 {
			current.state.turnsSinceCompaction++
		}
		if response.TurnID == current.state.currentTurnID {
			current.state.deliveredInputs = current.state.currentTurnInputs
		}
		current.addToolCallsToLocalState(response)

	case sessionstore.ItemToolCallStatus:
		status, ok := item.Data.(sessionstore.ToolCallStatus)
		if !ok {
			return sessionstore.Item{}, fmt.Errorf(
				"tool-call status data is %T, want sessionstore.ToolCallStatus",
				item.Data,
			)
		}
		for _, value := range status.Operations {
			current.addOperationToLocalState(value)
		}
		current.addToolCallOperationsToLocalState(status)
		if err := current.addToolResultToLocalState(status); err != nil {
			return sessionstore.Item{}, err
		}

	default:
		return sessionstore.Item{}, fmt.Errorf("unsupported item kind %q", item.Kind)
	}

	return item, nil
}

func (current *coordinator) addToolCallsToLocalState(response sessionstore.ModelResponse) {
	for _, output := range response.Response.Output {
		if output.Type != llm.ItemToolCall {
			continue
		}
		call := output.Data.(llm.ToolCall)
		current.state.toolCalls[toolCallKey{
			turnID: response.TurnID,
			callID: call.CallID,
		}] = toolCallState{
			toolCall:   call,
			operations: make(map[operation.ID]struct{}),
		}
	}
}

func (current *coordinator) addToolCallOperationsToLocalState(
	status sessionstore.ToolCallStatus,
) {
	call, exists := current.state.toolCalls[toolCallKey{
		turnID: status.TurnID,
		callID: status.CallID,
	}]
	if !exists {
		return
	}
	statusValue := status.Status
	call.status = &statusValue
	for _, id := range status.Status.WaitingFor {
		call.operations[id] = struct{}{}
	}
	current.state.toolCalls[toolCallKey{
		turnID: status.TurnID,
		callID: status.CallID,
	}] = call
}

func (current *coordinator) finishToolCall(
	turnID session.TurnID,
	callID string,
) {
	key := toolCallKey{turnID: turnID, callID: callID}
	delete(current.state.toolCalls, key)
	delete(current.state.graceToolCalls, key)
	delete(current.state.stepCalls, key)
	if len(current.state.graceToolCalls) == 0 {
		current.clearToolGrace()
	}
	current.state.availableInputs++
}

func (current *coordinator) toolCallOperationsAreTerminal(
	turnID session.TurnID,
	callID string,
) bool {
	call := current.state.toolCalls[toolCallKey{
		turnID: turnID,
		callID: callID,
	}]
	for id := range call.operations {
		value, exists := current.state.operations[id]
		if !exists || !operationIsTerminal(value.Status) {
			return false
		}
	}
	return true
}

func (current *coordinator) addToolResultToLocalState(
	status sessionstore.ToolCallStatus,
) error {
	call, exists := current.state.toolCalls[toolCallKey{
		turnID: status.TurnID,
		callID: status.CallID,
	}]
	if !exists {
		return nil
	}
	translator, exists := current.dependencies.Tools.Resolve(call.toolCall.Name)
	if !exists {
		if !toolCallRequiresTranslator(status) {
			current.dependencies.ContextBuilder.AddToolResult(status.CallID, []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: status.Status.Error}}, false)
			current.finishToolCall(status.TurnID, status.CallID)
		}
		return nil
	}

	operations := make([]operation.Operation, 0, len(status.Status.WaitingFor))
	for _, id := range status.Status.WaitingFor {
		if _, exists := call.operations[id]; !exists {
			return nil
		}
		value, exists := current.state.operations[id]
		if !exists {
			return nil
		}
		operations = append(operations, value)
	}
	result, err := translator.TranslateResult(status.CallID, status.Status, operations)
	if err != nil {
		return fmt.Errorf("add tool call %q result to context: %w", status.CallID, err)
	}
	running := !current.toolCallOperationsAreTerminal(status.TurnID, status.CallID)
	current.dependencies.ContextBuilder.AddToolResult(
		status.CallID,
		result.Output,
		running,
	)
	if !running {
		current.finishToolCall(status.TurnID, status.CallID)
	}
	return nil
}

func (current *coordinator) addOperationToLocalState(
	value operation.Operation,
) operation.Operation {
	current.state.operations[value.ID] = value
	return current.state.operations[value.ID]
}

func (current *coordinator) scheduleToolCalls(
	ctx context.Context,
) ([]sessionstore.ToolCallStatus, error) {
	statuses := make([]sessionstore.ToolCallStatus, 0)
	for key, call := range current.state.toolCalls {
		if call.status != nil {
			continue
		}
		status, err := current.scheduleToolCall(ctx, key, call.toolCall)
		if err != nil {
			return nil, err
		}
		statuses = append(statuses, status)
	}
	return statuses, nil
}

func (current *coordinator) scheduleToolCall(
	ctx context.Context,
	key toolCallKey,
	call llm.ToolCall,
) (sessionstore.ToolCallStatus, error) {
	translator, exists := current.dependencies.Tools.Resolve(call.Name)
	toolContext := &toolCallContext{}
	var status tool.CallStatus
	if exists {
		status = translator.Translate(toolContext, call)
	} else {
		status = tool.ErrorStatus(fmt.Sprintf("tool %q is not available", call.Name), 0)
	}
	operations := make([]operation.Operation, 0, len(toolContext.operations))
	for _, value := range toolContext.operations {
		operations = append(operations, current.addOperationToLocalState(value))
	}
	toolCallStatus := sessionstore.ToolCallStatus{
		TurnID:     key.turnID,
		CallID:     key.callID,
		Status:     status,
		Operations: operations,
	}
	item, err := current.addItemToLocalState(sessionstore.Item{
		Kind: sessionstore.ItemToolCallStatus,
		Data: toolCallStatus,
	})
	if err != nil {
		return sessionstore.ToolCallStatus{}, err
	}
	if err := current.storeItemInSessionStore(ctx, item); err != nil {
		return sessionstore.ToolCallStatus{}, err
	}
	return toolCallStatus, nil
}

func (current *toolCallContext) Submit(spec operation.Spec) operation.ID {
	id := operation.ID(uuid.New().String())
	current.operations = append(current.operations, operation.Operation{
		MaxOutputLength: spec.MaxOutputLength,
		ID:              id,
		Type:            spec.Type,
		Version:         spec.Version,
		Status:          operation.StatusReady,
		State:           spec.State,
		Idempotency:     spec.Idempotency,
	})
	return id
}

func (current *coordinator) reconcileToolCalls(
	ctx context.Context,
) ([]sessionstore.ToolCallStatus, error) {
	completed := make([]sessionstore.ToolCallStatus, 0)
	for key := range current.state.toolCalls {
		if !current.toolCallOperationsAreTerminal(key.turnID, key.callID) {
			continue
		}
		call := current.state.toolCalls[key]
		if call.status == nil {
			return nil, fmt.Errorf("reconcile untranslated tool call %q in turn %q", key.callID, key.turnID)
		}
		operations := make([]operation.Operation, 0, len(call.status.WaitingFor))
		for _, id := range call.status.WaitingFor {
			operations = append(operations, current.state.operations[id])
		}
		status := sessionstore.ToolCallStatus{
			TurnID:     key.turnID,
			CallID:     key.callID,
			Status:     *call.status,
			Operations: operations,
		}
		item, err := current.addItemToLocalState(sessionstore.Item{
			Kind: sessionstore.ItemToolCallStatus,
			Data: status,
		})
		if err != nil {
			return nil, err
		}
		if err := current.storeItemInSessionStore(ctx, item); err != nil {
			return nil, err
		}
		if _, exists := current.state.toolCalls[key]; !exists {
			completed = append(completed, status)
		}
	}
	return completed, nil
}

func toolCallStatusesRequireModelResponse(statuses []sessionstore.ToolCallStatus) bool {
	for _, status := range statuses {
		if status.Status.Error != "" || len(status.Status.WaitingFor) == 0 {
			return true
		}
	}
	return false
}

func (current *coordinator) storeItemInSessionStore(
	ctx context.Context,
	item sessionstore.Item,
) error {
	switch item.Kind {
	case sessionstore.ItemInput:
		input := item.Data.(inbox.Input)
		if err := current.dependencies.Sessions.AppendInput(
			ctx,
			current.dependencies.SessionID,
			input,
		); err != nil {
			return fmt.Errorf("store input %q: %w", input.ID, err)
		}

	case sessionstore.ItemTurn:
		turn := item.Data.(session.Turn)
		if err := current.dependencies.Sessions.AppendTurn(
			ctx,
			current.dependencies.SessionID,
			turn,
		); err != nil {
			return fmt.Errorf("store turn %q: %w", turn.ID, err)
		}

	case sessionstore.ItemModelResponse:
		response := item.Data.(sessionstore.ModelResponse)
		if err := current.dependencies.Sessions.AppendModelResponse(
			ctx,
			current.dependencies.SessionID,
			response,
		); err != nil {
			return fmt.Errorf("store turn %q response: %w", response.TurnID, err)
		}

	case sessionstore.ItemToolCallStatus:
		status := item.Data.(sessionstore.ToolCallStatus)
		if err := current.dependencies.Sessions.AppendToolCallStatus(
			ctx,
			current.dependencies.SessionID,
			status,
		); err != nil {
			return fmt.Errorf("store tool call %q status: %w", status.CallID, err)
		}

	default:
		return fmt.Errorf("unsupported local item kind %q", item.Kind)
	}
	return nil
}

func (current *coordinator) storeOperationInSessionStore(
	ctx context.Context,
	value operation.Operation,
) error {
	if err := current.dependencies.Sessions.SaveOperation(
		ctx,
		current.dependencies.SessionID,
		value,
	); err != nil {
		return fmt.Errorf("store operation %q: %w", value.ID, err)
	}
	return nil
}

func (current *coordinator) dispatchOperationsToManager() error {
	for _, value := range current.state.operations {
		if err := current.dispatchOperationToManager(value); err != nil {
			return err
		}
	}
	return nil
}

func (current *coordinator) dispatchOperationToManager(value operation.Operation) error {
	if operationIsTerminal(value.Status) {
		return nil
	}
	value.State = value.State.Clone()
	value.Idempotency = value.Idempotency.Clone()
	if err := current.dependencies.Operations.Add(value); err != nil {
		return fmt.Errorf("dispatch operation %q: %w", value.ID, err)
	}
	return nil
}

func operationIsTerminal(status operation.Status) bool {
	switch status {
	case operation.StatusCompleted, operation.StatusFailed, operation.StatusCanceled:
		return true
	default:
		return false
	}
}

func closedInputError(ctx context.Context, name string) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	return fmt.Errorf("%s closed", name)
}

// writtenBy is the response with its output marked as model's, which the
// session records on the response's turn. The recorded response stays as the
// provider returned it.
func writtenBy(response llm.Response, model string) llm.Response {
	if model == "" || len(response.Output) == 0 {
		return response
	}
	output := make([]llm.Item, len(response.Output))
	for index, item := range response.Output {
		if item.Model == "" {
			item.Model = model
		}
		output[index] = item
	}
	response.Output = output
	return response
}
