package terminal

import (
	"bufio"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"os/user"
	"path"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
)

// Shells start as login shells, as a terminal application starts them,
// with the environment of the server. With the kou-conveyor theme on, a
// shell kou-conveyor knows — zsh, bash, fish — starts through startup files
// of kou-conveyor's own, which run the user's own files unchanged and then
// give the shell its integration (the working directory and title it
// reports, the marks around its prompts) and the kou-conveyor prompt. The
// user's files are never written:
//
//   - zsh starts with ZDOTDIR pointing at kou-conveyor's directory, whose
//     .zshenv, .zprofile, .zshrc and .zlogin each source the user's file of
//     that name with ZDOTDIR as the user has it; after the user's .zshrc the
//     prompt is kou-conveyor's, and after the last file ZDOTDIR is the
//     user's again, for the shells started from this one.
//   - bash starts with --rcfile kou-conveyor's file, which reads the files a
//     login shell reads, then sets the prompt.
//   - fish starts as a login shell with --init-command, which runs after
//     the user's config.fish and sets the prompt.
//
// The files are the terminal plugin's shell/ directory, written to the
// user's cache directory by their content, so a shell that runs keeps the
// version it started with.

// Integration is where the shell integration files are on disk.
type Integration struct {
	Dir string
}

// Shell kinds with an integration.
const (
	shellZsh  = "zsh"
	shellBash = "bash"
	shellFish = "fish"
)

// kindOf says which shell a program is, by its name.
func kindOf(shell string) string {
	name := strings.TrimPrefix(filepath.Base(shell), "-")
	switch {
	case name == "zsh" || strings.HasPrefix(name, "zsh-") || strings.HasPrefix(name, "zsh5"):
		return shellZsh
	case name == "bash" || strings.HasPrefix(name, "bash-") || strings.HasPrefix(name, "bash5"):
		return shellBash
	case name == "fish":
		return shellFish
	}
	return ""
}

// UserShell is the shell the user logs in with: $SHELL, else the account's,
// else the system's.
func UserShell() string {
	if shell := os.Getenv("SHELL"); filepath.IsAbs(shell) && executable(shell) {
		return shell
	}
	if shell := accountShell(); shell != "" && executable(shell) {
		return shell
	}
	for _, shell := range []string{"/bin/zsh", "/bin/bash", "/bin/sh"} {
		if (runtime.GOOS == "darwin" || shell != "/bin/zsh") && executable(shell) {
			return shell
		}
	}
	return "/bin/sh"
}

func executable(file string) bool {
	info, err := os.Stat(file)
	return err == nil && !info.IsDir() && info.Mode()&0o111 != 0
}

// accountShell reads the user's shell from the account database.
func accountShell() string {
	current, err := user.Current()
	if err != nil {
		return ""
	}
	if runtime.GOOS == "darwin" {
		out, err := exec.Command("/usr/bin/dscl", ".", "-read", "/Users/"+current.Username, "UserShell").Output()
		if err != nil {
			return ""
		}
		_, shell, _ := strings.Cut(strings.TrimSpace(string(out)), ":")
		return strings.TrimSpace(shell)
	}
	file, err := os.Open("/etc/passwd")
	if err != nil {
		return ""
	}
	defer file.Close()
	lines := bufio.NewScanner(file)
	for lines.Scan() {
		fields := strings.Split(lines.Text(), ":")
		if len(fields) >= 7 && fields[0] == current.Username {
			return fields[6]
		}
	}
	return ""
}

// dotted are the zsh startup files, which are hidden files in the
// directory zsh reads them from.
var dotted = []string{"zshenv", "zprofile", "zshrc", "zlogin", "zlogout"}

// materialize writes the shell integration files of files to a directory
// named by their content, under base, and returns it. A directory that
// holds them already is taken as it is.
func materialize(files fs.FS, base string) (string, error) {
	digest := sha256.New()
	type file struct {
		name string
		data []byte
	}
	var all []file
	err := fs.WalkDir(files, ".", func(name string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}
		data, err := fs.ReadFile(files, name)
		if err != nil {
			return err
		}
		dir, base := path.Split(name)
		if slices.Contains(dotted, base) {
			name = dir + "." + base
		}
		all = append(all, file{name, data})
		fmt.Fprintf(digest, "%s %d\n", name, len(data))
		digest.Write(data)
		return nil
	})
	if err != nil {
		return "", err
	}
	if len(all) == 0 {
		return "", errors.New("no shell integration files")
	}
	dir := filepath.Join(base, "shell-"+hex.EncodeToString(digest.Sum(nil))[:12])
	if info, err := os.Stat(dir); err == nil && info.IsDir() {
		return dir, nil
	}
	if err := os.MkdirAll(base, 0o700); err != nil {
		return "", err
	}
	temporary, err := os.MkdirTemp(base, ".shell-")
	if err != nil {
		return "", err
	}
	for _, f := range all {
		target := filepath.Join(temporary, filepath.FromSlash(f.name))
		if err := os.MkdirAll(filepath.Dir(target), 0o700); err != nil {
			os.RemoveAll(temporary)
			return "", err
		}
		if err := os.WriteFile(target, f.data, 0o600); err != nil {
			os.RemoveAll(temporary)
			return "", err
		}
	}
	if err := os.Rename(temporary, dir); err != nil {
		os.RemoveAll(temporary)
		// Another server wrote the same files first.
		if info, statErr := os.Stat(dir); statErr == nil && info.IsDir() {
			return dir, nil
		}
		return "", err
	}
	return dir, nil
}

// integrationBase is where integration files are written: the user's cache
// directory, else the temporary one.
func integrationBase() string {
	if dir, err := os.UserCacheDir(); err == nil && dir != "" {
		return filepath.Join(dir, "kou-conveyor")
	}
	return filepath.Join(os.TempDir(), fmt.Sprintf("kou-conveyor-%d", os.Getuid()))
}

// launch is how a shell starts: its program, its arguments (the first is
// its name) and its environment.
type launch struct {
	path string
	args []string
	env  []string
}

// command says how to start shell for spec: a login shell, through the
// integration when the theme is on and integration is set up.
func command(shell string, spec Spec, id, version string, base []string, integration *Integration) launch {
	name := filepath.Base(shell)
	l := launch{path: shell, args: []string{"-" + name}}
	env := shellEnvironment(base, spec.Dir, id, version)
	kind := kindOf(shell)
	if spec.Theme && integration != nil && kind != "" {
		env = append(env, "KOU_CONVEYOR_TERMINAL_THEME=kou")
		switch kind {
		case shellZsh:
			// The user's ZDOTDIR, if they have one, is where kou-conveyor's
			// files find the user's.
			if user, ok := lookup(base, "ZDOTDIR"); ok {
				env = append(env, "KOU_CONVEYOR_USER_ZDOTDIR="+user)
			}
			env = append(env, "ZDOTDIR="+filepath.Join(integration.Dir, "zsh"))
		case shellBash:
			// A login shell reads no --rcfile: the file reads what a login
			// shell would.
			l.args = []string{name, "--rcfile", filepath.Join(integration.Dir, "bash", "kou.bash"), "-i"}
			env = append(env, "KOU_CONVEYOR_BASH_LOGIN=1")
		case shellFish:
			l.args = append(l.args, "--init-command", "source "+fishQuote(filepath.Join(integration.Dir, "fish", "kou.fish")))
		}
	}
	l.env = env
	return l
}

// fishQuote quotes a path for fish.
func fishQuote(s string) string {
	return "'" + strings.NewReplacer(`\`, `\\`, `'`, `\'`).Replace(s) + "'"
}

// lookup finds a variable in an environment.
func lookup(env []string, name string) (string, bool) {
	for i := len(env) - 1; i >= 0; i-- {
		if value, ok := strings.CutPrefix(env[i], name+"="); ok {
			return value, true
		}
	}
	return "", false
}

// dropped are variables of the server's environment a terminal does not
// pass on: what another terminal, multiplexer or editor said of itself, and
// what a new shell sets anew.
var dropped = []string{
	"TERM=", "COLORTERM=", "TERM_PROGRAM=", "TERM_PROGRAM_VERSION=", "TERM_SESSION_ID=", "TERMINAL_EMULATOR=",
	"ITERM_", "LC_TERMINAL=", "LC_TERMINAL_VERSION=", "KITTY_", "GHOSTTY_", "WEZTERM_", "ALACRITTY_", "KONSOLE_",
	"VTE_VERSION=", "WT_SESSION=", "WT_PROFILE_ID=", "VSCODE_", "TMUX=", "TMUX_PANE=", "STY=", "WINDOW=",
	"INSIDE_EMACS=", "__CFBundleIdentifier=", "SHLVL=", "COLUMNS=", "LINES=", "OLDPWD=", "PWD=", "P9K_", "_P9K_",
	"ZDOTDIR=", "KOU_CONVEYOR_USER_ZDOTDIR=", "KOU_CONVEYOR_TERMINAL", "KOU_CONVEYOR_BASH_LOGIN=", "KOU_CONVEYOR_WEB_",
}

// shellEnvironment is the environment a terminal's shell starts with.
func shellEnvironment(base []string, dir, id, version string) []string {
	env := make([]string, 0, len(base)+8)
	locale := false
	for _, value := range base {
		if slices.ContainsFunc(dropped, func(prefix string) bool { return strings.HasPrefix(value, prefix) }) {
			continue
		}
		if name, v, _ := strings.Cut(value, "="); (name == "LANG" || name == "LC_ALL" || name == "LC_CTYPE") && v != "" {
			locale = true
		}
		env = append(env, value)
	}
	if version == "" {
		version = "dev"
	}
	env = append(env,
		"TERM=xterm-256color", "COLORTERM=truecolor",
		"TERM_PROGRAM=kou-conveyor", "TERM_PROGRAM_VERSION="+version,
		"KOU_CONVEYOR_TERMINAL="+id, "PWD="+dir)
	if !locale {
		if runtime.GOOS == "darwin" {
			env = append(env, "LANG=en_US.UTF-8")
		} else {
			env = append(env, "LANG=C.UTF-8")
		}
	}
	return env
}

// Integrated reports whether kou-conveyor's theme and integration are
// there for a shell: zsh, bash and fish have them.
func Integrated(shell string) bool { return kindOf(shell) != "" }
