//go:build unix

package terminal

import (
	"encoding/json/v2"
	"errors"
	"os"
	"time"

	"golang.org/x/sys/unix"
)

// A server that takes up a new build of itself runs it in its own process
// (exec), and the shells go on in it: their pseudo-terminals stay open
// across the exec, and what the new build needs to take them up — the
// descriptors, the processes, the output kept — is written to a file whose
// path it is given.

// handedOver is a session as a server hands it over.
type handedOver struct {
	ID        string    `json:"id"`
	Workspace string    `json:"workspace"`
	Shell     string    `json:"shell"`
	Theme     bool      `json:"theme,omitzero"`
	Started   time.Time `json:"started"`
	PID       int       `json:"pid"`
	FD        int       `json:"fd"`
	Cols      int       `json:"cols"`
	Rows      int       `json:"rows"`
	Title     string    `json:"title,omitzero"`
	Dir       string    `json:"cwd,omitzero"`
	// Modes are those set where the output kept starts.
	Modes  []int  `json:"modes,omitzero"`
	Output []byte `json:"output,omitzero"`
	// ClosingAt is when the shell of a tab closed ends.
	ClosingAt time.Time `json:"closing_at,omitzero"`
}

// Handover readies the shells that run to live on in the program that
// replaces this one by exec, and returns the file that says how to take
// them up; "" when none runs. Resume undoes it if the exec fails.
func (m *Manager) Handover() (string, error) {
	m.mu.Lock()
	var sessions []*Session
	for _, s := range m.sessions {
		sessions = append(sessions, s)
	}
	m.mu.Unlock()
	var records []handedOver
	for _, s := range sessions {
		s.mu.Lock()
		if s.exited {
			s.mu.Unlock()
			continue
		}
		record := handedOver{
			ID: s.id, Workspace: s.workspace, Shell: s.shell, Theme: s.theme, Started: s.started, PID: s.pid,
			Cols: s.cols, Rows: s.rows, Title: s.scan.title, Dir: s.scan.dir,
			Modes: s.base.setModes(), Output: s.ring.bytes(), ClosingAt: s.closeAt,
		}
		s.mu.Unlock()
		fd := int(s.master.Fd())
		if _, err := unix.FcntlInt(uintptr(fd), unix.F_SETFD, 0); err != nil {
			continue // it closes with the exec; its shell is hung up on
		}
		record.FD = fd
		records = append(records, record)
	}
	if len(records) == 0 {
		return "", nil
	}
	data, err := json.Marshal(records)
	if err != nil {
		m.resume(records)
		return "", err
	}
	file, err := os.CreateTemp("", "kou-conveyor-terminals-*.json")
	if err != nil {
		m.resume(records)
		return "", err
	}
	if _, err := file.Write(data); err != nil {
		file.Close()
		os.Remove(file.Name())
		m.resume(records)
		return "", err
	}
	if err := file.Close(); err != nil {
		os.Remove(file.Name())
		m.resume(records)
		return "", err
	}
	return file.Name(), nil
}

// Resume undoes a handover whose exec failed: the descriptors close on the
// next exec again, and the file goes.
func (m *Manager) Resume(path string) {
	if path == "" {
		return
	}
	data, err := os.ReadFile(path)
	os.Remove(path)
	if err != nil {
		return
	}
	var records []handedOver
	if json.Unmarshal(data, &records) == nil {
		m.resume(records)
	}
}

func (m *Manager) resume(records []handedOver) {
	for _, record := range records {
		if record.FD > 0 {
			unix.CloseOnExec(record.FD)
		}
	}
}

// Adopt takes up the shells a server before this one handed over in the
// file at path, and removes the file. It returns how many it took up.
func (m *Manager) Adopt(path string) (int, error) {
	data, err := os.ReadFile(path)
	os.Remove(path)
	if err != nil {
		return 0, err
	}
	var records []handedOver
	if err := json.Unmarshal(data, &records); err != nil {
		return 0, err
	}
	adopted := 0
	var problems []error
	for i := range records {
		record := &records[i]
		if record.FD <= 2 || record.PID <= 0 {
			continue
		}
		if _, err := unix.FcntlInt(uintptr(record.FD), unix.F_GETFD, 0); err != nil {
			problems = append(problems, err)
			continue // the descriptor did not make it
		}
		unix.CloseOnExec(record.FD)
		master := os.NewFile(uintptr(record.FD), "/dev/ptmx")
		proc, err := os.FindProcess(record.PID)
		if err != nil || master == nil {
			if master != nil {
				master.Close()
			}
			problems = append(problems, err)
			continue
		}
		s := &Session{
			id: record.ID, workspace: record.Workspace, shell: record.Shell, theme: record.Theme,
			started: record.Started, pid: record.PID, master: master, proc: proc, manager: m,
			cols: max(record.Cols, 1), rows: max(record.Rows, 1),
		}
		s.init(record)
		if !m.add(s) {
			s.hangUp()
			continue
		}
		s.run()
		if !record.ClosingAt.IsZero() {
			_ = m.CloseAfter(s.id, max(time.Until(record.ClosingAt), time.Millisecond))
		}
		adopted++
	}
	return adopted, errors.Join(problems...)
}
