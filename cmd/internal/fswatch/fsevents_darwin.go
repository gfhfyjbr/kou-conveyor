//go:build darwin && cgo

package fswatch

/*
#cgo LDFLAGS: -framework CoreServices
#include <CoreServices/CoreServices.h>
#include <dispatch/dispatch.h>
#include <stdint.h>
#include <stdlib.h>

extern void kouFSWatchChanged(uintptr_t watcher);

static void kou_fswatch_callback(ConstFSEventStreamRef stream, void *info, size_t count, void *paths,
	const FSEventStreamEventFlags flags[], const FSEventStreamEventId ids[]) {
	kouFSWatchChanged((uintptr_t)info);
}

typedef struct {
	FSEventStreamRef stream;
	dispatch_queue_t queue;
} kou_fswatch;

// kou_fswatch_start follows the files under the n paths from now on, and
// calls back with watcher for each batch of changes, latency seconds after
// the first; NULL if FSEvents would not.
static kou_fswatch *kou_fswatch_start(char **paths, int n, uintptr_t watcher, double latency) {
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
	FSEventStreamContext context = {0, (void *)watcher, NULL, NULL, NULL};
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

// kou_fswatch_stop stops following: once it returns, nothing calls back.
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
	"runtime/cgo"
	"strings"
	"unsafe"
)

// latency is how long FSEvents gathers changes after the first, before it
// tells of them: an editor's save, a checkout, a build come as one.
const latency = 0.1

func watch(roots []string, changes chan struct{}) (func(), error) {
	paths := make([]*C.char, len(roots))
	for i, root := range roots {
		paths[i] = C.CString(root)
	}
	defer func() {
		for _, path := range paths {
			C.free(unsafe.Pointer(path))
		}
	}()
	handle := cgo.NewHandle(changes)
	w := C.kou_fswatch_start(&paths[0], C.int(len(paths)), C.uintptr_t(handle), latency)
	if w == nil {
		handle.Delete()
		return nil, fmt.Errorf("fswatch: FSEvents would not follow %s", strings.Join(roots, ", "))
	}
	return func() {
		C.kou_fswatch_stop(w)
		handle.Delete()
	}, nil
}
