package terminal

import (
	"fmt"
	"net/url"
	"slices"
	"strconv"
	"strings"
)

// scanner follows what a shell's output says of the terminal: the title a
// program sets (OSC 0 and 2), the working directory a shell reports (OSC 7),
// and the modes a page needs set again to draw the screen from the output
// kept — the alternate screen, the mouse, bracketed paste, the cursor keys.
// It takes the output in chunks as they come, a sequence split between
// two of them included.
type scanner struct {
	state   scanState
	payload []byte // the sequence collected so far
	title   string
	dir     string
	modes   map[int]bool // the tracked modes that are set
}

type scanState uint8

const (
	ground    scanState = iota
	escape              // after ESC
	osc                 // in an operating system command
	oscEscape           // after ESC in one: ST may follow
	csi                 // in a control sequence
)

// maxPayload bounds the sequence collected: a title or a path, not a
// picture sent inline.
const maxPayload = 4096

// trackedModes are the DEC private modes a page attaching needs, in the
// order they are set again.
var trackedModes = []int{
	1,    // application cursor keys
	25,   // the cursor shown (set by default)
	47,   // the alternate screen
	1047, // the alternate screen
	1049, // the alternate screen, with the cursor saved
	1000, // mouse: clicks
	1002, // mouse: drags
	1003, // mouse: all motion
	1004, // focus in and out
	1005, // mouse: UTF-8 coordinates
	1006, // mouse: SGR coordinates
	1015, // mouse: urxvt coordinates
	2004, // bracketed paste
}

func newScanner() *scanner {
	return &scanner{modes: map[int]bool{25: true}}
}

// clone copies the scanner, to go on from where it is without changing it.
func (s *scanner) clone() *scanner {
	c := *s
	c.payload = slices.Clone(s.payload)
	c.modes = make(map[int]bool, len(s.modes))
	for mode, set := range s.modes {
		c.modes[mode] = set
	}
	return &c
}

// feed reads output; changed says whether the title or the directory
// changed.
func (s *scanner) feed(p []byte) (changed bool) {
	for _, b := range p {
		switch s.state {
		case ground:
			if b == 0x1b {
				s.state = escape
			}
		case escape:
			switch b {
			case ']':
				s.state, s.payload = osc, s.payload[:0]
			case '[':
				s.state, s.payload = csi, s.payload[:0]
			case 0x1b:
				// another ESC starts again
			default:
				s.state = ground
			}
		case osc:
			switch b {
			case 0x07:
				changed = s.command() || changed
				s.state = ground
			case 0x1b:
				s.state = oscEscape
			case 0x18, 0x1a: // CAN, SUB cancel it
				s.state = ground
			default:
				if len(s.payload) < maxPayload {
					s.payload = append(s.payload, b)
				}
			}
		case oscEscape:
			if b == '\\' {
				changed = s.command() || changed
				s.state = ground
			} else if b == '[' {
				s.state, s.payload = csi, s.payload[:0]
			} else if b == ']' {
				s.state, s.payload = osc, s.payload[:0]
			} else {
				s.state = ground
			}
		case csi:
			switch {
			case b >= 0x40 && b <= 0x7e:
				s.control(b)
				s.state = ground
			case b == 0x1b:
				s.state = escape
			case b == 0x18 || b == 0x1a:
				s.state = ground
			default:
				if len(s.payload) < 64 {
					s.payload = append(s.payload, b)
				}
			}
		}
	}
	return changed
}

// command takes an operating system command: a title, or a directory.
func (s *scanner) command() bool {
	number, text, ok := strings.Cut(string(s.payload), ";")
	if !ok {
		return false
	}
	switch number {
	case "0", "2":
		if s.title != text {
			s.title = text
			return true
		}
	case "7":
		dir := dirFromURL(text)
		if dir != "" && s.dir != dir {
			s.dir = dir
			return true
		}
	}
	return false
}

// dirFromURL is the directory a file URL of OSC 7 names: file://host/path.
func dirFromURL(text string) string {
	u, err := url.Parse(text)
	if err != nil || u.Scheme != "file" || !strings.HasPrefix(u.Path, "/") {
		return ""
	}
	return u.Path
}

// control takes a control sequence: a DEC private mode set or reset.
func (s *scanner) control(final byte) {
	if final != 'h' && final != 'l' || len(s.payload) < 2 || s.payload[0] != '?' {
		return
	}
	for _, field := range strings.Split(string(s.payload[1:]), ";") {
		mode, err := strconv.Atoi(field)
		if err != nil || !slices.Contains(trackedModes, mode) {
			continue
		}
		set := final == 'h'
		s.modes[mode] = set
		// The three ways to the alternate screen are one screen.
		if mode == 47 || mode == 1047 || mode == 1049 {
			for _, other := range []int{47, 1047, 1049} {
				if other != mode {
					s.modes[other] = false
				}
			}
		}
	}
}

// prefix is what sets the terminal modes the scanner found, for output
// that follows to draw as it did.
func (s *scanner) prefix() []byte {
	var b strings.Builder
	for _, mode := range trackedModes {
		set, known := s.modes[mode]
		switch {
		case mode == 25 && known && !set:
			b.WriteString("\x1b[?25l")
		case mode != 25 && set:
			fmt.Fprintf(&b, "\x1b[?%dh", mode)
		}
	}
	return []byte(b.String())
}

// setModes lists the tracked modes that differ from a terminal's defaults.
func (s *scanner) setModes() []int {
	var out []int
	for _, mode := range trackedModes {
		if set, known := s.modes[mode]; known && set != (mode == 25) {
			out = append(out, mode)
		}
	}
	return out
}

// restoreModes sets the modes setModes listed.
func (s *scanner) restoreModes(modes []int) {
	for _, mode := range modes {
		s.modes[mode] = mode != 25
	}
}
