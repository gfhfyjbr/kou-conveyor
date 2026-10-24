//go:build darwin && cgo

package fswatch

/*
#cgo LDFLAGS: -framework CoreServices
#include <CoreServices/CoreServices.h>
#include <dispatch/dispatch.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

// kou_fswatch_callback tells of a batch of changes with a byte down the
// watcher's pipe, for Go to read. It runs on a thread of the system's, the
// stream's queue's, and calls no Go: Go called back on such a thread can be
// left owing a preemption signal the thread never takes (it blocks them), and
// syscall.Exec then waits for it for ever (runtime_BeforeExec on darwin). A
// pipe too full to take the byte has bytes to read already.
static void kou_fswatch_callback(ConstFSEventStreamRef stream, void *info, size_t count, void *paths,
	const FSEventStreamEventFlags flags[], const FSEventStreamEventId ids[]) {
	char b = 0;
	(void)write((int)(intptr_t)info, &b, 1);
}

typedef struct {
	FSEventStreamRef stream;
	dispatch_queue_t queue;
} kou_fswatch;

// kou_fswatch_start follows the files under the n paths from now on, and
// writes a byte to fd for each batch of changes, latency seconds after the
// first; NULL if FSEvents would not.
static kou_fswatch *kou_fswatch_start(char **paths, int n, int fd, double latency) {
	CFMutableArrayRef array = CFArrayCreateMutable(NULL, n, &kCFTypeArrayCallBacks);
	if (array == NULL) return NULL;
	for (int i = 0; i < n; i++) {
		CFStringRef path = CFStringCreateWithCString(NULL, paths[i], kCFStringEncodingUTF8);
		if (path == NULL) {
			CFRelease(array);
			return NULL;
		}
		CFArrayAppendValue(array, path);
		CFRelease(path);
	}
	FSEventStreamContext context = {0, (void *)(intptr_t)fd, NULL, NULL, NULL};
	FSEventStreamRef stream = FSEventStreamCreate(NULL, kou_fswatch_callback, &context, array,
		kFSEventStreamEventIdSinceNow, latency, kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer);
	CFRelease(array);
	if (stream == NULL) return NULL;
	dispatch_queue_t queue = dispatch_queue_create("kou-conveyor.fswatch", DISPATCH_QUEUE_SERIAL);
	FSEventStreamSetDispatchQueue(stream, queue);
	if (!FSEventStreamStart(stream)) {
		FSEventStreamInvalidate(stream);
		FSEventStreamRelease(stream);
		dispatch_release(queue);
		return NULL;
	}
	kou_fswatch *w = malloc(sizeof *w);
	w->stream = stream;
	w->queue = queue;
	return w;
}

static void kou_fswatch_nothing(void *context) {}

// kou_fswatch_stop stops following: once it returns, nothing writes.
static void kou_fswatch_stop(kou_fswatch *w) {
	FSEventStreamStop(w->stream);
	FSEventStreamInvalidate(w->stream);
	dispatch_sync_f(w->queue, NULL, kou_fswatch_nothing); // a callback under way is done
	FSEventStreamRelease(w->stream);
	dispatch_release(w->queue);
	free(w);
}
*/
import "C"

import (
	"fmt"
	"os"
	"strings"
	"sync/atomic"
	"syscall"
	"unsafe"
)

// latency is how long FSEvents gathers changes after the first, before it
// tells of them: an editor's save, a checkout, a build come as one.
const latency = 0.1

func watch(roots []string, changes chan struct{}) (func(), error) {
	read, write, err := pipe()
	if err != nil {
		return nil, err
	}
	paths := make([]*C.char, len(roots))
	for i, root := range roots {
		paths[i] = C.CString(root)
	}
	defer func() {
		for _, path := range paths {
			C.free(unsafe.Pointer(path))
		}
	}()
	w := C.kou_fswatch_start(&paths[0], C.int(len(paths)), C.int(write), latency)
	if w == nil {
		read.Close()
		syscall.Close(write)
		return nil, fmt.Errorf("fswatch: FSEvents would not follow %s", strings.Join(roots, ", "))
	}
	var stopped atomic.Bool
	done := make(chan struct{})
	go func() {
		defer close(done)
		var bytes [64]byte
		for {
			n, err := read.Read(bytes[:])
			if n > 0 && !stopped.Load() {
				changed(changes)
			}
			if err != nil {
				return // the end: stopped
			}
		}
	}()
	return func() {
		C.kou_fswatch_stop(w) // nothing writes from here on
		stopped.Store(true)
		read.Close() // the reader's Read returns
		<-done
		syscall.Close(write)
	}, nil
}

// pipe makes the pipe the stream's callback writes down. Its write end is a
// bare descriptor that does not block, which nothing but the watcher's stop
// closes (an os.File's finalizer could, while FSEvents still wrote to it);
// its read end a file a goroutine waits on without holding a thread. Neither
// is handed to what the process executes.
func pipe() (*os.File, int, error) {
	var fds [2]int
	syscall.ForkLock.RLock()
	err := syscall.Pipe(fds[:])
	if err == nil {
		syscall.CloseOnExec(fds[0])
		syscall.CloseOnExec(fds[1])
	}
	syscall.ForkLock.RUnlock()
	if err != nil {
		return nil, -1, fmt.Errorf("fswatch: %w", os.NewSyscallError("pipe", err))
	}
	for _, fd := range fds {
		if err := syscall.SetNonblock(fd, true); err != nil {
			syscall.Close(fds[0])
			syscall.Close(fds[1])
			return nil, -1, fmt.Errorf("fswatch: %w", os.NewSyscallError("setnonblock", err))
		}
	}
	return os.NewFile(uintptr(fds[0]), "fswatch"), fds[1], nil
}
