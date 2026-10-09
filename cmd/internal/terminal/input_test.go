package terminal

import (
	"testing"
)

func TestRenderShowsWhatALineEditorDrew(t *testing.T) {
	for _, c := range []struct{ name, in, want string }{
		{"plain", "hello\nworld\n", "hello\nworld\n"},
		{"colours", "\x1b[1;31mred\x1b[0m and \x1b[38;5;12mblue\x1b[m", "red and blue"},
		{"a carriage return writes over", "12345\rab", "ab345"},
		{"backspaces", "abc\b\bX", "aXc"},
		{"tabs", "a\tb", "a       b"},
		// zsh draws a paste highlighted, then goes back and draws it again.
		{"back and again", "❯ \x1b[7m(exit 7)\x1b[27m\x1b[8D(exit 7)", "❯ (exit 7)"},
		{"forward", "ab\x1b[3Cc", "ab   c"},
		{"to a column", "abcdef\x1b[3GX", "abXdef"},
		{"erase to the end", "abcdef\x1b[3D\x1b[K", "abc"},
		{"erase to the end, said", "abcdef\x1b[3D\x1b[0K!", "abc!"},
		{"erase to the start", "abcdef\x1b[3D\x1b[1K", "    ef"},
		{"erase the line", "abcdef\x1b[2Kxy", "      xy"},
		{"delete characters", "abcdef\x1b[4D\x1b[2P", "abef"},
		{"insert blanks", "abcdef\x1b[4D\x1b[2@", "ab  cdef"},
		{"erase characters", "abcdef\x1b[4D\x1b[2X", "ab  ef"},
		{"titles and marks leave nothing", "\x1b]0;title\x07a\x1b]133;A\x1b\\b", "ab"},
		{"private modes", "\x1b[?2004ha\x1b[?25lb", "ab"},
		{"two-byte escapes", "\x1b7a\x1b8\x1b(Bb\x1b=", "ab"},
		{"a sequence cut short", "abc\x1b[3", "abc"},
		{"C1 and other controls", "a\x00b\x07c\u0085d", "abcd"},
		{"lines keep apart", "first\x1b[5D\nsecond", "first\nsecond"},
		{"far right is bounded", "a\x1b[99999999Cb", ""},
	} {
		got := Render([]byte(c.in))
		if c.name == "far right is bounded" {
			if len(got) > maxRenderedLine+8 || got[len(got)-1] != 'b' {
				t.Errorf("%s: %d bytes", c.name, len(got))
			}
			continue
		}
		if got != c.want {
			t.Errorf("%s: Render(%q) = %q, want %q", c.name, c.in, got, c.want)
		}
	}
}

func TestLastLines(t *testing.T) {
	text := "one\ntwo\nthree\n\n  \n"
	for n, want := range map[int]string{0: "one\ntwo\nthree", 1: "three", 2: "two\nthree", 9: "one\ntwo\nthree"} {
		if got := LastLines(text, n); got != want {
			t.Errorf("LastLines(%d) = %q, want %q", n, got, want)
		}
	}
}

func TestSanitizePasteKeepsTextOnly(t *testing.T) {
	in := "ls\r\n\x1b[201~rm -rf /\x1b[200~\tok\rend\x00\x07\u009b31m\x7f"
	if got, want := SanitizePaste(in), "ls\n[201~rm -rf /[200~\tok\nend31m"; got != want {
		t.Fatalf("SanitizePaste = %q, want %q", got, want)
	}
}

func TestKeyBytesNameKeysAsTmuxDoes(t *testing.T) {
	for _, c := range []struct {
		name        string
		application bool
		want        string
	}{
		{"Enter", false, "\r"},
		{"Tab", false, "\t"},
		{"BTab", false, "\x1b[Z"},
		{"Escape", false, "\x1b"},
		{"BSpace", false, "\x7f"},
		{"Up", false, "\x1b[A"},
		{"Up", true, "\x1bOA"},
		{"Left", true, "\x1bOD"},
		{"Home", false, "\x1b[H"},
		{"End", true, "\x1bOF"},
		{"PageDown", false, "\x1b[6~"},
		{"F1", false, "\x1bOP"},
		{"F12", false, "\x1b[24~"},
		{"C-c", false, "\x03"},
		{"C-C", false, "\x03"},
		{"C-[", false, "\x1b"},
		{"C-?", false, "\x7f"},
		{"M-x", false, "\x1bx"},
		{"M-Up", true, "\x1b\x1bOA"},
		{"ls\x1b[2J", false, "ls[2J"},
	} {
		got, err := KeyBytes(c.name, c.application)
		if err != nil || string(got) != c.want {
			t.Errorf("KeyBytes(%q, %v) = %q, %v; want %q", c.name, c.application, got, err, c.want)
		}
	}
	if _, err := KeyBytes("C-é", false); err == nil {
		t.Error("C-é is no key")
	}
}

func TestScannerMarksPromptsAndCommands(t *testing.T) {
	s := newScanner()
	s.marking = true
	out := []byte("\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;2\x1b\\")
	s.feed(out[:20])
	s.feed(out[20:])
	marks := s.takeMarks()
	kinds := ""
	for _, m := range marks {
		kinds += string(m.Kind)
	}
	if kinds != "ABCD" || marks[3].Code != 2 || marks[0].Code != -1 {
		t.Fatalf("marks %+v", marks)
	}
	// Where a mark ended is counted in the output of its own feed.
	if marks[3].At != len(out)-20 {
		t.Fatalf("D ended at %d, want %d", marks[3].At, len(out)-20)
	}
	if s.takeMarks() != nil {
		t.Fatal("marks taken stay")
	}
	quiet := newScanner()
	quiet.feed(out)
	if quiet.takeMarks() != nil {
		t.Fatal("a scanner not marking keeps marks")
	}
	s.feed([]byte("\x1b[?1h"))
	if !s.modes[1] {
		t.Fatal("DECCKM not followed")
	}
}
