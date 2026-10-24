package spawn

import (
	"strings"
	"testing"
)

func TestPrefixSuffixSaverKeepsTheStartAndTheEnd(t *testing.T) {
	text := "0123456789abcdefghijklmnopqrstuvwxyz"
	for _, test := range []struct {
		n    int
		want string
	}{
		{10, "0123456789\n... omitting 16 bytes ...\nqrstuvwxyz"},
		{18, "0123456789abcdefgh" + "ijklmnopqrstuvwxyz"},
		{36, text},
		{40, text},
	} {
		for _, chunk := range []int{1, 7, 10, 11, len(text)} {
			saver := &prefixSuffixSaver{N: test.n}
			for rest := text; rest != ""; {
				part := rest[:min(chunk, len(rest))]
				rest = rest[len(part):]
				if n, err := saver.Write([]byte(part)); n != len(part) || err != nil {
					t.Fatalf("Write(%q) = %d, %v", part, n, err)
				}
			}
			if got := string(saver.Bytes()); got != test.want {
				t.Errorf("N %d, written %d bytes at a time: %q, want %q", test.n, chunk, got, test.want)
			}
		}
	}
	if got := string((&prefixSuffixSaver{N: 4}).Bytes()); got != "" {
		t.Errorf("nothing written: %q", got)
	}
	if strings.Contains(text, "\n") {
		t.Fatal("the text has a newline")
	}
}
