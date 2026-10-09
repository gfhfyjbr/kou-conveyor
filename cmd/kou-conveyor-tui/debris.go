package main

import (
	"regexp"
	"time"

	tea "github.com/charmbracelet/bubbletea"
)

// Bubble Tea v1 reads the terminal in chunks and takes a short read for the
// end of an event (readAnsiInputs in its key.go). When mouse reports pour
// in, as a fast scroll or a quick move sends them, one can be cut in two:
// its first half then arrives as an esc key, or as alt+[ and "<64;4", and
// the rest as typed keys, "[<64;40;10M" or "0;10M". Those would land in the
// composer, and the esc would arm esc esc, which stops a run. So where the
// terminal reports the mouse, a lone esc or alt+[ waits a moment, as the
// first report of a scroll can be cut too: with what follows it is dropped
// when that reads as the rest of a report, and passed on otherwise.

const (
	// debrisHold is how long a key that may start a cut report waits for
	// the rest, which is a read away: far less, unless the system stalls.
	// While the mouse is quiet, a report is less likely and debrisQuiet does.
	debrisHold  = 100 * time.Millisecond
	debrisQuiet = 20 * time.Millisecond
	// debrisWindow is how long after a mouse report the mouse counts as busy.
	debrisWindow = 250 * time.Millisecond
)

var (
	// After esc: the rest of a mouse report, "[<64;40;10M", or of a focus
	// report, "[I".
	restAfterEsc = regexp.MustCompile(`^\[(<[0-9;]*[Mm]?|[IO])?$`)
	// After alt+[: "<64;40;10M".
	restAfterBracket = regexp.MustCompile(`^<[0-9;]*[Mm]?$`)
	// More of a report whose start went by: "0;10M", "M".
	reportTail = regexp.MustCompile(`^[0-9;]*[Mm]?$`)
	// A report's tail on its own: "4;40;10M", whenever it comes. Typing
	// sends a key at a time, and pastes come as pastes.
	orphanTail = regexp.MustCompile(`^\[?<?[0-9]*;[0-9;]*[Mm]$`)
)

type releaseKeyMsg struct{ gen int }

type debris struct {
	mouse   time.Time   // the last mouse report
	held    *tea.KeyMsg // a key that may start a cut report
	gen     int
	pending bool // the start of a report went by; more of it may follow
}

// filterKey passes keys on, less the pieces of cut mouse reports.
func (m *uiModel) filterKey(msg tea.KeyMsg) tea.Cmd {
	d := &m.debris
	text := string(msg.Runes)
	typed := msg.Type == tea.KeyRunes && !msg.Paste && !msg.Alt
	if d.held != nil {
		held := *d.held
		d.held = nil
		rest := restAfterEsc
		if held.Type == tea.KeyRunes {
			rest = restAfterBracket
		}
		if typed && rest.MatchString(text) {
			d.pending = !reportEnds(text)
			return nil
		}
		// A key of its own: the held one goes first.
		first := m.handleKey(held)
		return tea.Batch(first, m.filterKey(msg))
	}
	if d.pending {
		d.pending = false
		if typed && reportTail.MatchString(text) {
			d.pending = !reportEnds(text)
			return nil
		}
	}
	if typed && orphanTail.MatchString(text) {
		return nil
	}
	// Compact mode leaves the mouse to the terminal: no reports come.
	if !m.compact && !msg.Paste &&
		(msg.Type == tea.KeyEscape && !msg.Alt || msg.Type == tea.KeyRunes && msg.Alt && text == "[") {
		hold := debrisQuiet
		if time.Since(d.mouse) < debrisWindow {
			hold = debrisHold
		}
		d.held = &msg
		d.gen++
		gen := d.gen
		return tea.Tick(hold, func(time.Time) tea.Msg { return releaseKeyMsg{gen} })
	}
	return m.handleKey(msg)
}

// release passes on a held key that nothing followed.
func (m *uiModel) release(msg releaseKeyMsg) tea.Cmd {
	d := &m.debris
	if msg.gen != d.gen || d.held == nil {
		return nil
	}
	held := *d.held
	d.held = nil
	if held.Type == tea.KeyRunes {
		// alt+[ types nothing: what came alone was a report's start.
		return nil
	}
	return m.handleKey(held)
}

// reportEnds reports whether a report's rest includes its final byte.
func reportEnds(text string) bool {
	switch text[len(text)-1] {
	case 'M', 'm', 'I', 'O':
		return true
	}
	return false
}
