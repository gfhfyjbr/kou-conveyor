package terminal

import "testing"

func TestProgramStatusBufferedForegroundExitKeepsOnlyCompletedRecords(t *testing.T) {
	for _, state := range []string{"working", "blocked", "idle", "done", "error"} {
		t.Run(state, func(t *testing.T) {
			s := newScanner()
			s.marking = true
			s.feed([]byte("\x1b]7501;state=working:app=claude-code\x07"))
			s.takePrograms()
			// The PTY still has this program's final output buffered after
			// its foreground group was observed returning to the shell.
			s.endProgram()
			s.feedOutput([]byte("\x1b]7501;state="+state+":app=claude-code\x07"), true)
			if s.program.Reported {
				t.Fatal("buffered final report re-enabled reporting for the shell")
			}
			r := s.program.Record("")
			if state == "done" || state == "error" {
				if r == nil || r.State != state {
					t.Fatal("exit discarded a completed record")
				}
			} else if r != nil {
				t.Fatalf("exit resurrected a live record: %#v", r)
			}
			events := s.takePrograms()
			last := events[len(events)-1]
			if last.State.Reported || !last.Ended {
				t.Fatal("the buffered chunk was not closed with its lifetime event")
			}
		})
	}
}

func TestProgramStatusBufferedExitDoesNotEndNewerCommand(t *testing.T) {
	s := newScanner()
	s.marking = true
	s.feed([]byte("\x1b]7501;state=blocked:app=claude-code\x07"))
	s.endProgram()
	s.feedOutput([]byte("\x1b]7501;state=done:app=claude-code\x07\x1b]133;C\x07\x1b]7501;state=working:app=codex\x07"), true)
	if r := s.program.Record(""); !s.program.Reported || r == nil || r.State != "working" || r.App != "codex" {
		t.Fatalf("the old exit cleared the newer command's report: %#v", s.program)
	}
	// An old→new foreground switch isn't an observed return to shell.
	s.resetProgram()
	s.feedOutput([]byte("\x1b]7501;state=blocked:app=gemini\x07"), false)
	if r := s.program.Record(""); !s.program.Reported || r == nil || r.App != "gemini" {
		t.Fatal("a direct foreground switch cleared the new program")
	}
}

func TestProgramStatusRISRecoversFromUnterminatedOSC(t *testing.T) {
	for _, interrupted := range []string{"\x1b]7501;state=working", "\x1b]0;old title"} {
		s := newScanner()
		s.marking = true
		s.feed([]byte("\x1b]7501;state=blocked:kind=permission\x07"))
		s.feed([]byte(interrupted + "\x1bc"))
		if s.state != ground || !s.program.Reported || len(s.program.Records) != 0 {
			t.Fatalf("unterminated OSC prevented RIS: %#v", s)
		}
		s.feed([]byte("\x1b]7501;state=idle\x07"))
		if r := s.program.Record(""); r == nil || r.State != "idle" {
			t.Fatal("scanner did not recover after RIS")
		}
	}
}
