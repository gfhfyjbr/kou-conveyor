package coordinator

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"strings"
	"testing"
	"testing/synctest"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

// The grace period of a response's calls is extended by every result that
// arrives while other calls run, within the limit.
func TestCoordinatorToolGraceExtendsForLateResults(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		run.current.dependencies.ToolGrace = 2 * time.Second
		run.current.dependencies.ToolGraceLimit = 5 * time.Second
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "run three tools"))
		run.respond(t, 0, toolGraceResponse("A", "B", "C"))
		started := time.Now()
		synctest.Sleep(1500 * time.Millisecond)
		updateToolGraceCall(t, run, "A", operation.StatusCompleted)
		// The original deadline, 2 s after the start, passes without a turn.
		synctest.Sleep(time.Until(started.Add(2*time.Second)) + 100*time.Millisecond)
		if run.requestCount() != 1 {
			t.Fatal("a result did not extend the grace period")
		}
		// The extension, 2 s after the result, is cut at the limit, 5 s
		// after the start: B arrives at 3.4 s and extends to 5 s, not 5.4 s.
		synctest.Sleep(time.Until(started.Add(3400 * time.Millisecond)))
		updateToolGraceCall(t, run, "B", operation.StatusCompleted)
		synctest.Sleep(time.Until(started.Add(5*time.Second)) - time.Nanosecond)
		if run.requestCount() != 1 {
			t.Fatal("the grace period ended before its limit")
		}
		synctest.Sleep(time.Nanosecond + 2*slurpIdleTimeout)
		if run.requestCount() != 2 {
			t.Fatal("the grace period did not end at its limit")
		}
		assertStopResult(t, run.calls[1].request, "A", string(operation.StatusCompleted))
		assertStopResult(t, run.calls[1].request, "B", string(operation.StatusCompleted))
		assertStopResult(t, run.calls[1].request, "C", contextbuilder.ToolCallRunningPayload)
	})
}

func harnessMessages(request llm.Request) []string {
	var found []string
	for _, item := range request.Input {
		if message, ok := item.Data.(llm.Message); ok && message.Role == llm.RoleUser && strings.HasPrefix(message.Text, "[harness") {
			found = append(found, message.Text)
		}
	}
	return found
}

func TestCoordinatorAsksToContinueAfterTheOutputLimit(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "write a long thing"), stopInput(t, "idle", inbox.StopWhenIdle))
		cut := textResponse("Once upon a")
		cut.Stop = llm.StopMaxOutputTokens
		for index := range maxContinues {
			run.respond(t, index, cut)
			run.assertRunning(t)
			if run.requestCount() != index+2 {
				t.Fatalf("after cut answer %d, requests = %d", index+1, run.requestCount())
			}
			messages := harnessMessages(run.calls[index+1].request)
			if len(messages) != index+1 || !strings.Contains(messages[index], "cut off at the output token limit") {
				t.Fatalf("harness messages = %q", messages)
			}
		}
		// Past the limit, the cut answer stands.
		run.respond(t, maxContinues, cut)
		run.assertStopped(t)
		if run.requestCount() != maxContinues+1 {
			t.Fatalf("requests = %d", run.requestCount())
		}
	})
}

func TestCoordinatorAsksAgainAfterARefusal(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "do it"), stopInput(t, "idle", inbox.StopWhenIdle))
		run.respond(t, 0, llm.Response{Stop: llm.StopRefused})
		run.assertRunning(t)
		if messages := harnessMessages(run.calls[1].request); len(messages) != 1 || !strings.Contains(messages[0], "was refused") {
			t.Fatalf("harness messages = %q", messages)
		}
		// A refusal again stands: the harness asks once.
		run.respond(t, 1, llm.Response{Stop: llm.StopRefused})
		run.assertStopped(t)
		if run.requestCount() != 2 {
			t.Fatalf("requests = %d", run.requestCount())
		}
	})
}

func TestCoordinatorRequestsAgainAfterAFailedResponse(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newStopTestRun(t, 0)
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "do it"), stopInput(t, "idle", inbox.StopWhenIdle))
		failed := llm.Response{Failure: &llm.Failure{Code: "server_error", Message: "try later"}}
		run.respond(t, 0, failed)
		run.respond(t, 1, failed)
		run.assertRunning(t)
		if run.requestCount() != 3 || len(harnessMessages(run.calls[2].request)) != 0 {
			t.Fatalf("requests = %d", run.requestCount())
		}
		run.respond(t, 2, failed)
		select {
		case err := <-run.done:
			if err == nil || !strings.Contains(err.Error(), "failed 3 times in a row") {
				t.Fatalf("err = %v", err)
			}
		default:
			t.Fatal("the run went on after the failures")
		}
	})
}

// A verifier whose checks are value operations; passed says how each ends.
type fakeVerifier struct {
	specs   int
	reports []string
	passed  func(operation.Operation) bool
}

func (verifier *fakeVerifier) Spec() (operation.Spec, bool) {
	verifier.specs++
	spec, _ := operation.NewValueSpec(jsontext.Value(`"check"`))
	return spec, true
}

func (verifier *fakeVerifier) Report(check operation.Operation) (string, bool) {
	report := "go test ./... failed (exit 1):\nFAIL: TestX"
	passed := verifier.passed != nil && verifier.passed(check)
	verifier.reports = append(verifier.reports, report)
	return report, passed
}

func TestCoordinatorVerifiesTheWorkBeforeGoingIdle(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		verifier := &fakeVerifier{}
		run.current.dependencies.Verifier = verifier
		run.current.dependencies.MaxVerifications = 2
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "fix the tests"), stopInput(t, "idle", inbox.StopWhenIdle))
		// No tool calls yet: nothing to verify, the run goes idle.
		run.respond(t, 0, textResponse("Nothing to do."))
		run.assertStopped(t)
		if verifier.specs != 0 {
			t.Fatal("verified without any work")
		}
	})
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		verifier := &fakeVerifier{}
		run.current.dependencies.Verifier = verifier
		run.current.dependencies.MaxVerifications = 2
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "fix the tests"), stopInput(t, "idle", inbox.StopWhenIdle))
		run.respond(t, 0, toolGraceResponse("A"))
		updateToolGraceCall(t, run, "A", operation.StatusCompleted)
		synctest.Sleep(2 * time.Second)
		run.respond(t, 1, textResponse("Fixed."))
		run.assertRunning(t)
		// The check runs as an operation the coordinator owns.
		if verifier.specs != 1 || len(run.operations.adds) != 2 || !isVerification(run.operations.adds[1]) {
			t.Fatalf("specs = %d, adds = %#v", verifier.specs, run.operations.adds)
		}
		check := run.operations.adds[1]
		check.Status = operation.StatusCompleted
		run.operations.updates <- check
		synctest.Wait()
		synctest.Sleep(2 * slurpIdleTimeout)
		synctest.Wait()
		// It failed: the model reads the report.
		run.assertRunning(t)
		if run.requestCount() != 3 {
			t.Fatalf("requests = %d", run.requestCount())
		}
		if messages := harnessMessages(run.calls[2].request); len(messages) != 1 || !strings.HasPrefix(messages[0], verificationPrefix+" go test ./... failed") {
			t.Fatalf("harness messages = %q", messages)
		}
		// Without new work the checks are not run again.
		run.respond(t, 2, textResponse("I see, but that is expected."))
		run.assertStopped(t)
		if verifier.specs != 1 {
			t.Fatalf("specs = %d", verifier.specs)
		}
	})
	synctest.Test(t, func(t *testing.T) {
		run := newToolGraceTestRun(t)
		verifier := &fakeVerifier{passed: func(operation.Operation) bool { return true }}
		run.current.dependencies.Verifier = verifier
		run.start(t)
		run.input(t, externalEvent(t, 0, "input", "fix the tests"), stopInput(t, "idle", inbox.StopWhenIdle))
		run.respond(t, 0, toolGraceResponse("A"))
		updateToolGraceCall(t, run, "A", operation.StatusCompleted)
		synctest.Sleep(2 * time.Second)
		run.respond(t, 1, textResponse("Fixed."))
		run.assertRunning(t)
		check := run.operations.adds[1]
		check.Status = operation.StatusCompleted
		run.operations.updates <- check
		synctest.Wait()
		synctest.Sleep(2 * slurpIdleTimeout)
		synctest.Wait()
		// It passed: the run is idle, and the model was not woken.
		run.assertStopped(t)
		if run.requestCount() != 2 {
			t.Fatalf("requests = %d", run.requestCount())
		}
		var saved []string
		for _, status := range run.store.appendedStatuses {
			if len(status.Operations) == 1 && isVerification(status.Operations[0]) {
				saved = append(saved, "status:"+string(status.Operations[0].Status))
			}
		}
		for _, value := range run.store.savedOperations {
			if isVerification(value) {
				saved = append(saved, string(value.Status))
			}
		}
		if strings.Join(saved, " ") != "status:ready completed" {
			t.Fatalf("saved verification = %v", saved)
		}
	})
}

func TestCoordinatorRestoresVerificationCounts(t *testing.T) {
	run := newStopTestRun(t, 0)
	verification, _ := json.Marshal(inbox.ControlMessage{Mode: inbox.Heartbeat, Reason: verificationPrefix + " failed"})
	items := []inbox.Input{
		externalEvent(t, 0, "prompt", "go"),
		{ID: "v1", Kind: inbox.InputControl, Payload: verification},
		{ID: "v2", Kind: inbox.InputControl, Payload: verification},
	}
	for _, input := range items {
		if _, err := run.current.addItemToLocalState(storedItem(1, "input", input)); err != nil {
			t.Fatal(err)
		}
	}
	if run.current.state.verifications != 2 {
		t.Fatalf("verifications = %d", run.current.state.verifications)
	}
	if _, err := run.current.addItemToLocalState(storedItem(1, "input", externalEvent(t, 1, "prompt-2", "again"))); err != nil {
		t.Fatal(err)
	}
	if run.current.state.verifications != 0 {
		t.Fatalf("verifications = %d after a prompt", run.current.state.verifications)
	}
}
