//go:build unix

package terminal

import (
	"bytes"
	"encoding/json/v2"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"testing/fstest"
	"time"

	"github.com/creack/pty"
	"golang.org/x/sys/unix"
)

func TestRingKeepsTheLastBytesAndSaysWhatLeaves(t *testing.T) {
	var evicted []byte
	r := newRing(8, func(p []byte) { evicted = append(evicted, p...) })
	r.write([]byte("abc"))
	if got := string(r.bytes()); got != "abc" {
		t.Fatalf("bytes = %q", got)
	}
	r.write([]byte("defghij"))
	if got := string(r.bytes()); got != "cdefghij" || string(evicted) != "ab" {
		t.Fatalf("bytes = %q, evicted %q", got, evicted)
	}
	r.write([]byte("klm"))
	if got := string(r.bytes()); got != "fghijklm" || string(evicted) != "abcde" {
		t.Fatalf("bytes = %q, evicted %q", got, evicted)
	}
	r.write([]byte("0123456789"))
	if got := string(r.bytes()); got != "23456789" || string(evicted) != "abcdefghijklm01" {
		t.Fatalf("bytes = %q, evicted %q", got, evicted)
	}
}

func TestScannerFollowsTitleDirectoryAndModes(t *testing.T) {
	s := newScanner()
	if !s.feed([]byte("hi \x1b]0;first\x07 there")) || s.title != "first" {
		t.Fatalf("title = %q", s.title)
	}
	// A sequence split between two reads.
	s.feed([]byte("\x1b]2;sec"))
	if !s.feed([]byte("ond\x1b\\")) || s.title != "second" {
		t.Fatalf("title = %q", s.title)
	}
	if !s.feed([]byte("\x1b]7;file://host/Users/me/My%20Dir\x07")) || s.dir != "/Users/me/My Dir" {
		t.Fatalf("dir = %q", s.dir)
	}
	if s.feed([]byte("\x1b]7;file://host/Users/me/My%20Dir\x07")) {
		t.Fatal("the same directory is no change")
	}
	s.feed([]byte("\x1b[2J\x1b[?1049h\x1b[?2004;1006h"))
	prefix := string(s.prefix())
	for _, want := range []string{"\x1b[?1049h", "\x1b[?2004h", "\x1b[?1006h"} {
		if !strings.Contains(prefix, want) {
			t.Fatalf("prefix %q lacks %q", prefix, want)
		}
	}
	s.feed([]byte("\x1b[?1049l\x1b[?25l"))
	prefix = string(s.prefix())
	if strings.Contains(prefix, "1049h") || !strings.Contains(prefix, "\x1b[?25l") {
		t.Fatalf("prefix = %q", prefix)
	}
	restored := newScanner()
	restored.restoreModes(s.setModes())
	if got := string(restored.prefix()); got != prefix {
		t.Fatalf("restored prefix = %q, want %q", got, prefix)
	}
}

func TestMaterializeNamesTheDirectoryByContent(t *testing.T) {
	files := fstest.MapFS{
		"zsh/zshenv":    {Data: []byte("# env\n")},
		"zsh/kou.zsh":   {Data: []byte("# theme\n")},
		"bash/kou.bash": {Data: []byte("# bash\n")},
	}
	base := t.TempDir()
	dir, err := materialize(files, base)
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"zsh/.zshenv", "zsh/kou.zsh", "bash/kou.bash"} {
		if _, err := os.Stat(filepath.Join(dir, name)); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
	}
	again, err := materialize(files, base)
	if err != nil || again != dir {
		t.Fatalf("again = %q, %v; want %q", again, err, dir)
	}
	files["zsh/kou.zsh"] = &fstest.MapFile{Data: []byte("# another theme\n")}
	other, err := materialize(files, base)
	if err != nil || other == dir {
		t.Fatalf("changed files = %q, %v", other, err)
	}
}

func TestCommandInjectsTheIntegrationOnlyWithTheTheme(t *testing.T) {
	base := []string{"HOME=/home/me", "ZDOTDIR=/home/me/.config/zsh", "TERM=screen", "ITERM_SESSION_ID=x", "SHLVL=3", "PATH=/bin", "LANG=en_US.UTF-8"}
	integration := &Integration{Dir: "/cache/shell-1"}
	themed := command("/bin/zsh", Spec{Dir: "/work", Theme: true}, "id1", "v1", base, integration)
	if themed.args[0] != "-zsh" {
		t.Fatalf("args = %q", themed.args)
	}
	for _, want := range []string{"ZDOTDIR=/cache/shell-1/zsh", "KOU_CONVEYOR_USER_ZDOTDIR=/home/me/.config/zsh", "TERM=xterm-256color",
		"TERM_PROGRAM=kou-conveyor", "KOU_CONVEYOR_TERMINAL=id1", "PWD=/work", "PATH=/bin"} {
		if !contains(themed.env, want) {
			t.Fatalf("env %q lacks %q", themed.env, want)
		}
	}
	for _, gone := range []string{"TERM=screen", "ITERM_SESSION_ID=x", "SHLVL=3", "ZDOTDIR=/home/me/.config/zsh"} {
		if contains(themed.env, gone) {
			t.Fatalf("env %q keeps %q", themed.env, gone)
		}
	}
	plain := command("/bin/zsh", Spec{Dir: "/work"}, "id1", "v1", base, integration)
	if v, ok := lookup(plain.env, "ZDOTDIR"); ok {
		t.Fatalf("without the theme ZDOTDIR = %q", v)
	}
	bash := command("/usr/local/bin/bash", Spec{Dir: "/work", Theme: true}, "id1", "v1", base, integration)
	if strings.Join(bash.args, " ") != "bash --rcfile /cache/shell-1/bash/kou.bash -i" || !contains(bash.env, "KOU_CONVEYOR_BASH_LOGIN=1") {
		t.Fatalf("bash = %q", bash.args)
	}
	fish := command("/opt/homebrew/bin/fish", Spec{Dir: "/work", Theme: true}, "id1", "v1", base, integration)
	if len(fish.args) != 3 || fish.args[1] != "--init-command" || fish.args[2] != "source '/cache/shell-1/fish/kou.fish'" {
		t.Fatalf("fish = %q", fish.args)
	}
	other := command("/bin/sh", Spec{Dir: "/work", Theme: true}, "id1", "v1", base, integration)
	if strings.Join(other.args, " ") != "-sh" || contains(other.env, "KOU_CONVEYOR_TERMINAL_THEME=kou") {
		t.Fatalf("sh = %q", other.args)
	}
}

func contains(list []string, want string) bool {
	for _, item := range list {
		if item == want {
			return true
		}
	}
	return false
}

// collect reads what a page is sent until want is in the output, or the
// time runs out.
func collect(t *testing.T, c *Client, want string, timeout time.Duration) (string, *int) {
	t.Helper()
	var out bytes.Buffer
	deadline := time.After(timeout)
	for {
		select {
		case m := <-c.Messages():
			out.Write(m.Output)
			if m.Exit != nil {
				return out.String(), m.Exit
			}
			if want != "" && strings.Contains(out.String(), want) {
				return out.String(), nil
			}
		case <-deadline:
			t.Fatalf("no %q in %q", want, out.String())
			return "", nil
		}
	}
}

// cleanManager runs shells in a home of their own, which has no startup
// files to wait for.
func cleanManager(t *testing.T) *Manager {
	m := NewManager(nil, "test")
	home := t.TempDir()
	m.environment = func() []string { return []string{"HOME=" + home, "PATH=/usr/bin:/bin", "PS1=$ "} }
	return m
}

func TestShellRunsResizesAndEnds(t *testing.T) {
	m := cleanManager(t)
	dir := t.TempDir()
	s, err := m.Start(Spec{Workspace: "w", Dir: dir, Shell: "/bin/sh", Cols: 80, Rows: 24})
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	c, _ := s.Attach()
	if err := s.Write([]byte("echo kou-$((40+2))\n")); err != nil {
		t.Fatal(err)
	}
	collect(t, c, "kou-42", 10*time.Second)
	if err := s.Resize(101, 31, 0, 0); err != nil {
		t.Fatal(err)
	}
	// bash may put the old size back, and is given the new one again.
	time.Sleep(2 * resizeCheck)
	s.Write([]byte("stty size\n"))
	collect(t, c, "31 101", 10*time.Second)
	want, _ := filepath.EvalSymlinks(dir)
	if got, _ := filepath.EvalSymlinks(s.Dir()); got != want {
		t.Fatalf("dir = %q, want %q", got, want)
	}
	if list := m.List("w"); len(list) != 1 || list[0].ID != s.ID() || list[0].Clients != 1 || list[0].Running != "" {
		t.Fatalf("list = %+v", list)
	}
	// A program in the foreground is named.
	s.Write([]byte("sleep 30\n"))
	deadline := time.Now().Add(5 * time.Second)
	for s.Info().Running != "sleep" && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if running := s.Info().Running; running != "sleep" {
		t.Fatalf("running = %q", running)
	}
	s.Write([]byte{3}) // ^C
	s.Write([]byte("exit 7\n"))
	if _, code := collect(t, c, "", 10*time.Second); code == nil || *code != 7 {
		t.Fatalf("exit = %v", code)
	}
	if info := s.Info(); !info.Exited || info.Code != 7 {
		t.Fatalf("info = %+v", info)
	}
	// A page that attaches afterwards learns how it ended.
	late, attachment := s.Attach()
	if _, code := collect(t, late, "", time.Second); code == nil || *code != 7 || !strings.Contains(string(attachment.Replay), "kou-42") {
		t.Fatalf("late exit = %v, replay %q", code, attachment.Replay)
	}
}

func TestKillHangsUp(t *testing.T) {
	m := cleanManager(t)
	s, err := m.Start(Spec{Dir: t.TempDir(), Shell: "/bin/sh"})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := s.Attach()
	if err := m.Kill(s.ID()); err != nil {
		t.Fatal(err)
	}
	if _, code := collect(t, c, "", 10*time.Second); code == nil {
		t.Fatal("no exit")
	}
	if err := m.Kill("nope"); err != ErrNotFound {
		t.Fatalf("kill unknown = %v", err)
	}
}

func TestHandoverClearsCloseOnExecAndResumeSetsItAgain(t *testing.T) {
	m := cleanManager(t)
	s, err := m.Start(Spec{Workspace: "w", Dir: t.TempDir(), Shell: "/bin/sh"})
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	c, _ := s.Attach()
	s.Write([]byte("printf '\\033]0;hand%s\\007' ed; echo be$((1))fore\n"))
	collect(t, c, "be1fore", 10*time.Second)
	path, err := m.Handover()
	if err != nil || path == "" {
		t.Fatalf("handover = %q, %v", path, err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var records []handedOver
	if err := json.Unmarshal(data, &records); err != nil || len(records) != 1 {
		t.Fatalf("records = %v, %v", records, err)
	}
	record := records[0]
	if record.ID != s.ID() || record.PID <= 0 || record.Title != "handed" || !bytes.Contains(record.Output, []byte("be1fore")) {
		t.Fatalf("record = %+v", record)
	}
	if flags, err := unix.FcntlInt(uintptr(record.FD), unix.F_GETFD, 0); err != nil || flags&unix.FD_CLOEXEC != 0 {
		t.Fatalf("flags = %d, %v", flags, err)
	}
	m.Resume(path)
	if flags, err := unix.FcntlInt(uintptr(record.FD), unix.F_GETFD, 0); err != nil || flags&unix.FD_CLOEXEC == 0 {
		t.Fatalf("after resume flags = %d, %v", flags, err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("the file stays: %v", err)
	}
}

func TestAdoptTakesUpAShellHandedOver(t *testing.T) {
	master, tty, err := pty.Open()
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("/bin/sh")
	cmd.Stdin, cmd.Stdout, cmd.Stderr = tty, tty, tty
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true, Setctty: true}
	cmd.Env = []string{"PS1=$ ", "PATH=/bin:/usr/bin"}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	tty.Close()
	// The record has a descriptor of its own, as one inherited would be.
	fd, err := unix.Dup(int(master.Fd()))
	if err != nil {
		t.Fatal(err)
	}
	master.Close()
	record := handedOver{
		ID: "adopted", Workspace: "w", Shell: "/bin/sh", Started: time.Now(), PID: cmd.Process.Pid, FD: fd,
		Cols: 80, Rows: 24, Title: "kept title", Dir: "/tmp", Modes: []int{2004}, Output: []byte("what it printed before\r\n"),
	}
	data, _ := json.Marshal([]handedOver{record})
	path := filepath.Join(t.TempDir(), "handover.json")
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	m := NewManager(nil, "test")
	defer m.Close()
	if n, err := m.Adopt(path); n != 1 || err != nil {
		t.Fatalf("adopt = %d, %v", n, err)
	}
	s, err := m.Get("adopted")
	if err != nil {
		t.Fatal(err)
	}
	c, attachment := s.Attach()
	replay := string(attachment.Replay)
	if !strings.HasPrefix(replay, "\x1b[?2004h") || !strings.Contains(replay, "what it printed before") || attachment.Info.Title != "kept title" {
		t.Fatalf("replay = %q, info %+v", replay, attachment.Info)
	}
	s.Write([]byte("echo taken-$((1+1))\n"))
	collect(t, c, "taken-2", 10*time.Second)
	s.Write([]byte("exit 3\n"))
	if _, code := collect(t, c, "", 10*time.Second); code == nil || *code != 3 {
		t.Fatalf("exit = %v", code)
	}
}

func TestCloseAfterWaitsAndKeepKeeps(t *testing.T) {
	m := cleanManager(t)
	defer m.Close()
	kept, err := m.Start(Spec{Dir: t.TempDir(), Shell: "/bin/sh"})
	if err != nil {
		t.Fatal(err)
	}
	ended, err := m.Start(Spec{Dir: t.TempDir(), Shell: "/bin/sh"})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := ended.Attach()
	for _, s := range []*Session{kept, ended} {
		if err := m.CloseAfter(s.ID(), 300*time.Millisecond); err != nil {
			t.Fatal(err)
		}
	}
	if kept.Info().ClosingAt.IsZero() {
		t.Fatal("no closing time")
	}
	if err := m.Keep(kept.ID()); err != nil {
		t.Fatal(err)
	}
	if _, code := collect(t, c, "", 5*time.Second); code == nil {
		t.Fatal("the shell closed did not end")
	}
	time.Sleep(200 * time.Millisecond)
	if info := kept.Info(); info.Exited || !info.ClosingAt.IsZero() {
		t.Fatalf("the shell kept = %+v", info)
	}
}
