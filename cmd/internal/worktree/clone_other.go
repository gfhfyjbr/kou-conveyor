//go:build !darwin

package worktree

import "errors"

// clone is copying copy-on-write, which only macOS's clonefile does here.
func clone(source, target string) error { return errors.ErrUnsupported }
