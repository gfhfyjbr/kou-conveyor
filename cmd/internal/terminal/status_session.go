package terminal

// ProgramState returns the current records and checks their foreground
// program's lifetime, including an exit that produced no more output.
func (s *Session) ProgramState() ProgramState {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.syncProgramLocked()
	s.notifyProgramsLocked()
	return s.scan.program.clone()
}

// syncProgramLocked catches program changes without shell integration.
// Changes stay queued until the caller can deliver them with its output.
// A true result identifies an observed foreground-program return to shell.
func (s *Session) syncProgramLocked() bool {
	if s.exited {
		return false
	}
	group := foreground(s.master)
	if group <= 0 {
		return false
	}
	ended := s.programPID != 0 && s.programPID != group && group == s.pid
	if s.programPID != 0 && s.programPID != group {
		s.scan.at = 0
		if ended {
			s.scan.endProgram()
		} else {
			s.scan.resetProgram()
		}
	}
	s.programPID = group
	return ended
}

func (s *Session) notifyProgramsLocked() {
	if events := s.scan.takePrograms(); len(events) > 0 {
		for _, observe := range s.observers {
			observe(Event{Programs: events})
		}
	}
}
