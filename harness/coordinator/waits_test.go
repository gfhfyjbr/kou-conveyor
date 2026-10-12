package coordinator

import (
	"strings"
	"testing"
	"testing/synctest"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// waitingTranslator is a tool whose calls wait for others (tool.Waiter).
type waitingTranslator struct{ operationStatusTranslator }

func (waitingTranslator) Waits() bool { return true }

// progressReports counts the progress reports the run was asked for.
func progressReports(t *testing.T, run *heartbeatTestRun) int {
	t.Helper()
	run.mu.Lock()
	defer run.mu.Unlock()
	count := 0
	for _, input := range run.recordedInputs {
		if input.Kind != inbox.InputControl {
			continue
		}
		control, err := input.DecodeControlMessage()
		if err != nil {
			t.Fatal(err)
		}
		if strings.HasPrefix(control.Reason, harnessPrefix+" Progress report") {
			count++
		}
	}
	return count
}

func TestCoordinatorLetsARunWaitForOthersInPeace(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newHeartbeatTestRun(t, 2)
		run.current.dependencies.Tools = tool.NewRegistry(tool.StaticTranslators{ViewImage: waitingTranslator{}}, tool.ViewImageName)
		run.current.dependencies.ReportEvery = 30 * time.Minute
		run.current.dependencies.ToolHeartbeatInterval = 10 * time.Minute
		run.start(t)

		// Calls that wait for others are checked on four times less often
		// from the first: they end by their own timeouts.
		advanceHeartbeatTime(40*time.Minute - time.Second)
		if run.requestCount() != 0 {
			t.Fatalf("requests = %d before the first check-in", run.requestCount())
		}
		advanceHeartbeatTime(time.Second + 2*slurpIdleTimeout)
		assertHeartbeatCount(t, run, 1)
		if reason := latestHeartbeatReason(t, run); !strings.HasPrefix(reason, "Heartbeat: waited 2400 seconds for tool calls.") {
			t.Fatalf("heartbeat reason = %q", reason)
		}
		run.respond(t, 0, textResponse("Still waiting."))

		// A turn has passed, but a run that only waits for others is not
		// asked for a report: it has done nothing meanwhile.
		advanceHeartbeatTime(40*time.Minute - 2*slurpIdleTimeout - time.Second)
		if run.requestCount() != 1 {
			t.Fatalf("requests = %d: the run was woken while it waited", run.requestCount())
		}

		if reports := progressReports(t, run); reports != 0 {
			t.Fatalf("%d reports asked for while the run waited", reports)
		}

		// One of them answers, and the run works on it: the report held is
		// asked for a period later.
		run.update(t, 0, operation.StatusCompleted)
		if run.requestCount() != 2 {
			t.Fatalf("requests = %d, want the result given", run.requestCount())
		}
		assertCompletedResults(t, run.calls[1].request, 1)
		advanceHeartbeatTime(29 * time.Minute)
		if reports := progressReports(t, run); reports != 0 {
			t.Fatalf("%d reports asked for as soon as the run worked", reports)
		}
		advanceHeartbeatTime(2 * time.Minute)
		if reports := progressReports(t, run); reports != 1 {
			t.Fatalf("%d reports asked for a period after the run worked", reports)
		}
		run.respond(t, 1, textResponse("One is in."))
		if run.requestCount() != 3 || !strings.HasPrefix(lastMessage(run.calls[2].request), harnessPrefix+" Progress report") {
			t.Fatalf("requests = %d, want one asking for the report held", run.requestCount())
		}
		run.respond(t, 2, textResponse("Report."))
		run.update(t, 1, operation.StatusCompleted)
		run.respond(t, 3, textResponse("Both are in."))
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.assertStopped(t)
	})
}

func TestCoordinatorReportsWhileOtherToolsRun(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		// Of two calls, one waits for others and one works (Bash): the run
		// works, and is asked for reports as ever.
		run := newHeartbeatTestRun(t, 2)
		run.current.dependencies.Tools = tool.NewRegistry(tool.StaticTranslators{ViewImage: waitingTranslator{}, Bash: operationStatusTranslator{}}, tool.ViewImageName, tool.BashName)
		response := run.store.items[1].Data.(sessionstore.ModelResponse)
		call := response.Response.Output[1].Data.(llm.ToolCall)
		call.Name = tool.BashName
		response.Response.Output[1].Data = call
		run.store.items[1].Data = response
		run.current.dependencies.ReportEvery = 30 * time.Minute
		run.start(t)
		run.input(t, externalEvent(t, 0, "question", "how is it going?"))
		run.respond(t, 0, textResponse("Working."))
		advanceHeartbeatTime(30 * time.Minute)
		if run.requestCount() != 2 || !strings.HasPrefix(lastMessage(run.calls[1].request), harnessPrefix+" Progress report") {
			t.Fatalf("requests = %d, want one asking for a report", run.requestCount())
		}
		run.respond(t, 1, textResponse("Report."))
		run.update(t, 0, operation.StatusCompleted)
		run.update(t, 1, operation.StatusCompleted)
		for index := 2; index < run.requestCount(); index++ {
			run.respond(t, index, textResponse("Done."))
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.assertStopped(t)
	})
}
