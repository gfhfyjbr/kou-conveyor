package coordinator

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"slices"
	"strings"
	"testing"
	"testing/synctest"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestCoordinatorAutoCompactsBeforeAnsweringANewPrompt(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 5000))
		run.input(t, externalEvent(t, 0, "second", "second task"))
		if len(run.calls) != 2 || !isCompactionRequest(run.calls[1].request) {
			t.Fatal("an oversized request was not compacted first")
		}
		// The compaction sees the prompt; the turn after it answers the prompt.
		if !containsMessage(run.calls[1].request, "second task") {
			t.Fatal("compaction request lacks the pending prompt")
		}
		run.respond(t, 1, textResponse("Summary of the first task."))
		if len(run.calls) != 3 {
			t.Fatal("no turn continued after the compaction")
		}
		input := run.calls[2].request.Input
		if len(input) != 3 || !isSummary(input[1], "Summary of the first task.") || messageText(input[2]) != "second task" {
			t.Fatalf("continuation input = %#v", input)
		}
		if summary := messageText(input[1]); !strings.Contains(summary, "<message>\nfirst task\n</message>") || strings.Contains(summary, "second task") {
			t.Fatalf("summary message does not keep only the answered prompts: %q", summary)
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.respond(t, 2, textResponse("Second answer."))
		run.assertStopped(t)
		assertTurnTypes(t, run, session.TurnRegular, session.TurnCompaction, session.TurnRegular)
		if run.current.pendingInputs() != 0 {
			t.Fatal("the continuation did not deliver the prompt")
		}
	})
}

func TestCoordinatorAutoCompactsBetweenToolCallsAndContinues(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		spec, err := operation.NewValueSpec(jsontext.Value(`{"value":1}`))
		if err != nil {
			t.Fatal(err)
		}
		run.current.dependencies.Tools = tool.NewRegistry(tool.StaticTranslators{
			Bash: &submittingTranslator{specs: []operation.Spec{spec}}, ViewImage: testTranslator{},
		}, tool.BashName, tool.ViewImageName)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "task", "run both checks"))
		calls := usageResponse("", 5000)
		calls.Output = []llm.Item{
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "quick", Name: tool.BashName, Arguments: `{"command":"make quick"}`}},
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: "slow", Name: tool.BashName, Arguments: `{"command":"make slow"}`}},
		}
		run.respond(t, 0, calls)
		if len(run.operations.adds) != 2 {
			t.Fatalf("operations = %d, want 2", len(run.operations.adds))
		}
		// Calls are scheduled in no particular order.
		complete := func(callID string) {
			t.Helper()
			for _, status := range run.store.appendedStatuses {
				if status.CallID != callID {
					continue
				}
				value := status.Operations[0]
				value.Status = operation.StatusCompleted
				run.operations.updates <- value
				synctest.Wait()
				synctest.Sleep(2 * slurpIdleTimeout)
				synctest.Wait()
				return
			}
			t.Fatalf("no operation for %s", callID)
		}
		complete("quick")
		// The slow call outlasts the grace period the tools get before the
		// model hears of the quick one.
		synctest.Sleep(toolCallRunGracePeriod)
		synctest.Wait()
		if len(run.calls) != 2 || !isCompactionRequest(run.calls[1].request) {
			t.Fatal("the turn after a tool result was not compacted first")
		}
		run.respond(t, 1, textResponse("Quick check passed; the slow one runs."))
		if len(run.calls) != 3 {
			t.Fatal("work did not continue after the compaction")
		}
		// The call still running is named in the summary message: its result
		// arrives without the call.
		input := run.calls[2].request.Input
		if len(input) != 2 || !isSummary(input[1], "Quick check passed; the slow one runs.") ||
			!strings.Contains(messageText(input[1]), `slow (Bash {"command":"make slow"})`) || strings.Contains(messageText(input[1]), "quick (") {
			t.Fatalf("continuation input = %#v", input)
		}
		complete("slow")
		if len(run.calls) != 3 || run.calls[2].ctx.Err() != nil {
			t.Fatal("a tool result interrupted the model")
		}
		run.respond(t, 2, usageResponse("Waiting for the slow check.", 300))
		if len(run.calls) != 4 {
			t.Fatal("the slow result did not wake the model")
		}
		input = run.calls[3].request.Input
		last := messageText(input[len(input)-1])
		if isCompactionRequest(run.calls[3].request) || !strings.HasPrefix(last, `Result of the tool call slow (Bash {"command":"make slow"}), made before the conversation was compacted:`) {
			t.Fatalf("result of the compacted call = %q", last)
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.respond(t, 3, textResponse("Both checks passed."))
		run.assertStopped(t)
		assertTurnTypes(t, run, session.TurnRegular, session.TurnCompaction, session.TurnRegular, session.TurnRegular)
	})
}

func TestCoordinatorDoesNotCompactForALargePrompt(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 100))
		// Compacting would keep the prompt that makes the request large.
		run.input(t, externalEvent(t, 0, "second", strings.Repeat("pasted log line\n", 400)))
		if len(run.calls) != 2 || isCompactionRequest(run.calls[1].request) {
			t.Fatal("compacted a short conversation for a long prompt")
		}
	})
}

func TestCoordinatorCompactsOnRequest(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, textResponse("First answer."))
		run.input(t, compactInput(t, "compact", "the open questions"), stopInput(t, "stop", inbox.StopWhenIdle))
		if len(run.calls) != 2 || !isCompactionRequest(run.calls[1].request) {
			t.Fatal("a requested compaction did not start")
		}
		prompt := messageText(run.calls[1].request.Input[len(run.calls[1].request.Input)-1])
		if !strings.Contains(prompt, "The user asked the summary to focus on:\nthe open questions\n\nReminder:") {
			t.Fatalf("compaction prompt = %q", prompt)
		}
		run.respond(t, 1, textResponse("Summary."))
		run.assertStopped(t)
		if len(run.calls) != 2 {
			t.Fatal("a requested compaction started a turn with nothing to answer")
		}
		built, err := run.current.dependencies.ContextBuilder.Build()
		if err != nil {
			t.Fatal(err)
		}
		if len(built.Request.Input) != 2 || !isSummary(built.Request.Input[1], "Summary.") || built.Compactable {
			t.Fatalf("compacted context = %#v", built.Request.Input)
		}
	})
}

func TestCoordinatorDropsCompactionWithNothingToSummarize(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, compactInput(t, "compact", ""), stopInput(t, "stop", inbox.StopWhenIdle))
		run.assertStopped(t)
		if len(run.calls) != 0 || len(run.store.appendedTurns) != 0 {
			t.Fatal("compacted a conversation the model never answered")
		}
	})
}

func TestCoordinatorLetsCompactionFinishBeforeNewInput(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, textResponse("First answer."))
		run.input(t, compactInput(t, "compact", ""))
		run.input(t, externalEvent(t, 0, "during", "a new task"))
		if len(run.calls) != 2 || run.calls[1].ctx.Err() != nil {
			t.Fatal("new input interrupted the compaction")
		}
		run.respond(t, 1, textResponse("Summary."))
		if len(run.calls) != 3 {
			t.Fatal("the input that arrived during the compaction was not answered")
		}
		input := run.calls[2].request.Input
		if len(input) != 3 || !isSummary(input[1], "Summary.") || messageText(input[2]) != "a new task" {
			t.Fatalf("input after the compaction = %#v", input)
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.respond(t, 2, textResponse("Done."))
		run.assertStopped(t)
	})
}

func TestCoordinatorDoesNotCompactTheSummaryAgain(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 5000))
		run.input(t, externalEvent(t, 0, "second", "second task"))
		// A summary as large as the limit leaves the request over it.
		run.respond(t, 1, textResponse(strings.Repeat("A long summary. ", 400)))
		if len(run.calls) != 3 || isCompactionRequest(run.calls[2].request) {
			t.Fatal("compacted the summary instead of answering")
		}
		// Once the model has answered, a request over the limit compacts again.
		run.respond(t, 2, usageResponse("Second answer.", 5000))
		run.input(t, externalEvent(t, 0, "third", "third task"))
		if len(run.calls) != 4 || !isCompactionRequest(run.calls[3].request) {
			t.Fatal("did not compact again after the model answered")
		}
	})
}

func TestCoordinatorContinuesWhenCompactionHasNoSummary(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 5000))
		run.input(t, externalEvent(t, 0, "second", "second task"))
		run.respond(t, 1, llm.Response{Stop: llm.StopRefused})
		if len(run.calls) != 3 || isCompactionRequest(run.calls[2].request) {
			t.Fatal("a failed compaction stopped the work or repeated")
		}
		if !containsMessage(run.calls[2].request, "first task") || !containsMessage(run.calls[2].request, "second task") {
			t.Fatal("a failed compaction changed the conversation")
		}
	})
}

func TestCoordinatorStopsAutoCompactingAfterCompactionsWithoutSummary(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "task-0", "task 0"))
		run.respond(t, 0, usageResponse("Answer.", 5000))
		refused := llm.Response{Stop: llm.StopRefused}
		for attempt := 1; attempt <= compactionBreaker; attempt++ {
			if !answerTask(t, run, fmt.Sprint("task ", attempt), refused) {
				t.Fatalf("compaction %d did not start", attempt)
			}
		}
		if answerTask(t, run, "task after the failures", refused) {
			t.Fatal("kept compacting after three compactions without a summary")
		}
		// A requested compaction still runs, and its summary lets automatic
		// compaction resume.
		call := len(run.calls)
		run.input(t, compactInput(t, "compact", ""))
		if len(run.calls) != call+1 || !isCompactionRequest(run.calls[call].request) {
			t.Fatal("a requested compaction did not start")
		}
		run.respond(t, call, textResponse("Summary."))
		if answerTask(t, run, "task after the summary", refused) {
			t.Fatal("compacted the summary")
		}
		if !answerTask(t, run, "task that fills the context", textResponse("Summary.")) {
			t.Fatal("automatic compaction did not resume after a summary")
		}
	})
}

func TestCoordinatorStopsAutoCompactingWhenTheContextRefillsRapidly(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		items := recordItems(run)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "task-0", "task 0"))
		run.respond(t, 0, usageResponse("Answer.", 5000))
		// Every turn fills the context again, so each compaction after the
		// first is a rapid refill.
		for index := 1; index <= compactionBreaker; index++ {
			if !answerTask(t, run, fmt.Sprint("task ", index), textResponse("Summary.")) {
				t.Fatalf("compaction %d did not start", index)
			}
		}
		if answerTask(t, run, "task 4", textResponse("Summary.")) {
			t.Fatal("kept compacting a context that refills at once")
		}

		// A resumed run replays the session into the same state.
		resumed := newStopTestRun(t, 0)
		resumed.store.items = slices.Clone(*items)
		resumed.current.dependencies.AutoCompactTokens = 1000
		resumed.start(t)
		if got, want := compactionCountersOf(resumed.current.state), compactionCountersOf(run.current.state); got != want {
			t.Fatalf("resumed compaction state = %+v, want %+v", got, want)
		}
		if answerTask(t, resumed, "task 5", textResponse("Summary.")) {
			t.Fatal("the resumed run compacted a context that refills at once")
		}
		// rapidRefillTurns turns after the last compaction, the context no
		// longer refilled rapidly.
		if !answerTask(t, resumed, "task 6", textResponse("Summary.")) {
			t.Fatal("automatic compaction did not resume")
		}
		if resumed.current.state.rapidRefills != 0 {
			t.Fatalf("rapid refills = %d after a compaction that was none", resumed.current.state.rapidRefills)
		}
	})
}

func TestCoordinatorRequestedCompactionIsNoRapidRefill(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.start(t)
		run.input(t, externalEvent(t, 0, "task-0", "task 0"))
		run.respond(t, 0, usageResponse("Answer.", 5000))
		for index := 1; index < compactionBreaker; index++ {
			if !answerTask(t, run, fmt.Sprint("task ", index), textResponse("Summary.")) {
				t.Fatalf("compaction %d did not start", index)
			}
		}
		// The user asks for a compaction one turn after the last one.
		call := len(run.calls)
		run.input(t, compactInput(t, "compact", ""))
		if len(run.calls) != call+1 || !isCompactionRequest(run.calls[call].request) {
			t.Fatal("a requested compaction did not start")
		}
		run.respond(t, call, textResponse("Summary."))
		if answerTask(t, run, "task after the summary", textResponse("Summary.")) {
			t.Fatal("compacted the summary")
		}
		if !answerTask(t, run, "task that fills the context", textResponse("Summary.")) {
			t.Fatal("a requested compaction counted as a rapid refill")
		}
	})
}

// answerTask gives the run a task and reports whether the turn for it began
// with a compaction, which the model answers with summary. The model uses
// 5,000 tokens for the task, over the limit these tests set.
func answerTask(t *testing.T, run *stopTestRun, task string, summary llm.Response) bool {
	t.Helper()
	call := len(run.calls)
	run.input(t, externalEvent(t, 0, inbox.ID(strings.ReplaceAll(task, " ", "-")), task))
	if len(run.calls) != call+1 {
		t.Fatalf("%s started no turn", task)
	}
	compaction := isCompactionRequest(run.calls[call].request)
	if compaction {
		run.respond(t, call, summary)
		call++
		if len(run.calls) != call+1 || isCompactionRequest(run.calls[call].request) {
			t.Fatalf("no turn answered %s after the compaction", task)
		}
	}
	run.respond(t, call, usageResponse("Read a large file.", 5000))
	if !containsMessage(run.calls[call].request, task) && !compaction {
		t.Fatalf("the turn for %s lacks it", task)
	}
	return compaction
}

// recordItems collects the items the run stores, in order, for another run
// to resume from.
func recordItems(run *stopTestRun) *[]sessionstore.Item {
	items := slices.Clone(run.store.items)
	add := func(kind sessionstore.ItemKind, data any) {
		items = append(items, storedItem(sessionstore.Sequence(len(items)+1), kind, data))
	}
	run.store.onAppendInput = func(input inbox.Input) { add(sessionstore.ItemInput, input) }
	run.store.onAppendTurn = func(turn session.Turn) { add(sessionstore.ItemTurn, turn) }
	run.store.onAppendModelResponse = func(response sessionstore.ModelResponse) {
		add(sessionstore.ItemModelResponse, response)
	}
	run.store.onAppendToolCallStatus = func(status sessionstore.ToolCallStatus) {
		add(sessionstore.ItemToolCallStatus, status)
	}
	return &items
}

type compactionCounters struct {
	compacted, asked                        bool
	turnsSinceCompaction, failures, refills int
}

func compactionCountersOf(state loopState) compactionCounters {
	return compactionCounters{
		compacted:            state.compacted,
		asked:                state.compactionAsked,
		turnsSinceCompaction: state.turnsSinceCompaction,
		failures:             state.compactionFailures,
		refills:              state.rapidRefills,
	}
}

func usageResponse(text string, tokens int64) llm.Response {
	response := textResponse(text)
	response.Usage = llm.Usage{InputTokens: tokens - tokens/10, OutputTokens: tokens / 10}
	return response
}

func compactInput(t *testing.T, id inbox.ID, focus string) inbox.Input {
	t.Helper()
	payload, err := json.Marshal(inbox.ControlMessage{Mode: inbox.Compact, Reason: focus})
	if err != nil {
		t.Fatal(err)
	}
	return inbox.Input{ID: id, Kind: inbox.InputControl, Payload: payload}
}

func isCompactionRequest(request llm.Request) bool {
	return len(request.Input) != 0 && strings.HasPrefix(messageText(request.Input[len(request.Input)-1]), "CRITICAL: respond with text only, and do not call any tools.")
}

func containsMessage(request llm.Request, text string) bool {
	for _, item := range request.Input {
		if messageText(item) == text {
			return true
		}
	}
	return false
}

func assertTurnTypes(t *testing.T, run *stopTestRun, want ...session.TurnType) {
	t.Helper()
	var got []session.TurnType
	for _, turn := range run.store.appendedTurns {
		got = append(got, turn.Type)
	}
	if strings.Join(turnTypeNames(got), ",") != strings.Join(turnTypeNames(want), ",") {
		t.Fatalf("turn types = %v, want %v", got, want)
	}
}

func turnTypeNames(types []session.TurnType) []string {
	names := make([]string, len(types))
	for index, value := range types {
		names[index] = string(value)
	}
	return names
}

func TestCoordinatorFitsCompactionRequestToContextWindow(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 8_000
		run.current.dependencies.ContextWindow = 20_000
		huge := strings.Repeat("log line\n", 12_000)
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", huge))
		if len(run.calls) != 1 || isCompactionRequest(run.calls[0].request) {
			t.Fatal("compacted before the model answered")
		}
		run.respond(t, 0, textResponse("Read it."))
		run.input(t, externalEvent(t, 0, "second", "what failed?"))
		if len(run.calls) != 2 || !isCompactionRequest(run.calls[1].request) {
			t.Fatal("did not compact")
		}
		// The window leaves no room for the huge message: it is left out.
		request := run.calls[1].request
		if containsMessage(request, huge) || !strings.Contains(messageText(request.Input[1]), "left out") || !containsMessage(request, "what failed?") {
			t.Fatalf("compaction request = %.300v", request.Input)
		}
		run.respond(t, 1, textResponse("The user pasted a log."))
		if len(run.calls) != 3 || !isSummary(run.calls[2].request.Input[1], "The user pasted a log.") {
			t.Fatal("did not continue from the summary")
		}
	})
}

func TestCoordinatorCompactsARequestTheWindowCannotHold(t *testing.T) {
	for _, auto := range []bool{true, false} {
		t.Run(fmt.Sprint("auto=", auto), func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) {
				run := newStopTestRun(t, 0)
				// The estimate stays under the limit: the provider counts more.
				run.current.dependencies.AutoCompactTokens = 1_000_000
				if !auto {
					run.current.dependencies.AutoCompactTokens = 0
				}
				run.current.dependencies.ContextWindow = 20_000
				huge := strings.Repeat("log line\n", 4000)
				run.start(t)
				run.input(t, externalEvent(t, 0, "first", huge))
				run.respond(t, 0, usageResponse("Read it.", 10_000))
				run.input(t, externalEvent(t, 0, "second", "what failed?"))
				if len(run.calls) != 2 || isCompactionRequest(run.calls[1].request) {
					t.Fatal("compacted a request under the limit")
				}
				overflow := &llm.ContextOverflowError{Tokens: 21_000, Limit: 20_000, Err: errors.New("prompt is too long: 21000 tokens > 20000 maximum")}
				run.fail(t, 1, overflow)
				if !auto {
					// Without automatic compaction the error stands.
					select {
					case err := <-run.done:
						if !errors.Is(err, overflow) {
							t.Fatalf("Run error = %v", err)
						}
					default:
						t.Fatal("Run went on after a request the window cannot hold")
					}
					return
				}
				run.assertRunning(t)
				if len(run.calls) != 3 || !isCompactionRequest(run.calls[2].request) {
					t.Fatal("a request the window cannot hold was not compacted")
				}
				// Cut to what the window holds, in the measure of the estimate
				// the provider found short.
				request := run.calls[2].request
				if containsMessage(request, huge) || !strings.Contains(messageText(request.Input[1]), "left out") || !containsMessage(request, "what failed?") {
					t.Fatalf("compaction request = %.300v", request.Input)
				}
				run.respond(t, 2, textResponse("The user pasted a log."))
				if len(run.calls) != 4 {
					t.Fatal("the turn did not follow the summary")
				}
				input := run.calls[3].request.Input
				if len(input) != 3 || !isSummary(input[1], "The user pasted a log.") || messageText(input[2]) != "what failed?" {
					t.Fatalf("input after the compaction = %#v", input)
				}
				run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
				run.respond(t, 3, textResponse("Nothing failed."))
				run.assertStopped(t)
				assertTurnTypes(t, run, session.TurnRegular, session.TurnRegular, session.TurnCompaction, session.TurnRegular)
			})
		})
	}
}

func TestCoordinatorCutsDownACompactionRequestTheWindowCannotHold(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 8_000
		huge := strings.Repeat("log line\n", 4000)
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", huge))
		run.respond(t, 0, usageResponse("Read it.", 10_000))
		run.input(t, compactInput(t, "compact", "the log"))
		if len(run.calls) != 2 || !isCompactionRequest(run.calls[1].request) || !containsMessage(run.calls[1].request, huge) {
			t.Fatal("did not compact the whole conversation")
		}
		// The provider says no more than that the request is too large.
		run.fail(t, 1, &llm.ContextOverflowError{Err: errors.New("input is too long")})
		if len(run.calls) != 3 || len(run.store.appendedTurns) != 2 {
			t.Fatal("the compaction was not sent again in its turn")
		}
		request := run.calls[2].request
		if !isCompactionRequest(request) || containsMessage(request, huge) ||
			!strings.Contains(messageText(request.Input[len(request.Input)-1]), "focus on:\nthe log") {
			t.Fatalf("compaction request = %.300v", request.Input)
		}
		run.respond(t, 2, textResponse("The user pasted a log."))
		built, err := run.current.dependencies.ContextBuilder.Build()
		if err != nil || !isSummary(built.Request.Input[1], "The user pasted a log.") {
			t.Fatalf("compacted context = %#v, %v", built.Request.Input, err)
		}
	})
}

func TestCoordinatorGivesUpOnACompactionTheWindowNeverHolds(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 10_000))
		run.input(t, compactInput(t, "compact", ""))
		overflow := &llm.ContextOverflowError{Err: errors.New("input is too long")}
		for attempt := range overflowRetries {
			run.fail(t, 1+attempt, overflow)
			run.assertRunning(t)
		}
		run.fail(t, 1+overflowRetries, overflow)
		select {
		case err := <-run.done:
			if !errors.Is(err, overflow) {
				t.Fatalf("Run error = %v", err)
			}
		default:
			t.Fatal("Run went on after the last attempt")
		}
		if len(run.calls) != 2+overflowRetries {
			t.Fatalf("requests = %d", len(run.calls))
		}
	})
}

func TestOverflowBudget(t *testing.T) {
	for _, test := range []struct {
		name                string
		base, estimate      int64
		tokens, limit, want int64
	}{
		// 180,000 of the window is the request's, and the estimate runs
		// short by 210,000/150,000.
		{"sizes", 0, 150_000, 210_000, 200_000, 128_571},
		{"sizes over the base", 100_000, 150_000, 210_000, 200_000, 100_000},
		{"no sizes", 0, 150_000, 0, 0, 120_000},
		{"no sizes under the base", 100_000, 150_000, 0, 0, 100_000},
		{"nothing estimated", 0, 0, 0, 0, 1},
	} {
		overflow := &llm.ContextOverflowError{Tokens: test.tokens, Limit: test.limit, Err: errors.New("too long")}
		if got := overflowBudget(test.base, test.estimate, overflow); got != test.want {
			t.Errorf("%s: budget = %d, want %d", test.name, got, test.want)
		}
	}
}
