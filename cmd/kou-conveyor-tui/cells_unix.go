//go:build unix

package main

import (
	"os"

	"golang.org/x/sys/unix"
)

// cellPixels is the size of a cell in pixels, as the terminal reports it;
// zero when it does not.
func cellPixels() (width, height int) {
	size, err := unix.IoctlGetWinsize(int(os.Stdout.Fd()), unix.TIOCGWINSZ)
	if err != nil || size.Col == 0 || size.Row == 0 {
		return 0, 0
	}
	return int(size.Xpixel) / int(size.Col), int(size.Ypixel) / int(size.Row)
}
