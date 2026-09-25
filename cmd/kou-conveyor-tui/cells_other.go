//go:build !unix

package main

// cellPixels is the size of a cell in pixels; this platform does not say.
func cellPixels() (width, height int) { return 0, 0 }
