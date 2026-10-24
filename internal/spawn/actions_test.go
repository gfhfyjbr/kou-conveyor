package spawn

import (
	"fmt"
	"math/rand/v2"
	"testing"
)

// childDescriptors does what posix_spawn does with actions under
// POSIX_SPAWN_CLOEXEC_DEFAULT, in a parent whose descriptors 0 to open-1
// are each a file of its own, named by its number. It returns the file at
// each descriptor the child keeps across exec: the targets of dup2 and
// those inherited, unless closed after.
func childDescriptors(t *testing.T, open int, actions []fileAction) map[int]int {
	t.Helper()
	files := map[int]int{}
	for fd := range open {
		files[fd] = fd
	}
	kept := map[int]bool{}
	for _, action := range actions {
		switch action.op {
		case dupTo:
			file, ok := files[action.fd]
			if !ok {
				t.Fatalf("dup2(%d, %d) of a closed descriptor in %v", action.fd, action.to, actions)
			}
			if action.fd == action.to {
				// dup2 onto itself leaves close-on-exec set.
				t.Fatalf("dup2(%d, %d) in %v", action.fd, action.to, actions)
			}
			files[action.to] = file
			kept[action.to] = true
		case keep:
			if _, ok := files[action.fd]; !ok {
				t.Fatalf("inherit of a closed descriptor %d in %v", action.fd, actions)
			}
			kept[action.fd] = true
		case closeFD:
			if _, ok := files[action.fd]; !ok {
				t.Fatalf("close of a closed descriptor %d in %v", action.fd, actions)
			}
			delete(files, action.fd)
			delete(kept, action.fd)
		default:
			t.Fatalf("unknown action %v", action)
		}
	}
	child := map[int]int{}
	for fd := range kept {
		child[fd] = files[fd]
	}
	return child
}

func checkFileActions(t *testing.T, fds []int) {
	t.Helper()
	open := len(fds) + 4 // and some the child must not get
	for _, fd := range fds {
		open = max(open, fd+5)
	}
	child := childDescriptors(t, open, fileActions(fds))
	want := map[int]int{}
	for i, fd := range fds {
		if fd >= 0 {
			want[i] = fd
		}
	}
	if fmt.Sprint(child) != fmt.Sprint(want) {
		t.Errorf("fileActions(%v) = %v: the child has %v, want %v", fds, fileActions(fds), child, want)
	}
}

func TestFileActionsGiveTheChildEachDescriptorAtItsPlace(t *testing.T) {
	for _, fds := range [][]int{
		{0, 1, 2},
		{5, 6, 7},
		{1, 0, 2},
		{2, 2, 2},
		{2, 1, 0},
		{7, 1, 1, 0},
		{-1, 3, -1, 4},
		{-1, -1, -1},
		{3, 4, 5, 0, 1, 2},
		{9, 8, 7, 6, 5, 4, 3, 2, 1, 0},
		{0, 1, 2, 2, 1, 0, 9},
		{4, 4, 4, 4, 4},
	} {
		checkFileActions(t, fds)
	}

	random := rand.New(rand.NewPCG(1, 2))
	for range 2000 {
		fds := make([]int, 3+random.IntN(8))
		for i := range fds {
			fds[i] = random.IntN(14) - 1
		}
		checkFileActions(t, fds)
	}
}

func TestFileActionsLeaveADescriptorAtItsPlaceAsItIs(t *testing.T) {
	for _, action := range fileActions([]int{0, 1, 2, 3}) {
		if action.op != keep {
			t.Errorf("fileActions(0, 1, 2, 3) has %v: want only keeps", action)
		}
	}
}
