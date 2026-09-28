//go:build !unix

package terminal

import "errors"

// handedOver is a session as a server hands it over; there is no handing
// over here.
type handedOver struct {
	Title, Dir string
	Modes      []int
	Output     []byte
}

// Handover hands nothing over here: a server restarts by hand.
func (m *Manager) Handover() (string, error) { return "", nil }

// Resume has nothing to undo here.
func (m *Manager) Resume(string) {}

// Adopt takes nothing up here.
func (m *Manager) Adopt(string) (int, error) { return 0, errors.ErrUnsupported }
