package highlight

import (
	"context"
	"reflect"
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

func TestLinesSplitsRunsByLine(t *testing.T) {
	for _, c := range []struct {
		name, text string
		runs       []int32
		want       [][]int32
	}{
		// Plain past the last run; an empty line has no runs.
		{"lines", "ab\ncd\n\nef", []int32{2, 1, 1, 0, 2, 2, 3, 0}, [][]int32{{2, 1}, {2, 2}, nil, {2, 0}}},
		// A comment over two lines colours both.
		{"run over lines", "/*a\nb*/", []int32{7, 5}, [][]int32{{3, 5}, {3, 5}}},
		// A newline at the end makes no line after it.
		{"newline at the end", "a\n", []int32{1, 3, 1, 0}, [][]int32{{1, 3}}},
		// Lengths count UTF-16 units: 😀 is two.
		{"utf-16", "é😀\nx", []int32{3, 2, 2, 0}, [][]int32{{3, 2}, {1, 0}}},
		{"no runs", "a\nb", nil, [][]int32{{1, 0}, {1, 0}}},
	} {
		if got := Lines(c.text, c.runs); !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s: Lines = %v, want %v", c.name, got, c.want)
		}
	}
}

func TestAppendLengthensARunOfTheSameClass(t *testing.T) {
	runs := Append(nil, 2, 1)
	runs = Append(runs, 3, 1)
	runs = Append(runs, 0, 4)
	runs = Append(runs, 1, 0)
	if want := []int32{5, 1, 1, 0}; !reflect.DeepEqual(runs, want) {
		t.Fatalf("runs = %v, want %v", runs, want)
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
