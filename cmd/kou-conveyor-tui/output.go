package main

import (
	"os"
	"sync"
)

// terminalOutput is the terminal as the cockpit writes to it, by one
// goroutine at a time. Bubble Tea's renderer writes each frame in one
// call, and the pictures' sequences (graphics.go) go out from a goroutine
// of their own; on a terminal that drains its output slowly a write blocks
// part way, and without the lock the next writer's bytes cut into it — a
// chunk of an image inside an escape sequence, whose tail then prints as
// text, wraps the row and scrolls the screen under the renderer, which
// leaves the rows it does not draw again a row off. It is still the file
// the terminal is on: Bubble Tea asks it for the window's size and its
// resizes through Fd.
type terminalOutput struct {
	*os.File
	mu sync.Mutex
}

func newTerminalOutput(f *os.File) *terminalOutput { return &terminalOutput{File: f} }

func (o *terminalOutput) Write(p []byte) (int, error) {
	o.mu.Lock()
	defer o.mu.Unlock()
	return o.File.Write(p)
}
