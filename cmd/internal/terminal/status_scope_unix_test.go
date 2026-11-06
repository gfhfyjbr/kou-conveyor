//go:build unix

package terminal

import (
	"testing"
	"time"
)

func TestProgramStatusProbeAndObservePreserveClosedScopeBeforeBufferedInput(t *testing.T) {
	m := cleanManager(t)
	defer m.Close()
	s, err := m.Start(Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := s.Attach()
	defer c.Detach()
	if err := s.Write([]byte("printf 'observe-%s\\n' scope\n")); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "observe-scope", 5*time.Second)
	s.mu.Lock()
	group := foreground(s.master)
	if group != s.pid {
		s.mu.Unlock()
		t.Fatalf("expected the shell foreground group %d, got %d", s.pid, group)
	}
	// Model an old reporter whose foreground group was already observed;
	// its final bytes haven't reached the scanner when the probe runs.
	s.programPID = group + 100000
	s.scan.program = ProgramState{Reported: true, Records: []ProgramStatus{{State: "blocked", App: "claude-code", Kind: "permission"}}}
	s.mu.Unlock()
	if state := s.ProgramState(); !state.Closed || state.Reported {
		t.Fatalf("out-of-band foreground probe lost closure: %#v", state)
	}
	initial := make(chan ProgramState, 1)
	cancel := s.Observe(func(ev Event) {
		for _, update := range ev.Programs {
			select {
			case initial <- update.State:
			default:
			}
		}
	})
	defer cancel()
	if state := <-initial; !state.Closed || state.Reported {
		t.Fatalf("Observe initial snapshot reopened the old scope: %#v", state)
	}
	s.mu.Lock()
	// The probe/Observe consumed the transition, so every later read has
	// ended=false; the report is also split at arbitrary stream boundaries.
	s.scan.feedOutput([]byte("\x1b]7501;sta"), false)
	s.scan.feedOutput([]byte("te=blocked:app=claude-code"), false)
	s.scan.feedOutput([]byte("\x07"), false)
	state := s.scan.program.clone()
	s.mu.Unlock()
	if !state.Closed || state.Reported || state.Record("") != nil {
		t.Fatalf("split buffered input reopened the probed/observed scope: %#v", state)
	}
}
