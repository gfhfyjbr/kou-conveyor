package coordinator

import (
	"context"
	"errors"
	"strings"
	"sync/atomic"
	"testing"
	"testing/synctest"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// lastMessage is the text of the last message of a request.
func lastMessage(request llm.Request) string {
	if len(request.Input) == 0 {
		return ""
	}
	return messageText(request.Input[len(request.Input)-1])
}

func countMessages(request llm.Request, prefix string) int {
	count := 0
	for _, item := range request.Input {
		if strings.HasPrefix(messageText(item), prefix) {
			count++
		}
	}
	return count
}

// countingAdapter counts the requests of the adapter it wraps. The count may
// be read while a timer, not the test, starts the next request, which
// run.calls may not.
type countingAdapter struct {
	llm.Adapter
	requests *atomic.Int64
}

func (adapter countingAdapter) Respond(ctx context.Context, request llm.Request, options llm.RequestOptions) (llm.Response, error) {
	adapter.requests.Add(1)
	return adapter.Adapter.Respond(ctx, request, options)
}

func TestCoordinatorWaitsForAUsageLimitToLift(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		var requests atomic.Int64
		run.current.dependencies.LLM = countingAdapter{Adapter: run.current.dependencies.LLM, requests: &requests}
		var retries []llm.Retry
		ctx := llm.WithRetryReporter(t.Context(), func(retry llm.Retry) { retries = append(retries, retry) })
		go func() { run.done <- run.current.Run(ctx) }()
		synctest.Wait()
		run.input(t, externalEvent(t, 0, "task", "work all night"), stopInput(t, "stop", inbox.StopWhenIdle))
		limit := &llm.RateLimitError{RetryAfter: time.Hour, Err: errors.New("429 limit reached")}
		run.fail(t, 0, limit)
		run.assertRunning(t)
		if len(retries) != 1 || retries[0].Delay != time.Hour || retries[0].Attempt != 1 || !strings.Contains(retries[0].Err.Error(), "limit reached") {
			t.Fatalf("retries = %+v", retries)
		}
		synctest.Sleep(time.Hour - time.Second)
		synctest.Wait()
		if requests.Load() != 1 {
			t.Fatal("the request went again before the limit lifted")
		}
		synctest.Sleep(time.Second)
		synctest.Wait()
		if len(run.calls) != 2 || !containsMessage(run.calls[1].request, "work all night") {
			t.Fatalf("requests = %d, want the turn again once the limit lifted", len(run.calls))
		}
		// A limit that says nothing of when it lifts is waited out a while;
		// a response resets the count.
		run.fail(t, 1, &llm.RateLimitError{Err: errors.New("429")})
		synctest.Sleep(defaultQuotaWait)
		synctest.Wait()
		if len(run.calls) != 3 {
			t.Fatalf("requests = %d after the default wait", len(run.calls))
		}
		run.respond(t, 2, textResponse("Done."))
		run.assertStopped(t)
		if run.current.state.quotaWaits != 0 {
			t.Fatal("a response did not reset the waits")
		}
	})
}

func TestCoordinatorGivesUpOnAUsageLimitThatDoesNotLift(t *testing.T) {
	for _, noRecovery := range []bool{false, true} {
		synctest.Test(t, func(t *testing.T) {
			run := newStopTestRun(t, 0)
			run.current.dependencies.NoRecovery = noRecovery
			run.start(t)
			run.input(t, externalEvent(t, 0, "task", "work"))
			limit := &llm.RateLimitError{RetryAfter: 10 * time.Hour, Err: errors.New("weekly limit")}
			waits := maxQuotaWaits
			if noRecovery {
				waits = 0
			}
			for index := range waits {
				run.fail(t, index, limit)
				run.assertRunning(t)
				// The wait is bounded.
				synctest.Sleep(maxQuotaWait)
				synctest.Wait()
			}
			run.fail(t, waits, limit)
			select {
			case err := <-run.done:
				if !errors.Is(err, limit) {
					t.Fatalf("Run error = %v", err)
				}
			default:
				t.Fatal("Run did not give up")
			}
		})
	}
}

func TestCoordinatorStopsAfterTheFinalReportOnceTheBudgetIsSpent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.MaxTurns = 1
		run.start(t)
		run.input(t, externalEvent(t, 0, "task", "endless work"))
		run.respond(t, 0, textResponse("Working on it."))
		if len(run.calls) != 2 || !strings.Contains(lastMessage(run.calls[1].request), "reached its limit of 1 model turns") {
			t.Fatalf("requests = %d, want one asking for the final report", len(run.calls))
		}
		run.assertRunning(t)
		run.respond(t, 1, textResponse("Final report."))
		run.assertStopped(t)
	})
}

func TestCoordinatorStopsWhenTheRunningTimeIsSpent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.MaxDuration = time.Hour
		run.start(t)
		run.input(t, externalEvent(t, 0, "task", "endless work"))
		synctest.Sleep(time.Hour)
		synctest.Wait()
		// The response in progress did not see the request for the report;
		// the next one answers it.
		run.respond(t, 0, textResponse("Still working."))
		run.assertRunning(t)
		if len(run.calls) != 2 || !strings.Contains(lastMessage(run.calls[1].request), "reached its limit of running time, 1h0m0s") {
			t.Fatalf("requests = %d, want one asking for the final report", len(run.calls))
		}
		run.respond(t, 1, textResponse("Final report."))
		run.assertStopped(t)
	})
}

func TestCoordinatorAsksForProgressReports(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.ReportEvery = 30 * time.Minute
		run.start(t)
		run.input(t, externalEvent(t, 0, "task", "long work"))
		synctest.Sleep(30 * time.Minute)
		synctest.Wait()
		// No turn has passed yet: there is nothing to report.
		run.respond(t, 0, textResponse("Started."))
		run.input(t, externalEvent(t, 0, "more", "go on"))
		synctest.Sleep(30 * time.Minute)
		synctest.Wait()
		run.respond(t, 1, textResponse("Going on."))
		if len(run.calls) != 3 || !strings.HasPrefix(lastMessage(run.calls[2].request), harnessPrefix+" Progress report") {
			t.Fatalf("requests = %d, want one asking for a report", len(run.calls))
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.respond(t, 2, textResponse("Report."))
		run.assertStopped(t)
	})
}

func TestCoordinatorRemindsBeforeCompactingAndHearsOfCompactions(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.current.dependencies.AutoCompactTokens = 1000
		run.current.dependencies.CompactionReminder = "Update your notes."
		var compactions []Compaction
		run.current.dependencies.Compacted = func(compaction Compaction) string {
			compactions = append(compactions, compaction)
			return "Workspace snapshot taken."
		}
		run.start(t)
		run.input(t, externalEvent(t, 0, "first", "first task"))
		run.respond(t, 0, usageResponse("First answer.", 850))
		run.input(t, externalEvent(t, 0, "second", "second task"))
		reminder := reminderPrefix + " Update your notes."
		if len(run.calls) != 2 || isCompactionRequest(run.calls[1].request) || lastMessage(run.calls[1].request) != reminder {
			t.Fatalf("requests = %d, want the reminder with the turn", len(run.calls))
		}
		run.respond(t, 1, usageResponse("Second answer.", 900))
		run.input(t, externalEvent(t, 0, "third", "third task"))
		if len(run.calls) != 3 || countMessages(run.calls[2].request, reminderPrefix) != 1 {
			t.Fatal("the reminder was sent twice before a compaction")
		}
		run.respond(t, 2, usageResponse("Third answer.", 5000))
		run.input(t, externalEvent(t, 0, "fourth", "fourth task"))
		if len(run.calls) != 4 || !isCompactionRequest(run.calls[3].request) {
			t.Fatal("an oversized request was not compacted")
		}
		run.respond(t, 3, textResponse("Summary."))
		if len(compactions) != 1 || compactions[0].Summary != "Summary." || compactions[0].Number != 1 {
			t.Fatalf("compactions = %+v", compactions)
		}
		if len(run.calls) != 5 || lastMessage(run.calls[4].request) != harnessPrefix+" Workspace snapshot taken." ||
			!containsMessage(run.calls[4].request, "fourth task") {
			t.Fatalf("requests = %d, want the runner's note after the compaction", len(run.calls))
		}
		run.input(t, stopInput(t, "stop", inbox.StopWhenIdle))
		run.respond(t, 4, usageResponse("Fourth answer.", 950))
		run.assertStopped(t)
		if run.current.state.reminded {
			t.Fatal("the compaction did not reset the reminder")
		}
	})
}

func TestChangesFilesKnowsTheFileToolsByAnyName(t *testing.T) {
	for name, want := range map[string]bool{
		"Edit": true, "Write": true, "apply_patch": true, "grace_Edit": true, "mcp__gateway__brain_Write": true,
		"Read": false, "Bash": false, "grace_Read": false, "edit": false,
	} {
		if changesFiles(name) != want {
			t.Errorf("changesFiles(%q) = %v", name, !want)
		}
	}
}
