package terminal

// ProgramState returns the current records and checks their foreground
// program's lifetime, including an exit that produced no more output.
func (s *Session) ProgramState() ProgramState {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.syncProgramLocked()
	return s.scan.program.clone()
}

// syncProgramLocked catches program changes without shell integration.
// It must run before new output is parsed, so the new program's first
// report cannot be cleared by a later foreground probe.
func (s *Session) syncProgramLocked() {
	if s.exited {
		return
	}
	group := foreground(s.master)
	if group <= 0 {
		return
	}
	if s.programPID != 0 && s.programPID != group {
		s.scan.at = 0
		if group == s.pid {
			s.scan.endProgram()
		} else {
			s.scan.resetProgram()
		}
		if events := s.scan.takePrograms(); len(events) > 0 {
			for _, observe := range s.observers {
				observe(Event{Programs: events})
			}
		}
	}
	s.programPID = group
}
