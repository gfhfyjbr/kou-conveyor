package highlight

import (
	"context"
	"strings"
	"testing"
	"time"
)

// spans turns a result back into the text's runs, as a page reads them.
func spans(t *testing.T, text string, r Result) []string {
	t.Helper()
	units := []rune(text) // the texts here are all in the basic plane
	var out []string
	at := 0
	for i := 0; i+1 < len(r.Runs); i += 2 {
		n, class := int(r.Runs[i]), Classes[r.Runs[i+1]]
		if at+n > len(units) {
			t.Fatalf("runs go past the text: %v", r.Runs)
		}
		if class != "" && strings.TrimSpace(string(units[at:at+n])) != "" {
			out = append(out, class+":"+strings.TrimSpace(string(units[at:at+n])))
		}
		at += n
	}
	return out
}

func TestHighlightNamesRunsByClass(t *testing.T) {
	text := "package main\n\n// Say hi.\nfunc main() { println(\"hi\", 42) }\n"
	r := Highlight("cmd/main.go", text, time.Time{})
	if r.Language != "Go" || !r.Complete {
		t.Fatalf("result = %+v", r)
	}
	got := strings.Join(spans(t, text, r), " ")
	for _, want := range []string{"kn:package", "c:// Say hi.", "kd:func", "nf:main", "nb:println", `s:"hi"`, "m:42"} {
		if !strings.Contains(got, want) {
			t.Fatalf("%q lacks %q", got, want)
		}
	}
}

func TestHighlightCountsUTF16AndStaysWithinTheText(t *testing.T) {
	text := "x = \"😀é\"" // no newline at the end: the lexer adds one
	r := Highlight("a.py", text, time.Time{})
	total := int32(0)
	for i := 0; i < len(r.Runs); i += 2 {
		total += r.Runs[i]
	}
	if total != utf16Len(text) || utf16Len(text) != 9 {
		t.Fatalf("runs cover %d units of %d: %v", total, utf16Len(text), r.Runs)
	}
}

func TestHighlightFindsScriptsByTheirFirstLine(t *testing.T) {
	if got := Language("bin/deploy", "#!/bin/bash\necho hi\n"); got != "Bash" {
		t.Fatalf("language = %q", got)
	}
	if got := Language("notes", "just words\n"); got != "" {
		t.Fatalf("plain text = %q", got)
	}
	r := Highlight("notes", "just words\n", time.Time{})
	if !r.Complete || len(r.Runs) != 0 {
		t.Fatalf("plain = %+v", r)
	}
}

func TestCacheFinishesInTheBackground(t *testing.T) {
	text := strings.Repeat("func f() { return 1 + 2 } // x\n", 20000)
	c := NewCache(10_000_000)
	first := c.Get("k", "big.go", text, time.Nanosecond)
	if first.Complete {
		t.Fatal("highlighted within a nanosecond")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	full, err := c.Wait(ctx, "k", "big.go", text)
	if err != nil || !full.Complete || len(full.Runs) <= len(first.Runs) {
		t.Fatalf("full = %d runs, complete %v, %v", len(full.Runs), full.Complete, err)
	}
	if again := c.Get("k", "big.go", text, time.Nanosecond); !again.Complete {
		t.Fatal("not kept")
	}
}

func TestCacheLetsTheOldestGo(t *testing.T) {
	c := NewCache(40)
	text := "a := 1\nb := 2\n"
	for _, key := range []string{"a", "b", "c", "d"} {
		c.Get(key, "x.go", text, time.Second)
	}
	if _, ok := c.lookup("a"); ok {
		t.Fatal("the oldest is kept")
	}
	if _, ok := c.lookup("d"); !ok {
		t.Fatal("the newest is gone")
	}
}
