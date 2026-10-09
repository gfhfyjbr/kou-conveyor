package fileops

import (
	"path/filepath"
	"slices"
	"sync"
)

// Changes to a file are made one at a time. Edit, Write and ApplyPatch read
// a file and write it whole; two of them on the same file at once, as a
// model's parallel tool calls make them, would each write what it read
// before the other wrote, and one change would be lost while both report
// success. Each takes the locks of the files it changes first: a change
// made after another reads what the other wrote.

// locks holds a lock for each file being changed, while it is.
var locks = struct {
	sync.Mutex
	held map[string]*pathLock
}{held: map[string]*pathLock{}}

type pathLock struct {
	sync.Mutex
	users int
}

// lockFiles takes the locks of the named files, in a fixed order so that
// changes of several files do not wait on each other in a circle, and
// returns what releases them.
func lockFiles(names ...string) (unlock func()) {
	keys := make([]string, 0, len(names))
	for _, name := range names {
		keys = append(keys, lockKey(name))
	}
	slices.Sort(keys)
	keys = slices.Compact(keys)
	taken := make([]*pathLock, len(keys))
	for index, key := range keys {
		locks.Lock()
		lock := locks.held[key]
		if lock == nil {
			lock = &pathLock{}
			locks.held[key] = lock
		}
		lock.users++
		locks.Unlock()
		lock.Lock()
		taken[index] = lock
	}
	return func() {
		for index := len(taken) - 1; index >= 0; index-- {
			taken[index].Unlock()
			locks.Lock()
			if taken[index].users--; taken[index].users == 0 {
				delete(locks.held, keys[index])
			}
			locks.Unlock()
		}
	}
}

// lockKey names a file the same way whatever path leads to it: absolute,
// with symbolic links resolved, those of its directory too when the file
// does not exist yet.
func lockKey(name string) string {
	absolute, err := filepath.Abs(name)
	if err != nil {
		absolute = filepath.Clean(name)
	}
	if resolved, err := filepath.EvalSymlinks(absolute); err == nil {
		return resolved
	}
	directory, base := filepath.Split(absolute)
	if resolved, err := filepath.EvalSymlinks(directory); err == nil {
		return filepath.Join(resolved, base)
	}
	return absolute
}
