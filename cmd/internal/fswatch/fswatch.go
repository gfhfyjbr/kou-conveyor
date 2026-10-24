// Package fswatch tells of changes to the files under a few paths as the
// system sees them — FSEvents on macOS — so that what follows files can look
// at them when they change, rather than walk them on a timer.
package fswatch

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sync"
)

// A Watcher follows the files under its roots.
type Watcher struct {
	changes chan struct{}
	stop    func()
	once    sync.Once
}

// Watch follows the files under roots — directories, the files and
// directories in them, and files — from now on. A root need not exist yet:
// what is made there is told of too. Where the system tells of no changes
// (not macOS, or a build without cgo) it returns an error that is
// errors.ErrUnsupported, and the caller looks for changes itself.
func Watch(roots ...string) (*Watcher, error) {
	if len(roots) == 0 {
		return nil, errors.New("fswatch: no roots to watch")
	}
	resolved := make([]string, 0, len(roots))
	for _, root := range roots {
		path, err := resolve(root)
		if err != nil {
			return nil, err
		}
		resolved = append(resolved, path)
	}
	slices.Sort(resolved)
	w := &Watcher{changes: make(chan struct{}, 1)}
	stop, err := watch(slices.Compact(resolved), w.changes)
	if err != nil {
		return nil, err
	}
	w.stop = stop
	return w, nil
}

// Changes receives once something under the roots changed since it last
// did: changes coalesce, and a receive says to look, not what changed.
func (w *Watcher) Changes() <-chan struct{} { return w.changes }

// Close stops the watcher: Changes receives nothing it did not have yet.
// Close may be called more than once.
func (w *Watcher) Close() { w.once.Do(w.stop) }

// changed tells a watcher's channel of a change, unless it has one to tell
// already.
func changed(changes chan struct{}) {
	select {
	case changes <- struct{}{}:
	default:
	}
}

// resolve makes a root absolute, with the links on its way resolved — the
// system tells of changes by the paths the files really have. Of a root that
// is not there yet, the part that is is resolved.
func resolve(root string) (string, error) {
	path, err := filepath.Abs(root)
	if err != nil {
		return "", err
	}
	missing := ""
	for {
		if resolved, err := filepath.EvalSymlinks(path); err == nil {
			return filepath.Join(resolved, missing), nil
		} else if !errors.Is(err, os.ErrNotExist) {
			return "", err
		}
		parent := filepath.Dir(path)
		if parent == path {
			return filepath.Join(path, missing), nil
		}
		missing = filepath.Join(filepath.Base(path), missing)
		path = parent
	}
}
