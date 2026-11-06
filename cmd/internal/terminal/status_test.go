package terminal

import (
	"encoding/base64"
	"fmt"
	"reflect"
	"strings"
	"testing"
)

func TestProgramStatusCapabilityQuery(t *testing.T) {
	for _, end := range []string{"\x07", "\x1b\\"} {
		query := "\x1b]7501;?" + end
		want := "\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth" + end
		for split := 0; split <= len(query); split++ {
			s := newScanner()
			s.marking = true
			s.feed([]byte(query[:split]))
			s.feed([]byte(query[split:]))
			if got := string(s.takeResponses()); got != want {
				t.Fatalf("terminator %q, split %d: reply %q, want %q", end, split, got, want)
			}
			if s.program.Reported || len(s.takePrograms()) != 0 || len(s.takeResponses()) != 0 {
				t.Fatal("a query changed status, or a drained reply was kept")
			}
		}
	}
	s := newScanner()
	s.feed([]byte("\x1b]7501;?\x07")) // historical replay
	if len(s.takeResponses()) != 0 {
		t.Fatal("replaying output answered an old query")
	}
	s.marking = true
	s.feed([]byte("\x1b]7501;?:states=idle,working\x07\x1b]7501;?x\x07"))
	if len(s.takeResponses()) != 0 || s.program.Reported {
		t.Fatal("a capability reply or malformed query was treated as a query/report")
	}
}

func TestProgramStatusParser(t *testing.T) {
	for _, state := range []string{"idle", "working", "done", "blocked", "error", "clear"} {
		r, ok := parseProgramStatus("state=" + state + ":app=claude-code")
		if !ok || r.State != state || r.App != "claude-code" {
			t.Fatalf("state %s: %#v, %v", state, r, ok)
		}
	}
	r, ok := parseProgramStatus(" state = blocked : kind=permission:id=agent/tool:app=claude-code:progress=0:title=VG9vbA:msg=QXBwbHk/")
	if !ok || r.Kind != "permission" || r.ID != "agent/tool" || r.Title != "Tool" || r.Message != "Apply?" || r.Progress == nil || *r.Progress != 0 {
		t.Fatalf("real report: %#v, %v", r, ok)
	}
	for _, kind := range []string{"permission", "question", "auth"} {
		r, ok := parseProgramStatus("state=blocked:kind=" + kind)
		if !ok || r.Kind != kind {
			t.Fatalf("blocked kind %s: %#v, %v", kind, r, ok)
		}
	}
	for _, body := range []string{
		"state=idle:kind=permission:progress=50",
		"state=done:kind=question:progress=100",
		"state=working:kind=auth:progress=-1",
		"state=working:progress=101",
		"state=working:progress=+1",
		"state=working:progress=99999999999999999999999",
		"state=blocked:kind=future:app=bad/name",
	} {
		r, ok := parseProgramStatus(body)
		if !ok || r.Progress != nil || r.Kind != "" || r.App != "" {
			t.Errorf("optional invalid/inapplicable fields in %q: %#v, %v", body, r, ok)
		}
	}
	r, ok = parseProgramStatus("state=idle:bogus:future=thing:state=working:app=claude-code:app=codex:progress=100")
	if !ok || r.State != "working" || r.App != "codex" || r.Progress == nil || *r.Progress != 100 {
		t.Fatalf("last key wins, unknown/malformed pairs ignored: %#v, %v", r, ok)
	}
	for _, body := range []string{
		"", "app=claude-code", "state=future", "state=WORKING", "state=working:state=future",
		"state=clear:id=", "state=clear:id=/tool", "state=clear:id=tool/", "state=clear:id=tool//child", "state=clear:id=bad@id",
		"state=working:id=" + strings.Repeat("x", 33),
		"state=working:id=" + strings.Repeat("a/", 8) + "a",
		"state=working:app=" + strings.Repeat("x", 33),
		"state=working:abcdefghijklmnopq=x", "state=working:msg=a",
		"state=blocked:msg=YQo=",   // newline
		"state=blocked:msg=AA==",   // NUL
		"state=blocked:msg=fw==",   // DEL
		"state=blocked:title=woU=", // C1 control
		"state=working:msg=/w==",   // invalid UTF-8
		"state=working:title=" + base64.StdEncoding.EncodeToString([]byte(strings.Repeat("x", 193))),
		"state=working:msg=" + base64.StdEncoding.EncodeToString([]byte(strings.Repeat("x", 2049))),
		"state=working:msg=" + strings.Repeat("A", 2733) + ":msg=ok",
		"state=working:" + strings.Repeat("x", 4096),
	} {
		if r, ok := parseProgramStatus(body); ok {
			t.Errorf("invalid report accepted: %q: %#v", body, r)
		}
	}
}

func TestProgramStatusRecordsAndLifecycle(t *testing.T) {
	s := newScanner()
	s.marking = true
	feed := func(body string) { t.Helper(); s.feed([]byte("\x1b]7501;" + body + "\x07")) }
	feed("state=idle:app=claude-code")
	first := s.takePrograms()[0]
	feed("state=working:id=worker:app=codex")
	feed("state=blocked:id=worker/tool:kind=permission:msg=QXBwbHk/")
	feed("state=done:id=worker2")
	if r := s.program.Record("worker/tool"); r == nil || r.App != "codex" || r.Kind != "permission" {
		t.Fatalf("child app inheritance: %#v", r)
	}
	if root := s.program.Record(""); root == nil || root.State != "idle" || root.App != "claude-code" {
		t.Fatalf("child report replaced root: %#v", root)
	}
	feed("state=clear:id=worker")
	if s.program.Record("worker") != nil || s.program.Record("worker/tool") != nil || s.program.Record("worker2") == nil {
		t.Fatalf("clear did not respect id path boundaries: %#v", s.program)
	}
	if root := first.State.Record(""); root == nil || root.App != "claude-code" || len(first.State.Records) != 1 {
		t.Fatalf("later reports mutated a queued snapshot: %#v", first)
	}
	feed("state=working")
	if root := s.program.Record(""); root.App != "" || root.Message != "" {
		t.Fatalf("report merged instead of replacing record: %#v", root)
	}
	before := s.program.clone()
	s.feed([]byte("\x1b[?1049h\x1b[?1049l\x1b[!p"))
	if !reflect.DeepEqual(s.program, before) {
		t.Fatal("screen switch or DECSTR removed program records")
	}
	s.feed([]byte("\x1bc"))
	if !s.program.Reported || len(s.program.Records) != 0 {
		t.Fatalf("RIS should clear records without enabling heuristics for this program: %#v", s.program)
	}
	feed("state=blocked:kind=auth")
	feed("state=done:id=finished")
	feed("state=error:id=failed")
	s.feed([]byte("\x1b]133;A\x07"))
	if s.program.Reported || s.program.Record("") != nil || s.program.Record("finished") == nil || s.program.Record("failed") == nil {
		t.Fatalf("prompt lifetime: %#v", s.program)
	}
	s.feed([]byte("\x1b]133;C\x07"))
	if s.program.Reported || len(s.program.Records) != 0 {
		t.Fatalf("new command inherited the previous program: %#v", s.program)
	}
	feed("state=working:app=claude-code")
	feed("state=clear")
	if !s.program.Reported || len(s.program.Records) != 0 {
		t.Fatalf("clear re-enabled heuristic status: %#v", s.program)
	}
	feed("state=working")
	s.feed([]byte("\x1b]133;D;0\x07"))
	if s.program.Reported || len(s.program.Records) != 0 {
		t.Fatalf("command end left a live record: %#v", s.program)
	}
}

func TestProgramStatusSplitReportsAndBounds(t *testing.T) {
	sequence := "\x1b]7501;state=blocked:app=claude-code:kind=permission:msg=QXBwbHk/\x1b\\"
	for split := 0; split <= len(sequence); split++ {
		s := newScanner()
		s.marking = true
		s.feed([]byte(sequence[:split]))
		s.feed([]byte(sequence[split:]))
		events := s.takePrograms()
		if len(events) != 1 || !events[0].Root || events[0].State.Record("").Message != "Apply?" {
			t.Fatalf("split %d: %#v", split, events)
		}
	}
	s := newScanner()
	s.marking = true
	s.feed([]byte("text\x1b]7501;state=working:app=claude-code\x07"))
	events := s.takePrograms()
	if len(events) != 1 || events[0].At != len("text\x1b]7501;state=working:app=claude-code\x07") {
		t.Fatalf("report offset: %#v", events)
	}
	s.feed([]byte("\x1b]7501;state=error:msg=" + strings.Repeat("A", 8192) + "\x07"))
	if len(s.takePrograms()) != 0 || s.program.Record("").State != "working" {
		t.Fatal("oversized report applied a truncated prefix")
	}
	var records ProgramState
	for i := range 256 {
		records.apply(ProgramStatus{State: "working", ID: fmt.Sprint(i)})
	}
	records.apply(ProgramStatus{State: "done", ID: "0"})
	records.apply(ProgramStatus{State: "working", ID: "256"})
	if len(records.Records) != 256 || records.Record("0") == nil || records.Record("1") != nil || records.Record("256") == nil {
		t.Fatalf("bounded least-recently-updated eviction: %#v", records)
	}
}

func TestProgramStatusHandoverDoesNotReplayReportsOrQueries(t *testing.T) {
	program := ProgramState{Reported: true, Records: []ProgramStatus{{State: "blocked", App: "claude-code", Kind: "permission"}}}
	s := &Session{}
	s.init(&handedOver{Program: program, ProgramPID: 1234, Output: []byte("\x1b]7501;?\x07\x1b]7501;state=working:app=old\x07")})
	if !reflect.DeepEqual(s.scan.program, program) || s.programPID != 1234 || len(s.scan.takeResponses()) != 0 || len(s.scan.takePrograms()) != 0 {
		t.Fatalf("handover lost current facts or replayed historical output: %#v", s.scan)
	}
	fresh := &Session{}
	fresh.init(nil)
	if fresh.scan.program.Reported || len(fresh.scan.program.Records) != 0 {
		t.Fatal("a fresh session inherited reporting state")
	}
}
