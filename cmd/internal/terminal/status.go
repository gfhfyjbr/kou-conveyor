package terminal

import (
	"encoding/base64"
	"slices"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// OSC 7501, revision 0.4: https://www.superlogical.com/rex/docs/build/program-status
const programStatusReply = "?:states=idle,working,done,blocked,error:kinds=permission,question,auth"

// ProgramStatus is one OSC 7501 record. Every report replaces its record,
// including fields the program left out. Text is decoded, single-line UTF-8.
type ProgramStatus struct {
	State    string `json:"state"`
	ID       string `json:"id,omitzero"`
	App      string `json:"app,omitzero"`
	Kind     string `json:"kind,omitzero"`
	Progress *int   `json:"progress,omitzero"`
	Title    string `json:"title,omitzero"`
	Message  string `json:"msg,omitzero"`
}

// ProgramState keeps records oldest-update first. Reported belongs to the
// current foreground program, not to the terminal's lifetime. Clear and RIS
// remove records without making that program a nonreporting program again.
type ProgramState struct {
	Reported bool            `json:"reported,omitzero"`
	Records  []ProgramStatus `json:"records,omitzero"`
}

func (s ProgramState) clone() ProgramState {
	s.Records = slices.Clone(s.Records)
	return s
}

// Record returns a record with app inherited from its closest ancestor.
// Child states never replace or imply the state of their parent.
func (s ProgramState) Record(id string) *ProgramStatus {
	var record *ProgramStatus
	for _, r := range s.Records {
		if r.ID == id {
			record = &r
			break
		}
	}
	if record == nil || record.App != "" {
		return record
	}
	for id != "" {
		at := strings.LastIndexByte(id, '/')
		if at < 0 {
			id = ""
		} else {
			id = id[:at]
		}
		for _, r := range s.Records {
			if r.ID == id && r.App != "" {
				record.App = r.App
				return record
			}
		}
	}
	return record
}

func (s *ProgramState) apply(r ProgramStatus) {
	s.Reported = true
	if r.State == "clear" {
		s.Records = slices.DeleteFunc(s.Records, func(old ProgramStatus) bool {
			return r.ID == "" || old.ID == r.ID || strings.HasPrefix(old.ID, r.ID+"/")
		})
		return
	}
	s.Records = slices.DeleteFunc(s.Records, func(old ProgramStatus) bool { return old.ID == r.ID })
	if len(s.Records) >= 256 {
		s.Records = slices.Delete(s.Records, 0, 1)
	}
	s.Records = append(s.Records, r)
}

// ended drops live records at a shell prompt or process exit. Completed
// records survive these events, but are no longer the next program's status.
func (s *ProgramState) ended() {
	s.Reported = false
	s.Records = slices.DeleteFunc(s.Records, func(r ProgramStatus) bool {
		return r.State != "done" && r.State != "error"
	})
}

// ProgramEvent follows the root record and reporting lifecycle at a byte
// offset in Event.Output. State is an immutable root-only snapshot; the
// complete record set is available through Session.ProgramState.
type ProgramEvent struct {
	At    int
	State ProgramState
	// Root distinguishes a root report (including an unchanged one) from
	// a child update or a lifecycle boundary.
	Root bool
}

func (s ProgramState) rootState() ProgramState {
	state := ProgramState{Reported: s.Reported}
	if root := s.Record(""); root != nil {
		state.Records = []ProgramStatus{*root}
	}
	return state
}

func (s *scanner) programEvent(root bool) {
	if s.marking {
		// Copy only what observers use, not all 256 worker records for
		// every update in a stream of reports.
		s.programs = append(s.programs, ProgramEvent{At: s.at, State: s.program.rootState(), Root: root})
	}
}

func (s *scanner) resetProgram() {
	if !s.program.Reported && len(s.program.Records) == 0 {
		return
	}
	s.program = ProgramState{}
	s.programEvent(false)
}

func (s *scanner) endProgram() {
	if !s.program.Reported {
		return
	}
	s.program.ended()
	s.programEvent(false)
}

func (s *scanner) takePrograms() []ProgramEvent {
	p := s.programs
	s.programs = nil
	return p
}

func (s *scanner) takeResponses() []byte {
	p := s.responses
	s.responses = nil
	return p
}

func parseProgramStatus(body string) (ProgramStatus, bool) {
	// Count the whole sequence, conservatively allowing the longer ST.
	if len(body) > 4096-len("\x1b]7501;\x1b\\") {
		return ProgramStatus{}, false
	}
	values := make(map[string]string)
	for pair := range strings.SplitSeq(body, ":") {
		key, value, ok := strings.Cut(pair, "=")
		if !ok {
			continue
		}
		key, value = strings.Trim(key, " \t\r\n\v\f"), strings.Trim(value, " \t\r\n\v\f")
		if len(key) > 16 {
			return ProgramStatus{}, false
		}
		if key == "id" && !statusID(value) {
			return ProgramStatus{}, false // never fall back from an invalid id to the root
		}
		if key == "" || strings.ContainsFunc(key, func(r rune) bool { return r < 'a' || r > 'z' }) || !statusValue(value) {
			continue
		}
		// Limits apply to every occurrence, even one replaced later.
		switch key {
		case "app":
			if len(value) > 32 {
				return ProgramStatus{}, false
			}
		case "title":
			if len(value) > 256 {
				return ProgramStatus{}, false
			}
		case "msg":
			if len(value) > 2732 {
				return ProgramStatus{}, false
			}
		}
		values[key] = value
	}
	r := ProgramStatus{State: values["state"], ID: values["id"]}
	switch r.State {
	case "idle", "working", "done", "blocked", "error", "clear":
	default:
		return ProgramStatus{}, false
	}
	if app := values["app"]; statusName(app) {
		r.App = app
	}
	if r.State == "blocked" {
		switch kind := values["kind"]; kind {
		case "permission", "question", "auth":
			r.Kind = kind
		}
	}
	if r.State == "working" || r.State == "blocked" {
		p := values["progress"]
		if p != "" && !strings.ContainsFunc(p, func(r rune) bool { return r < '0' || r > '9' }) {
			if n, err := strconv.Atoi(p); err == nil && n <= 100 {
				r.Progress = &n
			}
		}
	}
	var ok bool
	if r.Title, ok = statusText(values["title"], 192); !ok {
		return ProgramStatus{}, false
	}
	if r.Message, ok = statusText(values["msg"], 2048); !ok {
		return ProgramStatus{}, false
	}
	return r, true
}

func statusName(s string) bool {
	return s != "" && !strings.ContainsFunc(s, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || strings.ContainsRune("_.+-", r))
	})
}

func statusValue(s string) bool {
	return !strings.ContainsFunc(s, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || strings.ContainsRune("_.,+/=-", r))
	})
}

func statusID(s string) bool {
	if len(s) > 128 || strings.Count(s, "/") >= 8 {
		return false
	}
	for part := range strings.SplitSeq(s, "/") {
		if len(part) > 32 || !statusName(part) {
			return false
		}
	}
	return true
}

func statusText(s string, limit int) (string, bool) {
	encoding := base64.RawStdEncoding
	if strings.HasSuffix(s, "=") {
		encoding = base64.StdEncoding
	}
	p, err := encoding.DecodeString(s)
	if err != nil || len(p) > limit || !utf8.Valid(p) || strings.ContainsFunc(string(p), unicode.IsControl) {
		return "", false
	}
	return string(p), true
}
