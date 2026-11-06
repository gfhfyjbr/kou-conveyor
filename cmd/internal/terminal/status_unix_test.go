//go:build unix

package terminal

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestProgramStatusQueryIsAnsweredInPTYWithoutBrowser(t *testing.T) {
	m := NewManager(nil, "test")
	defer m.Close()
	s, err := m.Start(Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := s.Attach()
	defer c.Detach()
	reply := "\x1b]7501;" + programStatusReply + "\x07"
	command := fmt.Sprintf("stty -echo -icanon min 1 time 0; printf '\\033]7501;?\\007'; dd bs=1 count=%d 2>/dev/null | od -An -tx1; stty echo icanon; printf '\\nquery-%%s\\n' 7501\n", len(reply))
	if err := s.Write([]byte(command)); err != nil {
		t.Fatal(err)
	}
	input := s.LastInput()
	got, _ := collect(t, c, "query-7501", 5*time.Second)
	want := fmt.Sprintf("% x", []byte(reply))
	if normalized := strings.Join(strings.Fields(got), " "); !strings.Contains(normalized, want) {
		t.Fatalf("program did not read the real protocol reply: %q, want bytes %q", got, want)
	}
	if !s.LastInput().Equal(input) {
		t.Fatal("a terminal-generated reply counted as user input")
	}
	if state := s.ProgramState(); state.Reported {
		t.Fatalf("capability query enabled status precedence: %#v", state)
	}
}

func TestProgramStatusForegroundExitAndNextProgram(t *testing.T) {
	m := NewManager(nil, "test")
	defer m.Close()
	s, err := m.Start(Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := s.Attach()
	defer c.Detach()
	states := make(chan ProgramState, 32)
	cancel := s.Observe(func(ev Event) {
		for _, ev := range ev.Programs {
			select {
			case states <- ev.State:
			default:
			}
		}
	})
	defer cancel()
	if err := s.Write([]byte("sh -c 'printf \"\\033]7501;state=blocked:app=claude-code:kind=permission\\007\"; read answer; printf \"accepted-%s\\n\" \"$answer\"'\n")); err != nil {
		t.Fatal(err)
	}
	deadline := time.After(5 * time.Second)
	for {
		select {
		case state := <-states:
			if r := state.Record(""); state.Reported && r != nil && r.State == "blocked" {
				goto blocked
			}
		case <-deadline:
			t.Fatal("the foreground program never reported blocked")
		}
	}
blocked:
	// Direct terminal input must remain possible even while automated
	// canvas delivery is guarded against answering a permission prompt.
	if err := s.Write([]byte("yes\n")); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "accepted-yes", 5*time.Second)
	if err := s.Write([]byte("sh -c 'printf \"next-%s\\n\" program; read answer'\n")); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "next-program", 5*time.Second)
	if state := s.ProgramState(); state.Reported || state.Record("") != nil {
		t.Fatalf("the next nonreporting program inherited blocked/precedence: %#v", state)
	}
}
