//go:build darwin && cgo

package fswatch

// #include <stdint.h>
import "C"

import "runtime/cgo"

// kouFSWatchChanged is FSEvents' callback, on its queue: the files under a
// watcher's roots changed.
//
//export kouFSWatchChanged
func kouFSWatchChanged(watcher C.uintptr_t) {
	changed(cgo.Handle(watcher).Value().(chan struct{}))
}
