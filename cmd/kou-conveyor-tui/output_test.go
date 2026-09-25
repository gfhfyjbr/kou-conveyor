package main

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/charmbracelet/x/term"
)

// The screen and the pictures write to the terminal one at a time: a
// sequence never cuts into a frame.
func TestTerminalOutputKeepsWritesWhole(t *testing.T) {
	f, err := os.Create(filepath.Join(t.TempDir(), "tty"))
	if err != nil {
		t.Fatal(err)
	}
	out := newTerminalOutput(f)
	// Bubble Tea takes it for the terminal's file.
	if _, ok := any(out).(term.File); !ok {
		t.Fatal("not a terminal file")
	}
	frame := strings.Repeat("\x1b[38;2;1;2;3mframe text\x1b[39m", 4000)
	image := "\x1b_Ga=t,f=100;" + strings.Repeat("iVBORw0KGgo", 4000) + "\x1b\\"
	var wg sync.WaitGroup
	for range 20 {
		wg.Add(2)
		go func() { defer wg.Done(); out.Write([]byte(frame)) }()
		go func() { defer wg.Done(); out.Write([]byte(image)) }()
	}
	wg.Wait()
	f.Close()
	got, err := os.ReadFile(f.Name())
	if err != nil {
		t.Fatal(err)
	}
	if n := bytes.Count(got, []byte(frame)); n != 20 {
		t.Fatalf("%d frames came out whole", n)
	}
	if n := bytes.Count(got, []byte(image)); n != 20 {
		t.Fatalf("%d images came out whole", n)
	}
}
