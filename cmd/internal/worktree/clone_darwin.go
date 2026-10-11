package worktree

import "golang.org/x/sys/unix"

// clone copies a file, or a folder with all it holds, copy-on-write: on
// APFS the copy shares the original's blocks until one of them is
// written. A link is cloned as a link.
func clone(source, target string) error {
	return unix.Clonefile(source, target, unix.CLONE_NOFOLLOW)
}
