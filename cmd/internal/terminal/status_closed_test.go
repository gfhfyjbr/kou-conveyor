package terminal

import "testing"

func TestProgramStatusClosedScopeSurvivesSplitBufferedReports(t *testing.T) {
	for _, state := range []string{"working", "blocked", "done", "error"} {
		t.Run(state, func(t *testing.T) {
			s := newScanner()
			s.marking = true
			s.feed([]byte("\x1b]7501;state=working:app=claude-code\x07"))
			s.endProgram() // an out-of-band ProgramState/Observe probe saw exit first
			s.takePrograms()
			s.feedOutput([]byte("\x1b]7501;state="), false)
			s.feedOutput([]byte(state+":app=claude-code"), false)
			s.feedOutput([]byte("\x07"), false)
			if s.program.Reported || !s.program.Closed {
				t.Fatal("a split final report reopened the old program's scope")
			}
			r := s.program.Record("")
			if state == "done" || state == "error" {
				if r == nil || r.State != state {
					t.Fatal("the completed final record was not retained")
				}
			} else if r != nil {
				t.Fatal("a buffered live record survived the closed program")
			}
			for _, ev := range s.takePrograms() {
				if !ev.State.Closed {
					t.Fatal("an observer snapshot forgot the persistent closed scope")
				}
			}
			// Reads and new observer snapshots cannot reopen it, including
			// while a report is still partial. A verified new C can.
			if !s.program.rootState().Closed || !s.program.clone().Closed {
				t.Fatal("a snapshot dropped the lifecycle boundary")
			}
			s.feedOutput([]byte("\x1b]133;C\x07\x1b]7501;state=working:app=codex\x07"), false)
			if !s.program.Reported || s.program.Closed || s.program.Record("").App != "codex" {
				t.Fatal("a verified new command could not start a new reporting scope")
			}
		})
	}
}

func TestProgramStatusNewForegroundClearsEmptyClosedScope(t *testing.T) {
	s := newScanner()
	s.marking = true
	s.feed([]byte("\x1b]7501;state=blocked:kind=permission\x07"))
	s.endProgram()
	if len(s.program.Records) != 0 || !s.program.Closed {
		t.Fatal("expected an empty closed scope")
	}
	s.resetProgram() // an observed new foreground, even without shell integration
	if s.program.Closed || s.program.Reported {
		t.Fatal("the old empty scope blocked a verified new foreground")
	}
	events := s.takePrograms()
	if events[len(events)-1].State.Closed {
		t.Fatal("observers were not told the old closed scope ended")
	}
}
