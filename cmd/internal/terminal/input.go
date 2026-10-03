package terminal

import (
	"bytes"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

// What else than a page uses a shell: a canvas types into it as tmux's
// send-keys does, follows what it prints and where its prompts and commands
// start and end (OSC 133), and reads the end of its output as text.

// Event is what an observer of a session hears: output as it comes, with
// the semantic prompt marks found in it, news of the title or the
// directory, and the shell's end.
type Event struct {
	Output []byte
	// Marks are the OSC 133 marks of Output, with where in it each ended.
	Marks []Mark
	Meta  *Meta
	Exit  *int
}

// Observe has fn hear what the session prints from now on, until the
// returned function is called. fn runs while the session's lock is held:
// it must neither block nor call the session back. A session that ended
// already is said to have, at once.
func (s *Session) Observe(fn func(Event)) (cancel func()) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.observed++
	n := s.observed
	if s.exited {
		code := s.code
		fn(Event{Exit: &code})
		return func() {}
	}
	s.observers[n] = fn
	return func() {
		s.mu.Lock()
		delete(s.observers, n)
		s.mu.Unlock()
	}
}

// BracketedPaste reports whether the program at the terminal asked for
// pastes to come bracketed (mode 2004).
func (s *Session) BracketedPaste() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.scan.modes[2004]
}

// ApplicationCursor reports whether the program at the terminal asked for
// the cursor keys' application form (DECCKM, mode 1).
func (s *Session) ApplicationCursor() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.scan.modes[1]
}

// Exited reports whether the shell ended, and how.
func (s *Session) Exited() (bool, int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.exited, s.code
}

// PasteOptions say how text is pasted.
type PasteOptions struct {
	// Bracketed pastes the text between ESC[200~ and ESC[201~ when the
	// program asked for that; otherwise it is typed as it is.
	Bracketed bool
	// Newline is what a line break of the text becomes: "\r", as a
	// terminal sends Enter, when empty.
	Newline string
	// Submit is typed after the text, SubmitDelay later: "\r" presses
	// Enter. Empty types nothing.
	Submit      string
	SubmitDelay time.Duration
}

// maxPaste bounds the text pasted at once.
const maxPaste = 1 << 20

// Paste types text into the terminal as a paste, then presses Submit. No
// one else types meanwhile: the text and what submits it go in together,
// even with the delay between them, so keys a person types at the same
// time come before or after, never inside.
func (s *Session) Paste(text string, options PasteOptions) error {
	if len(text) > maxPaste {
		return fmt.Errorf("the text is over %d KiB", maxPaste>>10)
	}
	if exited, _ := s.Exited(); exited {
		return errors.New("the shell ended")
	}
	newline := options.Newline
	if newline == "" {
		newline = "\r"
	}
	clean := SanitizePaste(text)
	clean = strings.ReplaceAll(clean, "\n", newline)
	var payload []byte
	if clean != "" {
		if options.Bracketed && s.BracketedPaste() {
			payload = append(payload, "\x1b[200~"...)
			payload = append(payload, clean...)
			payload = append(payload, "\x1b[201~"...)
		} else {
			payload = append(payload, clean...)
		}
	}
	s.writing.Lock()
	defer s.writing.Unlock()
	if err := s.writeAll(payload); err != nil {
		return err
	}
	if options.Submit == "" {
		return nil
	}
	if options.SubmitDelay > 0 && len(payload) > 0 {
		// A program that takes a paste in pieces reads all of it before the
		// key that submits it.
		time.Sleep(min(options.SubmitDelay, 5*time.Second))
	}
	return s.writeAll([]byte(options.Submit))
}

// writeAll writes p to the terminal; the caller holds s.writing.
func (s *Session) writeAll(p []byte) error {
	if len(p) > 0 {
		s.lastInput.Store(time.Now().UnixNano())
	}
	for len(p) > 0 {
		n, err := s.master.Write(p)
		if err != nil {
			return err
		}
		p = p[n:]
	}
	return nil
}

// SanitizePaste takes out of text what a paste must not carry: escape
// sequences, which could end a bracketed paste early or drive the
// terminal, and control characters other than tabs and line breaks. Line
// breaks become "\n".
func SanitizePaste(text string) string {
	text = strings.ReplaceAll(text, "\r\n", "\n")
	text = strings.ReplaceAll(text, "\r", "\n")
	var b strings.Builder
	b.Grow(len(text))
	for _, r := range text {
		switch {
		case r == utf8.RuneError:
			continue
		case r == '\t' || r == '\n':
			b.WriteRune(r)
		case r < 0x20 || r == 0x7f:
			continue // C0 controls and DEL, ESC among them
		case r >= 0x80 && r <= 0x9f:
			continue // C1 controls
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Keys presses keys, named as tmux's send-keys names them: Enter, Tab,
// BTab, Escape, BSpace, Space, Up, Down, Left, Right, Home, End, PageUp,
// PageDown, Insert, Delete, F1–F12, C-a … C-z (and C-@, C-[, C-\, C-],
// C-^, C-_), M-x for Meta (ESC, then the key). A word that names no key is
// typed as it is, without its control characters. They go in at once,
// with nothing typed between them.
func (s *Session) Keys(names ...string) error {
	if exited, _ := s.Exited(); exited {
		return errors.New("the shell ended")
	}
	application := s.ApplicationCursor()
	var payload []byte
	for _, name := range names {
		bytes, err := KeyBytes(name, application)
		if err != nil {
			return err
		}
		payload = append(payload, bytes...)
	}
	s.writing.Lock()
	defer s.writing.Unlock()
	return s.writeAll(payload)
}

var namedKeys = map[string]string{
	"enter": "\r", "return": "\r", "tab": "\t", "btab": "\x1b[Z", "escape": "\x1b", "esc": "\x1b",
	"bspace": "\x7f", "backspace": "\x7f", "space": " ",
	"pageup": "\x1b[5~", "ppage": "\x1b[5~", "pgup": "\x1b[5~", "pagedown": "\x1b[6~", "npage": "\x1b[6~", "pgdn": "\x1b[6~",
	"insert": "\x1b[2~", "ic": "\x1b[2~", "delete": "\x1b[3~", "dc": "\x1b[3~",
	"f1": "\x1bOP", "f2": "\x1bOQ", "f3": "\x1bOR", "f4": "\x1bOS", "f5": "\x1b[15~", "f6": "\x1b[17~",
	"f7": "\x1b[18~", "f8": "\x1b[19~", "f9": "\x1b[20~", "f10": "\x1b[21~", "f11": "\x1b[23~", "f12": "\x1b[24~",
}

// cursorKeys are the keys whose form follows DECCKM: ESC [ x normally,
// ESC O x in the application form.
var cursorKeys = map[string]byte{"up": 'A', "down": 'B', "right": 'C', "left": 'D', "home": 'H', "end": 'F'}

// KeyBytes is what pressing a key named as Keys names it sends; application
// is the cursor keys' application form (DECCKM).
func KeyBytes(name string, application bool) ([]byte, error) {
	if name == "" {
		return nil, nil
	}
	lower := strings.ToLower(name)
	if seq, ok := namedKeys[lower]; ok {
		return []byte(seq), nil
	}
	if final, ok := cursorKeys[lower]; ok {
		if application {
			return []byte{0x1b, 'O', final}, nil
		}
		return []byte{0x1b, '[', final}, nil
	}
	if rest, ok := strings.CutPrefix(name, "M-"); ok && rest != "" {
		key, err := KeyBytes(rest, application)
		if err != nil {
			return nil, err
		}
		return append([]byte{0x1b}, key...), nil
	}
	if rest, ok := strings.CutPrefix(name, "C-"); ok && utf8.RuneCountInString(rest) == 1 {
		r, _ := utf8.DecodeRuneInString(rest)
		switch {
		case r >= 'a' && r <= 'z':
			return []byte{byte(r - 'a' + 1)}, nil
		case r >= 'A' && r <= 'Z':
			return []byte{byte(r - 'A' + 1)}, nil
		case r == '@' || r == ' ' || r == '2':
			return []byte{0}, nil
		case r == '[':
			return []byte{0x1b}, nil
		case r == '\\':
			return []byte{0x1c}, nil
		case r == ']':
			return []byte{0x1d}, nil
		case r == '^' || r == '6':
			return []byte{0x1e}, nil
		case r == '_' || r == '-':
			return []byte{0x1f}, nil
		case r == '?':
			return []byte{0x7f}, nil
		}
		return nil, fmt.Errorf("no key %q", name)
	}
	// A word that names no key is typed as it is.
	return []byte(strings.ReplaceAll(SanitizePaste(name), "\n", "\r")), nil
}

// Tail is the last lines the shell printed, as text: without the escape
// sequences, and with what carriage returns and backspaces wrote over
// written over.
func (s *Session) Tail(lines int) string {
	s.mu.Lock()
	kept := s.ring.bytes()
	s.mu.Unlock()
	// The end is what matters: a screen's worth of lines is far less than
	// the ring.
	if limit := 512 << 10; len(kept) > limit {
		kept = kept[len(kept)-limit:]
		if at := bytes.IndexByte(kept, '\n'); at >= 0 && at < 4096 {
			kept = kept[at+1:]
		}
	}
	return LastLines(Render(kept), lines)
}

// Render turns terminal output into the text it shows: carriage returns go
// back to the start of the line, backspaces one place back, and the escape
// sequences that move the cursor along a line or erase some of it do so —
// as a shell's line editor redraws what is typed; the other sequences, of
// colours or of the screen as a whole, leave nothing.
func Render(output []byte) string {
	text := string(output)
	var out strings.Builder
	out.Grow(len(text))
	line := make([]rune, 0, 128)
	cursor := 0
	flush := func() {
		out.WriteString(strings.TrimRight(string(line), " "))
		line, cursor = line[:0], 0
	}
	// pad makes the line reach the cursor, for a write there.
	pad := func() {
		for len(line) < cursor {
			line = append(line, ' ')
		}
	}
	for i := 0; i < len(text); {
		r, size := utf8.DecodeRuneInString(text[i:])
		if r == 0x1b {
			n, final, params := escapeSequence(text[i:])
			i += n
			if final == 0 || params != "" && params[0] >= '<' && params[0] <= '?' {
				continue // not a CSI sequence, or a private one
			}
			count := 1
			if first, _, _ := strings.Cut(params, ";"); first != "" {
				if v, err := strconv.Atoi(first); err == nil {
					count = v
				}
			}
			moves := min(max(count, 1), maxRenderedLine)
			switch final {
			case 'D': // back
				cursor = max(0, cursor-moves)
			case 'C': // forward
				cursor = min(cursor+moves, maxRenderedLine)
			case 'G', '`': // to the column
				cursor = moves - 1
			case 'K': // erase in the line: to its end, to its start, all of it
				switch {
				case params == "" || count == 0:
					line = line[:min(cursor, len(line))]
				case count == 1:
					for j := 0; j <= cursor && j < len(line); j++ {
						line[j] = ' '
					}
				case count == 2:
					line = line[:0]
				}
			case 'P': // delete characters
				if cursor < len(line) {
					line = append(line[:cursor], line[min(cursor+moves, len(line)):]...)
				}
			case '@': // insert blanks
				if cursor < len(line) {
					line = append(line[:cursor], append([]rune(strings.Repeat(" ", min(moves, len(line)-cursor))), line[cursor:]...)...)
				}
			case 'X': // erase characters
				for j := cursor; j < cursor+moves && j < len(line); j++ {
					line[j] = ' '
				}
			}
			continue
		}
		i += size
		switch r {
		case '\n':
			flush()
			out.WriteByte('\n')
		case '\r':
			cursor = 0
		case '\b':
			cursor = max(0, cursor-1)
		case '\t':
			cursor = min((cursor/8+1)*8, maxRenderedLine)
			pad()
		default:
			if r < 0x20 || r == 0x7f || r >= 0x80 && r <= 0x9f {
				continue
			}
			pad()
			if cursor < len(line) {
				line[cursor] = r
			} else {
				line = append(line, r)
			}
			cursor++
		}
	}
	flush()
	return out.String()
}

// maxRenderedLine bounds how far right Render moves the cursor.
const maxRenderedLine = 4096

// escapeSequence measures the escape sequence s starts with (s[0] is ESC);
// for a CSI sequence it gives its final byte and its parameters too. A
// sequence cut short takes the rest of s.
func escapeSequence(s string) (n int, final byte, params string) {
	if len(s) < 2 {
		return len(s), 0, ""
	}
	switch s[1] {
	case '[':
		for i := 2; i < len(s); i++ {
			if c := s[i]; c >= 0x40 && c <= 0x7e {
				return i + 1, c, s[2:i]
			}
		}
		return len(s), 0, ""
	case ']', 'P', '_', '^', 'X':
		// A string, which BEL (after OSC) or ST ends.
		for i := 2; i < len(s); i++ {
			if s[i] == 0x07 && s[1] == ']' {
				return i + 1, 0, ""
			}
			if s[i] == 0x1b && i+1 < len(s) && s[i+1] == '\\' {
				return i + 2, 0, ""
			}
		}
		return len(s), 0, ""
	}
	// ESC, intermediate bytes, a final byte: ESC 7, ESC ( B.
	i := 1
	for i < len(s) && s[i] >= 0x20 && s[i] <= 0x2f {
		i++
	}
	return min(i+1, len(s)), 0, ""
}

// LastLines is the last n lines of text, without the empty lines at its
// end; n ≤ 0 is all of them.
func LastLines(text string, n int) string {
	text = strings.TrimRight(text, "\n ")
	if n <= 0 {
		return text
	}
	end := len(text)
	for i := 0; i < n; i++ {
		at := strings.LastIndexByte(text[:end], '\n')
		if at < 0 {
			return text
		}
		end = at
	}
	return text[end+1:]
}
