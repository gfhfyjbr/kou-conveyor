//go:build unix

package terminal

import (
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// shellFiles are the terminal plugin's shell integration files.
func shellFiles() (fs.FS, error) {
	return os.DirFS(filepath.Join("..", "..", "kou-conveyor-web", "plugins", "terminal", "shell")), nil
}

// zshHome makes a home whose startup files note that they ran; with
// relocated, .zshenv moves the others to ~/.config/zsh.
func zshHome(t *testing.T, relocated bool) string {
	t.Helper()
	home := t.TempDir()
	dir := home
	env := "export KOU_E=1\n"
	if relocated {
		dir = filepath.Join(home, ".config", "zsh")
		if err := os.MkdirAll(dir, 0o700); err != nil {
			t.Fatal(err)
		}
		env += "export ZDOTDIR=$HOME/.config/zsh\n"
	}
	files := map[string]string{
		filepath.Join(home, ".zshenv"):  env,
		filepath.Join(dir, ".zprofile"): "export KOU_P=1\n",
		filepath.Join(dir, ".zshrc"): strings.Join([]string{
			"export KOU_R=1",
			"PROMPT='user> '",
			"typeset -A kou_assoc",
			"kou_assoc[x]=y",
			"alias kt='print alias-works'",
			"typeset -gi kou_count=0",
			"kou_hook() { (( kou_count++ )) }",
			"autoload -Uz add-zsh-hook && add-zsh-hook precmd kou_hook",
			"",
		}, "\n"),
		filepath.Join(dir, ".zlogin"): "export KOU_L=1\n",
	}
	for name, content := range files {
		if err := os.WriteFile(name, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return home
}

func zshManager(t *testing.T, home string) *Manager {
	t.Helper()
	m := NewManager(shellFiles, "test")
	m.base = t.TempDir()
	m.environment = func() []string {
		return []string{"HOME=" + home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "USER=" + os.Getenv("USER"), "SHELL=/bin/zsh", "LANG=en_US.UTF-8"}
	}
	t.Cleanup(m.Close)
	return m
}

const probe = `print -r -- "PROBE Z=[${ZDOTDIR-unset}] E=$KOU_E P=$KOU_P R=$KOU_R L=$KOU_L A=${kou_assoc[x]} C=$(( kou_count > 0 )) H=${HISTFILE:t} T=${KOU_CONVEYOR_TERMINAL_THEME-none}"; kt` + "\n"

func TestZshThemeRunsTheUsersFilesAndTakesThePromptOver(t *testing.T) {
	if _, err := exec.LookPath("zsh"); err != nil {
		t.Skip("no zsh")
	}
	for _, relocated := range []bool{false, true} {
		home := zshHome(t, relocated)
		m := zshManager(t, home)
		dir := t.TempDir()
		s, err := m.Start(Spec{Dir: dir, Shell: "/bin/zsh", Theme: true})
		if err != nil {
			t.Fatal(err)
		}
		if !s.Info().Theme {
			t.Fatal("the theme is not on")
		}
		c, _ := s.Attach()
		// The prompt is kou-conveyor's, with its marks.
		out, _ := collect(t, c, "\x1b]133;B\a", 15*time.Second)
		if !strings.Contains(out, "▪") || !strings.Contains(out, "❯") || strings.Contains(out, "user> ") {
			t.Fatalf("prompt = %q", out)
		}
		// The shell said where it is.
		want, _ := filepath.EvalSymlinks(dir)
		if got, _ := filepath.EvalSymlinks(s.Info().Dir); got != want {
			t.Fatalf("dir = %q, want %q (%q)", s.Info().Dir, want, out)
		}
		zdotdir := "unset"
		if relocated {
			zdotdir = filepath.Join(home, ".config", "zsh")
		}
		s.Write([]byte(probe))
		out, _ = collect(t, c, "alias-works", 10*time.Second)
		line := "PROBE Z=[" + zdotdir + "] E=1 P=1 R=1 L=1 A=y C=1 H=.zsh_history T=kou"
		if !strings.Contains(out, line) {
			t.Fatalf("relocated=%v: no %q in %q", relocated, line, out)
		}
		// History goes where the user keeps it, not to kou-conveyor's
		// directory.
		s.Write([]byte("print -r -- \"HIST=${HISTFILE:h}\"\n"))
		out, _ = collect(t, c, "HIST=/", 10*time.Second)
		if strings.Contains(out, "shell-") {
			t.Fatalf("history file in kou-conveyor's directory: %q", out)
		}
	}
}

func TestZshWithoutTheThemeIsTheUsers(t *testing.T) {
	if _, err := exec.LookPath("zsh"); err != nil {
		t.Skip("no zsh")
	}
	home := zshHome(t, false)
	m := zshManager(t, home)
	s, err := m.Start(Spec{Dir: t.TempDir(), Shell: "/bin/zsh"})
	if err != nil {
		t.Fatal(err)
	}
	c, _ := s.Attach()
	collect(t, c, "user> ", 15*time.Second)
	s.Write([]byte(probe))
	out, _ := collect(t, c, "alias-works", 10*time.Second)
	if line := "PROBE Z=[unset] E=1 P=1 R=1 L=1 A=y C=1 H=.zsh_history T=none"; !strings.Contains(out, line) {
		t.Fatalf("no %q in %q", line, out)
	}
}
