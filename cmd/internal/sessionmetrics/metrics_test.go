package sessionmetrics

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// sampleLog writes a session log as the session store does, at the times the
// test says.
type sampleLog struct {
	t        *testing.T
	b        bytes.Buffer
	at       time.Time
	sequence sessionstore.Sequence
	inputs   int
	turns    int
	last     session.TurnID
}

func newSampleLog(t *testing.T, start time.Time) *sampleLog {
	l := &sampleLog{t: t, at: start}
	l.record("session", struct {
		Version int
		Session session.Session
	}{2, session.Session{ID: "metrics-session", CreatedAt: start}})
	return l
}

func (l *sampleLog) record(kind string, data any) {
	l.t.Helper()
	line, err := json.Marshal(struct {
		Type string `json:"type"`
		Data any    `json:"data"`
	}{kind, data})
	if err != nil {
		l.t.Fatal(err)
	}
	l.b.Write(line)
	l.b.WriteByte('\n')
}

func (l *sampleLog) item(after time.Duration, kind sessionstore.ItemKind, data any, operations ...operation.Operation) {
	l.t.Helper()
	l.at = l.at.Add(after)
	l.sequence++
	l.record("item", struct {
		Item       sessionstore.Item
		Operations []operation.Operation `json:",omitempty"`
	}{sessionstore.Item{Sequence: l.sequence, RecordedAt: l.at, Kind: kind, Data: data}, operations})
}

func (l *sampleLog) input(after time.Duration, kind inbox.InputKind, payload any, delivery inbox.Delivery) {
	l.t.Helper()
	encoded, err := json.Marshal(payload)
	if err != nil {
		l.t.Fatal(err)
	}
	l.inputs++
	l.item(after, sessionstore.ItemInput, inbox.Input{
		ID: inbox.ID(fmt.Sprintf("input-%d", l.inputs)), Kind: kind, Payload: jsontext.Value(encoded), Delivery: delivery,
	})
}

func (l *sampleLog) prompt(after time.Duration, text string, delivery inbox.Delivery) {
	l.t.Helper()
	l.input(after, inbox.InputExternal, text, delivery)
}

func (l *sampleLog) control(after time.Duration, mode inbox.ControlMode, reason string) {
	l.t.Helper()
	l.input(after, inbox.InputControl, inbox.ControlMessage{Mode: mode, Reason: reason}, "")
}

// settings records what a cockpit sends before a prompt that opens a run.
func (l *sampleLog) settings(after time.Duration) {
	l.t.Helper()
	l.input(after, inbox.InputControl, inbox.ControlMessage{
		Mode: inbox.UpdateSettings, Parameters: inbox.Settings{ReasoningEffort: llm.ReasoningEffortHigh},
	}, "")
}

func (l *sampleLog) respond(after time.Duration, kind session.TurnType, usage llm.Usage, compacted *sessionstore.Compacted, output ...llm.Item) {
	l.t.Helper()
	l.turns++
	id := session.TurnID(fmt.Sprintf("turn-%d", l.turns))
	l.item(after, sessionstore.ItemTurn, session.Turn{ID: id, PreviousTurnID: l.last, Type: kind})
	l.item(0, sessionstore.ItemModelResponse, sessionstore.ModelResponse{
		TurnID:    id,
		Response:  llm.Response{ID: fmt.Sprintf("response-%d", l.turns), Stop: llm.StopComplete, Output: output, Usage: usage},
		Compacted: compacted,
	})
	l.last = id
}

func (l *sampleLog) started(call string, id operation.ID) {
	l.t.Helper()
	l.item(0, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: l.last, CallID: call, Status: tool.CallStatus{WaitingFor: []operation.ID{id}},
	}, operation.Operation{ID: id, Type: "test", Version: 1, Status: operation.StatusReady})
}

func (l *sampleLog) ended(after time.Duration, id operation.ID) {
	l.t.Helper()
	l.at = l.at.Add(after)
	l.record("operation", struct{ Operation operation.Operation }{
		operation.Operation{ID: id, Type: "test", Version: 1, Status: operation.StatusCompleted},
	})
}

func (l *sampleLog) failed(call, problem string) {
	l.t.Helper()
	l.item(0, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: l.last, CallID: call, Status: tool.CallStatus{Error: problem},
	})
}

func usage(input, read, written, output int64) llm.Usage {
	return llm.Usage{InputTokens: input, CachedInputTokens: read, CacheWriteInputTokens: written, OutputTokens: output}
}

func thinking() llm.Item {
	return llm.Item{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"think"}}}
}

func text(value string) llm.Item {
	return llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: value}}
}

func call(id, name, arguments string) llm.Item {
	return llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: arguments}}
}

func bash(id, command string) llm.Item {
	arguments, _ := json.Marshal(map[string]string{"command": command})
	return call(id, "Bash", string(arguments))
}

var start = time.Date(2026, 10, 1, 10, 0, 0, 0, time.UTC)

// sample is a session with each thing the metrics count. It returns the log
// and the time the turn after its compaction began.
func sample(t *testing.T) ([]byte, time.Time) {
	l := newSampleLog(t, start)
	regular, compaction := session.TurnRegular, session.TurnCompaction

	l.settings(0)
	l.prompt(0, "Do the task", "")
	// A cold start before any compaction: cost 96000 + 2000 + 5000.
	l.respond(time.Second, regular, usage(50000, 0, 48000, 1000), nil, thinking(), bash("call-1", "go test ./..."))
	l.started("call-1", "op-1")
	l.ended(time.Minute, "op-1")
	// Only waits: 5000 + 4000 + 2500.
	l.respond(time.Second, regular, usage(52000, 50000, 2000, 500), nil, bash("call-2", "sleep 60"))
	l.started("call-2", "op-2")
	l.ended(time.Minute, "op-2")
	l.control(time.Second, inbox.Heartbeat, "[harness] Progress report: write where you are.")
	// 5200 + 2000 + 1000.
	l.respond(time.Second, regular, usage(53000, 52000, 1000, 200), nil, text("still going"))
	clipped := "the start […12 characters left out …] the end"
	// 5300 + 2000 + 15000.
	l.respond(time.Second, compaction, usage(54000, 53000, 1000, 3000),
		&sessionstore.Compacted{Message: "the summary", Notes: &clipped}, text("the summary"))
	l.control(time.Second, inbox.Heartbeat, "[harness] Your notes in a.notes.md are 40 KB. Rewrite them shorter now, under 32 KB.")
	compacted := l.at.Add(time.Second)
	// A cold start after the compaction: 300 + 84000 + 500.
	l.respond(time.Second, regular, usage(45000, 3000, 42000, 100), nil, thinking(), bash("call-5", "make"))
	l.started("call-5", "op-5")
	l.ended(time.Minute, "op-5")
	l.prompt(0, "[canvas] reply from planner: ok", inbox.DeliverAfterTools)
	// 4500 + 4000 + 500.
	l.respond(time.Second, regular, usage(47000, 45000, 2000, 100), nil, text("noted"))
	l.prompt(time.Minute, "[canvas] from planner: the next step", "")
	// The third API turn since the compaction, and the first had thinking:
	// a thinking window rewrite, 2000 + 120000 + 500.
	l.respond(time.Second, regular, usage(80000, 20000, 60000, 100), nil, bash("call-7", "ls"))
	l.started("call-7", "op-7")
	l.ended(time.Second, "op-7")
	// The input shrank by 20000: a prune rewrite, 600 + 108000 + 500.
	l.respond(time.Second, regular, usage(60000, 6000, 54000, 100), nil, text("done"))
	l.settings(2 * time.Hour)
	l.prompt(0, "back", "")
	// A cold start after a pause over the TTL, and a new run rewrite: 100 +
	// 122000 + 500.
	l.respond(time.Second, regular, usage(62000, 1000, 61000, 100), nil, call("call-9", "Read", `{"path":"a","offset":"5"}`))
	l.failed("call-9", `decode Read arguments: json: cannot unmarshal JSON string into Go int within "/offset"`)
	l.control(time.Minute, inbox.StopHard, "stopped from the cockpit")
	// A line being written.
	l.b.WriteString(`{"type":"item","data":{"Item":`)
	return l.b.Bytes(), compacted
}

func read(t *testing.T, log []byte, options Options) Report {
	t.Helper()
	report, err := Read(bytes.NewReader(log), options)
	if err != nil {
		t.Fatal(err)
	}
	return report
}

func near(a, b float64) bool { return math.Abs(a-b) < 1e-6 }

func parts(list []Part) map[string][2]float64 {
	out := map[string][2]float64{}
	for _, part := range list {
		out[part.Name] = [2]float64{float64(part.Count), part.Cost}
	}
	return out
}

func TestReadMeasuresASession(t *testing.T) {
	log, _ := sample(t)
	r := read(t, log, Options{})

	if r.Session != "metrics-session" {
		t.Errorf("Session = %q", r.Session)
	}
	if !r.First.Equal(start) || !r.Last.Equal(start.Add(2*time.Hour+5*time.Minute+12*time.Second)) {
		t.Errorf("First, Last = %v, %v", r.First, r.Last)
	}
	if r.Turns != 9 || r.Requests != 9 || r.APITurns != 5 {
		t.Errorf("Turns, Requests, APITurns = %d, %d, %d; want 9, 9, 5", r.Turns, r.Requests, r.APITurns)
	}
	if !near(r.Cost, 593000) {
		t.Errorf("Cost = %v, want 593000", r.Cost)
	}
	if want := (Tokens{Input: 503000, Read: 230000, Written: 271000, Output: 5200}); r.Tokens != want {
		t.Errorf("Tokens = %+v, want %+v", r.Tokens, want)
	}

	if r.Rewrites.Count != 3 || !near(r.Rewrites.Cost, 350000) || !near(r.Rewrites.Share, 350000.0/593000) {
		t.Errorf("Rewrites = %d, %v, %v", r.Rewrites.Count, r.Rewrites.Cost, r.Rewrites.Share)
	}
	if got, want := parts(r.Rewrites.Causes), map[string][2]float64{
		"new run": {1, 122000}, "thinking window": {1, 120000}, "prune": {1, 108000}, "other": {0, 0},
	}; !reflect.DeepEqual(got, want) {
		t.Errorf("rewrite causes = %v, want %v", got, want)
	}
	if want := map[string]int{"new run": 1, "thinking window": 1, "prune": 1}; !reflect.DeepEqual(r.Rewrites.Also, want) {
		t.Errorf("Also = %v, want %v", r.Rewrites.Also, want)
	}
	var causes [][]string
	for _, d := range r.Rewrites.List {
		causes = append(causes, d.Causes)
	}
	if want := [][]string{{"thinking window"}, {"prune"}, {"new run"}}; !reflect.DeepEqual(causes, want) {
		t.Errorf("listed causes = %v, want %v", causes, want)
	}
	if last := r.Rewrites.List[2]; !last.Compacted || last.Pause < 120 || last.Read != 1000 || last.Written != 61000 {
		t.Errorf("the last rewrite = %+v", last)
	}

	if r.ColdStarts.Count != 3 || !near(r.ColdStarts.Cost, 302000) {
		t.Errorf("ColdStarts = %d, %v", r.ColdStarts.Count, r.ColdStarts.Cost)
	}
	if got, want := parts(r.ColdStarts.Parts), map[string][2]float64{
		"after compaction": {1, 84000}, "pause over TTL": {1, 122000}, "before compactions": {1, 96000},
	}; !reflect.DeepEqual(got, want) {
		t.Errorf("cold starts = %v, want %v", got, want)
	}

	if r.Waits.Requests != 1 || !near(r.Waits.Cost, 11500) || r.Waits.Calls != 1 {
		t.Errorf("Waits = %+v", r.Waits)
	}
	if want := (Messages{Count: 4, AfterTools: 1, OpenedRun: 2, FromCanvas: 2, Replies: 1}); r.Messages != want {
		t.Errorf("Messages = %+v, want %+v", r.Messages, want)
	}
	if want := (Compactions{Count: 1, Requests: 1, Kept: 1, NotesClipped: 1, OverflowAsks: 1}); r.Compactions != want {
		t.Errorf("Compactions = %+v, want %+v", r.Compactions, want)
	}
	if want := map[string]int{"progress report": 1, "after compaction": 1}; !reflect.DeepEqual(r.Heartbeats, want) {
		t.Errorf("Heartbeats = %v, want %v", r.Heartbeats, want)
	}
	if len(r.Stops) != 1 || r.Stops[0].Reason != "stopped from the cockpit" || !r.Stops[0].At.Equal(r.Last) {
		t.Errorf("Stops = %+v", r.Stops)
	}
	if want := (ToolErrors{Count: 1, Arguments: 1}); r.ToolErrors != want {
		t.Errorf("ToolErrors = %+v, want %+v", r.ToolErrors, want)
	}
	if r.Unreadable != 1 || r.Problem != "the last line is not complete" {
		t.Errorf("Unreadable = %d, %q", r.Unreadable, r.Problem)
	}

	var b strings.Builder
	if err := r.Markdown(&b, "sample", true); err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		"## sample\n",
		"| Requests | 9 | 593k | 100.0% |",
		"| Full cache rewrites | 3 | 350k | 59.0% |",
		"| · thinking window | 1 | 120k | 20.2% |",
		"| Requests that only wait | 1 | 12k | 1.9% |",
		"| · before compactions | 1 | 96k | 16.2% |",
		"whatever came first: new run 1, thinking window 1, prune 1.",
		"| Messages | 4 | 1 after tool calls, 2 opened a run, 2 from the canvas (1 replies) |",
		"| Heartbeats | 2 | progress report 1, after compaction 1 |",
		"| Unreadable lines | 1 | the last line is not complete |",
		"- 2026-10-01 12:05:12 UTC: stopped from the cockpit",
		"| 2026-10-01 12:04:12 | 62000 | 1000 | 61000 | 120.0 | yes | new run |",
		"| 2026-10-01 12:04:12 | 62000 | 1000 | 61000 | 120.0 | yes | pause over TTL |",
	} {
		if !strings.Contains(b.String(), want) {
			t.Errorf("the markdown has no %q:\n%s", want, b.String())
		}
	}
}

// The session store records the status of a call whose operations finished
// with each one it saved as it is named by its ID: such a status reads as
// one that holds the operation whole.
func TestReadTakesStatusesThatNameTheirOperations(t *testing.T) {
	log := func(operations any) []byte {
		l := newSampleLog(t, start)
		l.prompt(0, "Do the task", "")
		l.respond(time.Second, session.TurnRegular, usage(1000, 0, 1000, 10), nil, bash("call-1", "ls"))
		l.started("call-1", "op-1")
		l.ended(time.Minute, "op-1")
		l.sequence++
		l.record("item", map[string]any{
			"Item": sessionstore.Item{
				Sequence: l.sequence, RecordedAt: l.at, Kind: sessionstore.ItemToolCallStatus,
				Data: sessionstore.ToolCallStatus{
					TurnID: l.last, CallID: "call-1", Status: tool.CallStatus{WaitingFor: []operation.ID{"op-1"}},
				},
			},
			"Operations": operations,
		})
		return l.b.Bytes()
	}
	whole := read(t, log([]operation.Operation{{ID: "op-1", Type: "test", Version: 1, Status: operation.StatusCompleted}}), Options{})
	named := read(t, log([]operation.ID{"op-1"}), Options{})
	if named.Unreadable != 0 || !reflect.DeepEqual(named, whole) {
		t.Fatalf("a status naming its operation reads as %+v, one holding it as %+v", named, whole)
	}
}

func TestReadCountsFromSince(t *testing.T) {
	log, compacted := sample(t)
	r := read(t, log, Options{Since: compacted})
	// What came before is not counted, but still tells what came after:
	// the compaction before makes the first request a cold start after it.
	if r.Requests != 5 || !near(r.Cost, 448000) || !r.First.Equal(compacted) {
		t.Errorf("Requests, Cost, First = %d, %v, %v", r.Requests, r.Cost, r.First)
	}
	if r.Compactions != (Compactions{}) || r.Heartbeats != nil {
		t.Errorf("Compactions, Heartbeats = %+v, %v", r.Compactions, r.Heartbeats)
	}
	if got, want := parts(r.ColdStarts.Parts), map[string][2]float64{
		"after compaction": {1, 84000}, "pause over TTL": {1, 122000}, "before compactions": {0, 0},
	}; !reflect.DeepEqual(got, want) {
		t.Errorf("cold starts = %v, want %v", got, want)
	}
	if r.Rewrites.Count != 3 || r.Messages.Count != 3 || r.Waits.Requests != 0 {
		t.Errorf("Rewrites, Messages, Waits = %d, %d, %d", r.Rewrites.Count, r.Messages.Count, r.Waits.Requests)
	}
}

func TestReadTakesTheTTL(t *testing.T) {
	log, _ := sample(t)
	// A cache of three hours outlives the pause of two.
	r := read(t, log, Options{TTL: 3 * time.Hour})
	if got := parts(r.ColdStarts.Parts); got["after compaction"][0] != 2 || got["pause over TTL"][0] != 0 || !near(r.Cost, 593000) {
		t.Errorf("TTL 3h: cold starts %v, cost %v", got, r.Cost)
	}
	// A cache of five minutes costs 1.25 a token written.
	r = read(t, log, Options{TTL: 5 * time.Minute})
	if got := parts(r.ColdStarts.Parts); got["pause over TTL"][0] != 1 || !near(r.Cost, 593000-271000*0.75) {
		t.Errorf("TTL 5m: cold starts %v, cost %v", got, r.Cost)
	}
}

func TestReadTakesTheWaits(t *testing.T) {
	log, _ := sample(t)
	r := read(t, log, Options{Wait: regexp.MustCompile(`^(sleep|make)\b`)})
	if r.Waits.Requests != 2 || r.Waits.Calls != 2 || !near(r.Waits.Cost, 11500+84800) {
		t.Errorf("Waits = %+v", r.Waits)
	}
}

// TestReadReadsTheStoresLogs reads the logs the session store's tests keep,
// old ones too. The only line it cannot read is a stop of a mode the store
// no longer knows; it reads on past it.
func TestReadReadsTheStoresLogs(t *testing.T) {
	paths, err := filepath.Glob("../../../harness/sessionstore/localfile/testdata/*.session.jsonl")
	if err != nil || len(paths) == 0 {
		t.Fatalf("no logs: %v", err)
	}
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		r := read(t, data, Options{})
		name := filepath.Base(path)
		unreadable := 0
		if name == "soft-stop-session.session.jsonl" {
			unreadable = 1
			if !strings.Contains(r.Problem, `unsupported control mode "soft"`) {
				t.Errorf("%s: problem %q", name, r.Problem)
			}
		}
		if r.Unreadable != unreadable || r.Session == "" {
			t.Errorf("%s: unreadable %d (%s), session %q", name, r.Unreadable, r.Problem, r.Session)
		}
		if name == "golden-session.session.jsonl" && (r.Requests != 1 || r.Messages.Count != 1) {
			t.Errorf("%s: requests %d, messages %d", name, r.Requests, r.Messages.Count)
		}
	}
}
