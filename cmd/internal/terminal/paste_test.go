//go:build unix

package terminal

import (
	"strings"
	"sync"
	"testing"
	"time"
)

func TestPasteIsBracketedWhenAskedAndKeysFollowDECCKM(t *testing.T) {
	m := cleanManager(t)
	s, err := m.Start(Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh", Owner: "canvas:w/c/n"})
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	if list, owned := m.List("w"), m.ListOwned("canvas:"); len(list) != 0 || len(owned) != 1 || owned[0].Owner != "canvas:w/c/n" {
		t.Fatalf("listed %+v, owned %+v", list, owned)
	}
	var mu sync.Mutex
	var marks []Mark
	heard := make(chan struct{}, 64)
	cancel := s.Observe(func(ev Event) {
		mu.Lock()
		defer mu.Unlock()
		marks = append(marks, ev.Marks...)
		select {
		case heard <- struct{}{}:
		default:
		}
	})
	defer cancel()
	c, _ := s.Attach()

	// Not asked for, a paste goes in as typed.
	if err := s.Paste("echo plain-$((1+1))", PasteOptions{Bracketed: true, Submit: "\r"}); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "plain-2", 10*time.Second)

	// cat -v shows what it is given: the program asks for bracketed pastes
	// and the application form of the cursor keys.
	if err := s.Write([]byte("stty -echo; printf '\\033[?2004h\\033[?1h\\033]133;C\\007'; cat -v\n")); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(10 * time.Second)
	for !s.BracketedPaste() || !s.ApplicationCursor() {
		if time.Now().After(deadline) {
			t.Fatal("the modes asked for were not seen")
		}
		time.Sleep(10 * time.Millisecond)
	}
	if err := s.Paste("one\ntwo\x1b[201~", PasteOptions{Bracketed: true, Submit: "\r", SubmitDelay: 20 * time.Millisecond}); err != nil {
		t.Fatal(err)
	}
	out, _ := collect(t, c, "^[[201~", 10*time.Second)
	if !strings.Contains(out, "^[[200~one") || !strings.Contains(out, "two[201~^[[201~") {
		t.Fatalf("pasted %q", out)
	}
	if err := s.Keys("Up", "x", "Enter"); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "^[OAx", 10*time.Second)
	if err := s.Keys("C-é"); err == nil {
		t.Fatal("C-é is no key")
	}
	select {
	case <-heard:
	case <-time.After(time.Second):
		t.Fatal("the observer heard nothing")
	}
	s.Keys("C-c")
	if err := s.Write([]byte("exit 4\n")); err != nil {
		t.Fatal(err)
	}
	if _, code := collect(t, c, "", 10*time.Second); code == nil || *code != 4 {
		t.Fatalf("exit %v", code)
	}
	if err := s.Paste("late", PasteOptions{}); err == nil {
		t.Fatal("a paste into a shell that ended")
	}
	mu.Lock()
	defer mu.Unlock()
	if len(marks) == 0 || marks[0].Kind != 'C' {
		t.Fatalf("marks %+v", marks)
	}
}
