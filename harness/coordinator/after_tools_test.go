package coordinator

import (
	"fmt"
	"slices"
	"strings"
	"testing"
	"testing/synctest"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// Input delivered after tools is what a user sends while the agent works:
// it never cuts a response short, waits for the tool calls the model just
// made, and follows their results.

func TestCoordinatorInputAfterToolsLetsTheModelFinish(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		log := recordSession(run)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "write the report"))
		run.input(t, afterTools(t, "steer", "mention the costs too"))
		if run.requestCount() != 1 || run.calls[0].ctx.Err() != nil {
			t.Fatal("input delivered after tools interrupted the model")
		}
		if slices.Contains(*log, "input steer") {
			t.Fatal("input was recorded before the model read it")
		}
		run.respond(t, 0, textResponse("Here is the report."))
		if run.requestCount() != 2 {
			t.Fatal("the answer did not let the waiting input start a turn")
		}
		assertTail(t, run.calls[1].request, "assistant: Here is the report.", "user: mention the costs too")
		assertOrder(t, *log, "response", "input steer", "turn")
	})
}

func TestCoordinatorInputAfterToolsWaitsForTheStepsToolCalls(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		log := recordSession(run)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "run the tests"))
		run.input(t, afterTools(t, "steer", "use the race detector next time"))
		run.respond(t, 0, toolGraceResponse("A"))
		// Long past the grace period, the input still waits for the call.
		synctest.Sleep(10 * toolCallRunGracePeriod)
		synctest.Wait()
		if run.requestCount() != 1 {
			t.Fatal("input went out before the tool call it waits for finished")
		}
		updateToolGraceCall(t, run, "A", operation.StatusCompleted)
		if run.requestCount() != 2 {
			t.Fatal("the finished call did not deliver the waiting input")
		}
		assertStopResult(t, run.calls[1].request, "A", string(operation.StatusCompleted))
		assertTail(t, run.calls[1].request, "result A: completed", "user: use the race detector next time")
		assertOrder(t, *log, "status A completed", "input steer", "turn")
	})
}

func TestCoordinatorInputAfterToolsGoesOutWithAnEarlierRequest(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "build and test"))
		run.respond(t, 0, toolGraceResponse("A", "B"))
		run.input(t, afterTools(t, "steer", "skip the slow tests"))
		updateToolGraceCall(t, run, "A", operation.StatusCompleted)
		if run.requestCount() != 1 {
			t.Fatal("the input cut the grace period short")
		}
		// The grace period ends with A's result, and the request that
		// carries it takes the input along instead of holding it back.
		synctest.Sleep(toolCallRunGracePeriod)
		synctest.Wait()
		if run.requestCount() != 2 {
			t.Fatal("the result was not delivered when the grace period ended")
		}
		assertStopResult(t, run.calls[1].request, "B", contextbuilder.ToolCallRunningPayload)
		assertTail(t, run.calls[1].request, "user: skip the slow tests")
		run.respond(t, 1, textResponse("Skipping them."))
		updateToolGraceCall(t, run, "B", operation.StatusCompleted)
		if run.requestCount() != 3 || strings.Count(fmt.Sprint(describe(run.calls[2].request)), "skip the slow tests") != 1 {
			t.Fatal("the input was not delivered exactly once")
		}
	})
}

func TestCoordinatorInputAfterToolsDoesNotWaitForOlderCalls(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "start the server"))
		run.respond(t, 0, toolGraceResponse("A"))
		// The model hears the call is still running and ends its turn.
		run.input(t, externalEvent(t, 1, "check", "is it up?"))
		run.respond(t, 1, textResponse("It is starting; I will wait."))
		run.input(t, afterTools(t, "steer", "use port 8080"))
		if run.requestCount() != 3 {
			t.Fatal("input waited for a call of an earlier step")
		}
		assertStopResult(t, run.calls[2].request, "A", contextbuilder.ToolCallRunningPayload)
		assertTail(t, run.calls[2].request, "user: use port 8080")
	})
}

func TestCoordinatorInputAfterToolsKeepsItsOrder(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		log := recordSession(run)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "refactor"))
		run.input(t, afterTools(t, "first", "one"))
		run.input(t, afterTools(t, "second", "two"))
		// Input delivered at once interrupts, and what waits goes first.
		run.input(t, externalEvent(t, 3, "now", "three"))
		if run.requestCount() != 2 || run.calls[0].ctx.Err() == nil {
			t.Fatal("input delivered at once did not interrupt the model")
		}
		assertTail(t, run.calls[1].request, "user: one", "user: two", "user: three")
		assertOrder(t, *log, "input first", "input second", "input now", "turn")
	})
}

func TestCoordinatorInputAfterToolsKeepsTheRunGoing(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "summarize"), stopInput(t, "stop", inbox.StopWhenIdle))
		run.input(t, afterTools(t, "steer", "in French"))
		run.respond(t, 0, textResponse("Summary."))
		run.assertRunning(t)
		if run.requestCount() != 2 {
			t.Fatal("the run ended with input waiting")
		}
		assertTail(t, run.calls[1].request, "user: in French")
		run.respond(t, 1, textResponse("Résumé."))
		run.assertStopped(t)
	})
}

func TestCoordinatorInputAfterToolsGoesOutWithAHeartbeat(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.current.dependencies.ToolHeartbeatInterval = time.Minute
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "run the migration"))
		run.respond(t, 0, toolGraceResponse("A"))
		run.input(t, afterTools(t, "steer", "and back up first"))
		advanceHeartbeatTime(time.Minute + 2*slurpIdleTimeout)
		if run.requestCount() != 2 || countHeartbeatMessages(run.calls[1].request) != 1 {
			t.Fatal("a call that runs on kept the input past the heartbeat")
		}
		assertTail(t, run.calls[1].request, "user: and back up first")
	})
}

func TestCoordinatorInputAfterToolsWaitsForToolsOnlySoLong(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.current.dependencies.ToolWaitLimit = time.Minute
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "start the dev server"))
		run.input(t, afterTools(t, "steer", "use port 8080"))
		// The limit counts from the end of the response, when the input
		// starts waiting for the server, which never finishes.
		synctest.Sleep(10 * time.Minute)
		run.respond(t, 0, toolGraceResponse("A"))
		synctest.Sleep(time.Minute - time.Second)
		synctest.Wait()
		if run.requestCount() != 1 {
			t.Fatal("the input went out before the limit")
		}
		synctest.Sleep(2 * time.Second)
		synctest.Wait()
		if run.requestCount() != 2 {
			t.Fatal("the input waited past the limit")
		}
		assertStopResult(t, run.calls[1].request, "A", contextbuilder.ToolCallRunningPayload)
		assertTail(t, run.calls[1].request, "user: use port 8080")
		// The next input waits afresh.
		run.respond(t, 1, toolGraceResponse("B"))
		run.input(t, afterTools(t, "again", "and log requests"))
		synctest.Sleep(30 * time.Second)
		synctest.Wait()
		if run.requestCount() != 2 {
			t.Fatal("the limit carried over to the next input")
		}
		updateToolGraceCall(t, run, "B", operation.StatusCompleted)
		if run.requestCount() != 3 {
			t.Fatal("the finished call did not deliver the input")
		}
		assertTail(t, run.calls[2].request, "result B: completed", "user: and log requests")
	})
}

func TestCoordinatorHardStopLeavesInputAfterToolsUnrecorded(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		log := recordSession(run)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "deploy"))
		run.respond(t, 0, toolGraceResponse("A"))
		run.input(t, afterTools(t, "steer", "to staging"))
		run.input(t, stopInput(t, "stop", inbox.StopHard))
		updateToolGraceCall(t, run, "A", operation.StatusCanceled)
		run.assertStopped(t)
		if slices.Contains(*log, "input steer") || run.requestCount() != 1 {
			t.Fatalf("a stopped run recorded or sent the waiting input: %v", *log)
		}
	})
}

func afterTools(t *testing.T, id inbox.ID, text string) inbox.Input {
	t.Helper()
	input := externalEvent(t, 0, id, text)
	input.Delivery = inbox.DeliverAfterTools
	return input
}

// recordSession logs what the coordinator appends to the session, in order.
func recordSession(run *stopTestRun) *[]string {
	var log []string
	store := run.store
	store.onAppendInput = func(input inbox.Input) {
		if input.Kind == inbox.InputExternal {
			log = append(log, "input "+string(input.ID))
		}
	}
	store.onAppendTurn = func(session.Turn) { log = append(log, "turn") }
	store.onAppendModelResponse = func(sessionstore.ModelResponse) { log = append(log, "response") }
	store.onAppendToolCallStatus = func(status sessionstore.ToolCallStatus) {
		state := "running"
		for _, value := range status.Operations {
			if operationIsTerminal(value.Status) {
				state = string(value.Status)
			}
		}
		log = append(log, "status "+status.CallID+" "+state)
	}
	return &log
}

// assertOrder checks that the session's items end with want, in order.
func assertOrder(t *testing.T, log []string, want ...string) {
	t.Helper()
	if len(log) < len(want) || !slices.Equal(log[len(log)-len(want):], want) {
		t.Fatalf("session ends with %q, want %q", log, want)
	}
}

// assertTail checks the last items of a request.
func assertTail(t *testing.T, request llm.Request, want ...string) {
	t.Helper()
	got := describe(request)
	if len(got) < len(want) || !slices.Equal(got[len(got)-len(want):], want) {
		t.Fatalf("request ends with %q, want %q", got, want)
	}
}

func describe(request llm.Request) []string {
	var items []string
	for _, item := range request.Input {
		switch data := item.Data.(type) {
		case llm.Message:
			items = append(items, string(data.Role)+": "+data.Text)
		case llm.ToolCall:
			items = append(items, "call "+data.CallID)
		case llm.ToolResult:
			items = append(items, "result "+data.CallID+": "+data.Output[0].Value)
		}
	}
	return items
}
