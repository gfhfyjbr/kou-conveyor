package fileops

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// Parallel tool calls change the same file at once: every change must land,
// whichever path names the file.
func TestParallelChangesOfOneFileAllLand(t *testing.T) {
	const edits = 24
	dir := t.TempDir()
	var original strings.Builder
	for index := range edits {
		fmt.Fprintf(&original, "line %d\n", index)
	}
	name := write(t, filepath.Join(dir, "shared.txt"), original.String())
	link := filepath.Join(dir, "link.txt")
	if err := os.Symlink(name, link); err != nil {
		t.Fatal(err)
	}
	var wait sync.WaitGroup
	errs := make(chan error, edits)
	for index := range edits {
		wait.Go(func() {
			path := name
			if index%2 == 1 {
				path = link
			}
			old, replacement := fmt.Sprintf("line %d\n", index), fmt.Sprintf("line %d edited\n", index)
			var err error
			if index%3 == 0 {
				_, err = ApplyPatch("*** Begin Patch\n*** Update File: "+path+"\n@@\n-"+strings.TrimSuffix(old, "\n")+"\n+"+strings.TrimSuffix(replacement, "\n")+"\n*** End Patch", dir)
			} else {
				_, err = Edit(path, old, replacement, EditOptions{})
			}
			errs <- err
		})
	}
	wait.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Error(err)
		}
	}
	content := read(t, name)
	for index := range edits {
		if !strings.Contains(content, fmt.Sprintf("line %d edited\n", index)) {
			t.Errorf("the change of line %d was lost:\n%s", index, content)
		}
	}
	locks.Lock()
	defer locks.Unlock()
	if len(locks.held) != 0 {
		t.Fatalf("locks left held: %v", locks.held)
	}
}

func TestWritesOfOneNewFileDoNotInterleave(t *testing.T) {
	name := filepath.Join(t.TempDir(), "new", "file.txt")
	var wait sync.WaitGroup
	for index := range 16 {
		wait.Go(func() {
			if _, err := Write(name, strings.Repeat(fmt.Sprintf("%d", index%10), 1<<16)); err != nil {
				t.Error(err)
			}
		})
	}
	wait.Wait()
	content := read(t, name)
	if len(content) != 1<<16 || strings.Count(content, content[:1]) != len(content) {
		t.Fatalf("the file holds a mix of writes (%d bytes)", len(content))
	}
}
