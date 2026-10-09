package agentrunner

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// The environment block. The model works better when it knows where it
// is: the operating system and its shell (GNU or BSD tools, whether
// `timeout` exists), the workspace and its branch, the date, and which
// tools it has for files. The block is short and changes rarely, so it
// costs little and stays cached.

// resolveShell finds the shell the Bash tool runs: bash, which the tool is
// named for and the model writes for, else the user's shell, else sh.
func resolveShell(getenv func(string) string) string {
	if shell := strings.TrimSpace(getenv("KOU_CONVEYOR_SHELL")); shell != "" {
		return shell
	}
	for _, candidate := range []string{"/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash", "/opt/homebrew/bin/bash"} {
		if info, err := os.Stat(candidate); err == nil && !info.IsDir() {
			return candidate
		}
	}
	if path, err := exec.LookPath("bash"); err == nil {
		return path
	}
	if shell := strings.TrimSpace(getenv("SHELL")); shell != "" {
		return shell
	}
	return "/bin/sh"
}

// environmentBlock describes where the agent runs, for the system prompt.
func environmentBlock(workspace, shell string, profile tool.Profile, sandboxed string, now time.Time) string {
	var text strings.Builder
	text.WriteString("## Environment\n")
	fmt.Fprintf(&text, "- OS: %s (%s)", osName(), runtime.GOARCH)
	if runtime.GOOS == "darwin" {
		text.WriteString("; BSD userland: `sed -i ''`, no `timeout`, no GNU `grep -P`; prefer `rg`, `python3` or the file tools for edits")
	}
	text.WriteString("\n")
	fmt.Fprintf(&text, "- Shell for Bash: %s (each call starts a fresh shell in the workspace; cd does not persist)\n", shell)
	fmt.Fprintf(&text, "- Workspace: %s", workspace)
	if branch := gitBranch(workspace); branch != "" {
		fmt.Fprintf(&text, " (git branch %s)", branch)
	}
	text.WriteString("\n")
	if sandboxed != "" {
		fmt.Fprintf(&text, "- Sandbox: %s\n", sandboxed)
	} else {
		text.WriteString("- This is the user's own machine, not a sandbox: commands run for real; do not run destructive git or rm commands unless asked.\n")
	}
	var found []string
	for _, name := range []string{"rg", "fd", "jq", "go", "node", "python3", "uv", "cargo", "docker"} {
		if _, err := exec.LookPath(name); err == nil {
			found = append(found, name)
		}
	}
	if len(found) != 0 {
		fmt.Fprintf(&text, "- On PATH: %s\n", strings.Join(found, ", "))
	}
	fmt.Fprintf(&text, "- Date: %s\n", now.Format("2006-01-02 (Monday)"))
	fmt.Fprintf(&text, "- Tools: %s", profileGuidance(profile))
	return text.String()
}

// profileGuidance says how the tools of a profile are meant to be used.
func profileGuidance(profile tool.Profile) string {
	switch profile {
	case tool.ProfileEdit:
		return "Read files with Read (not cat/sed/head), change them with Edit and Write (not sed -i, python or heredocs), run commands with Bash. Batch independent calls in one turn."
	case tool.ProfilePatch:
		return "Read files with Read (not cat/sed/head), change them with apply_patch (not sed -i, python or heredocs), run commands with Bash. Batch independent calls in one turn."
	case tool.ProfileCode:
		return "You have one tool, Code: write JavaScript that calls the declared functions. Put several steps, loops and Promise.all in one call rather than many small calls; each call is a whole turn."
	case tool.ProfileShell:
		return "Bash only: read files with sed -n or cat -n, write with heredocs (quote the delimiter: <<'EOF'). Batch independent calls in one turn."
	}
	return "Batch independent calls in one turn."
}

func osName() string {
	switch runtime.GOOS {
	case "darwin":
		if out, err := exec.Command("sw_vers", "-productVersion").Output(); err == nil {
			return "macOS " + strings.TrimSpace(string(out))
		}
		return "macOS"
	case "linux":
		if data, err := os.ReadFile("/etc/os-release"); err == nil {
			for _, line := range strings.Split(string(data), "\n") {
				if value, ok := strings.CutPrefix(line, "PRETTY_NAME="); ok {
					return strings.Trim(value, `"`)
				}
			}
		}
		return "Linux"
	}
	return runtime.GOOS
}

// gitBranch is the branch the workspace is on, or "" outside a repository.
func gitBranch(workspace string) string {
	head, err := os.ReadFile(filepath.Join(workspace, ".git", "HEAD"))
	if err != nil {
		// A worktree's .git is a file that points at the repository.
		pointer, readErr := os.ReadFile(filepath.Join(workspace, ".git"))
		if readErr != nil {
			return ""
		}
		directory, ok := strings.CutPrefix(strings.TrimSpace(string(pointer)), "gitdir: ")
		if !ok {
			return ""
		}
		if !filepath.IsAbs(directory) {
			directory = filepath.Join(workspace, directory)
		}
		if head, err = os.ReadFile(filepath.Join(directory, "HEAD")); err != nil {
			return ""
		}
	}
	reference := strings.TrimSpace(string(head))
	if branch, ok := strings.CutPrefix(reference, "ref: refs/heads/"); ok {
		return branch
	}
	if len(reference) >= 7 {
		return "detached at " + reference[:7]
	}
	return ""
}

// The system prompt's norms: what a careful engineer does, which the
// default prompt says nothing about.
const engineeringNorms = `## Working norms
- Read before you change: look at the code around an edit, and at how the project already does similar things.
- Do not guess APIs, flags or file layouts you have not seen; check with a quick command or a read.
- Make the smallest change that solves the task; do not refactor, reformat or add features nobody asked for.
- Verify: build and run the relevant tests after changing code, and fix what you broke. Say what you ran.
- Do not commit, push, reset or delete branches unless asked.
- A turn is expensive (the whole conversation is re-read): batch independent tool calls in one turn, never poll with sleep, and read a file once, not in pieces.
- When you finish, state what was done and what was not, briefly.`

// defaultToolProfile is the profile a run uses: KOU_CONVEYOR_TOOL_PROFILE,
// else the request's, else auto.
func defaultToolProfile(getenv func(string) string, requested string) (tool.Profile, error) {
	if value := strings.TrimSpace(getenv(runconfig.ToolProfileEnvironment)); value != "" && strings.TrimSpace(requested) == "" {
		return tool.ParseProfile(value)
	}
	return tool.ParseProfile(requested)
}

// disallowedNames leaves the names the request disallows out.
func disallowedNames(names []string, disallowed []string) []string {
	return slices.DeleteFunc(slices.Clone(names), func(name string) bool { return slices.Contains(disallowed, name) })
}
