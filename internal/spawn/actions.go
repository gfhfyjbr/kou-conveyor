package spawn

// fileAction is one step that sets up the child's descriptors.
type fileAction struct {
	op     actionOp
	fd, to int
}

type actionOp int

const (
	dupTo   actionOp = iota // dup2(fd, to)
	keep                    // fd stays the child's, as itself
	closeFD                 // close(fd)
)

// fileActions are the steps that give the child fds[i] as its descriptor
// i, -1 for none, in the order the child takes them. A source that a
// dup2 before it would overwrite — one below its own index — moves first
// above every descriptor involved, as fork's child does; the moved copies
// are closed at the end. Whatever else is open stays out of the child:
// posix_spawn is asked to close it.
func fileActions(fds []int) []fileAction {
	next := len(fds)
	for _, fd := range fds {
		next = max(next, fd+1)
	}
	sources := append([]int(nil), fds...)
	var actions []fileAction
	var moved []int
	for i, fd := range sources {
		if fd >= 0 && fd < i {
			actions = append(actions, fileAction{op: dupTo, fd: fd, to: next})
			sources[i] = next
			moved = append(moved, next)
			next++
		}
	}
	for i, fd := range sources {
		switch {
		case fd < 0:
		case fd == i:
			actions = append(actions, fileAction{op: keep, fd: i})
		default:
			actions = append(actions, fileAction{op: dupTo, fd: fd, to: i})
		}
	}
	for _, fd := range moved {
		actions = append(actions, fileAction{op: closeFD, fd: fd})
	}
	return actions
}
