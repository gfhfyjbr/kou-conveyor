package terminal

import "testing"

func TestProgramStatusClosedHandoverCannotReplayOldLiveStatus(t *testing.T) {
	program := ProgramState{Closed: true, Records: []ProgramStatus{{State: "done", App: "claude-code"}}}
	s := &Session{}
	s.init(&handedOver{Program: program, ProgramPID: 1234, Output: []byte("\x1b]7501;state=working:app=old\x07")})
	state := s.scan.program.rootState()
	if !state.Closed || state.Reported || state.Record("") == nil || state.Record("").State != "done" {
		t.Fatalf("handover/replay reopened the closed scope: %#v", state)
	}
}
