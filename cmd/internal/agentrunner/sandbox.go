package agentrunner

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

// The sandbox. A run can work apart from the user's checkout, and apart
// from the user's machine:
//
//   - worktree: the session gets a git worktree of its own, on a branch
//     of its own (kou/<session>), in .harness/worktrees/<session> of the
//     workspace. Commands and file tools work there; the user's checkout
//     stays as it is, and the branch holds what the agent did.
//   - container: the worktree, and the commands run in a container
//     (docker, which OrbStack provides too) with the workspace mounted at
//     its own path, so paths mean the same on both sides. The container
//     is one per workspace, kept between runs, and made from the image
//     the workspace names: the "sandbox" of a plugin.json, a Dockerfile in
//     .harness/sandbox, or KOU_CONVEYOR_SANDBOX_IMAGE.
//
// KOU_CONVEYOR_SANDBOX selects the mode (off, worktree, container), as
// does the request's "sandbox".

const (
	sandboxImageEnvironment = "KOU_CONVEYOR_SANDBOX_IMAGE"

	SandboxOff       = runconfig.SandboxOff
	SandboxWorktree  = runconfig.SandboxWorktree
	SandboxContainer = runconfig.SandboxContainer

	worktreesDirectory = ".harness/worktrees"
	sandboxDockerfile  = ".harness/sandbox/Dockerfile"
	// containerStart bounds starting a container and its setup.
	containerStart = 10 * time.Minute
)

// ParseSandboxMode reads a mode; empty is off.
func ParseSandboxMode(value string) (string, error) { return runconfig.ParseSandboxMode(value) }

// sandbox is what a run works in.
type sandbox struct {
	Mode string
	// Workspace is where commands and file tools work: the worktree, or
	// the workspace itself.
	Workspace string
	// Branch is the worktree's branch.
	Branch string
	// Shell runs the Bash tool's commands: a wrapper that runs them in the
	// container, or the shell itself.
	Shell string
	// Container is the container's name and Image what it runs.
	Container, Image string
}

// Description is what the model reads of the sandbox.
func (current sandbox) Description() string {
	switch current.Mode {
	case SandboxWorktree:
		return fmt.Sprintf("you work in a git worktree of the repository on branch %s (%s); the user's checkout is elsewhere and stays untouched. Commit your work on this branch when it is ready; the user merges it.", current.Branch, current.Workspace)
	case SandboxContainer:
		return fmt.Sprintf("commands run in a container (image %s) with the workspace mounted at the same path; files you read and edit are the same on both sides. You work in a git worktree on branch %s (%s); the user's checkout stays untouched. Commit your work on this branch when it is ready; the user merges it.", current.Image, current.Branch, current.Workspace)
	}
	return ""
}

// prepareSandbox sets the run's sandbox up: the worktree, and the
// container when asked, whose shell wrapper goes in the operation
// directory.
func prepareSandbox(ctx context.Context, mode, workspace, shell, operations string, id session.ID, getenv func(string) string, found plugin.Found, report io.Writer) (sandbox, error) {
	current := sandbox{Mode: mode, Workspace: workspace, Shell: shell}
	if mode == SandboxOff {
		return current, nil
	}
	worktree, branch, err := ensureWorktree(ctx, workspace, id, report)
	if err != nil {
		return sandbox{}, err
	}
	current.Workspace, current.Branch = worktree, branch
	if mode == SandboxWorktree {
		return current, nil
	}
	image, err := sandboxImage(ctx, workspace, getenv, found, report)
	if err != nil {
		return sandbox{}, err
	}
	container, err := ensureContainer(ctx, workspace, image, sandboxConfig(found), report)
	if err != nil {
		return sandbox{}, err
	}
	wrapper := filepath.Join(operations, "sandbox-shell")
	script := fmt.Sprintf("#!/bin/sh\n# Runs the Bash tool's commands in the sandbox container.\nexec docker exec -i -w %s %s /bin/bash \"$@\"\n", shellQuote(worktree), shellQuote(container))
	if err := os.WriteFile(wrapper, []byte(script), 0o700); err != nil {
		return sandbox{}, fmt.Errorf("write the sandbox shell: %w", err)
	}
	current.Shell, current.Container, current.Image = wrapper, container, image
	return current, nil
}

// ensureWorktree makes the session's worktree, or finds it again.
func ensureWorktree(ctx context.Context, workspace string, id session.ID, report io.Writer) (string, string, error) {
	if _, err := os.Stat(filepath.Join(workspace, ".git")); err != nil {
		return "", "", fmt.Errorf("the sandbox needs a git repository; %s has no .git", workspace)
	}
	short := string(id)
	if len(short) > 8 {
		short = short[:8]
	}
	branch := "kou/" + short
	worktree := filepath.Join(workspace, worktreesDirectory, short)
	if _, err := os.Stat(filepath.Join(worktree, ".git")); err == nil {
		return worktree, branch, nil
	}
	if err := os.MkdirAll(filepath.Dir(worktree), 0o755); err != nil {
		return "", "", fmt.Errorf("create the worktrees directory: %w", err)
	}
	git := func(arguments ...string) (string, error) {
		command := exec.CommandContext(ctx, "git", arguments...)
		command.Dir = workspace
		out, err := command.CombinedOutput()
		if err != nil {
			return "", fmt.Errorf("git %s: %v: %s", strings.Join(arguments, " "), err, strings.TrimSpace(string(out)))
		}
		return strings.TrimSpace(string(out)), nil
	}
	// A stale registration of the path, from a worktree removed by hand.
	git("worktree", "prune")
	if _, err := git("rev-parse", "--verify", "--quiet", "refs/heads/"+branch); err == nil {
		if _, err := git("worktree", "add", worktree, branch); err != nil {
			return "", "", err
		}
	} else if _, err := git("worktree", "add", "-b", branch, worktree, "HEAD"); err != nil {
		return "", "", err
	}
	fmt.Fprintf(report, "sandbox> worktree %s on branch %s\n", worktree, branch)
	return worktree, branch, nil
}

// sandboxConfig is the sandbox the active plugins declare, if one does:
// the workspace's own plugin usually.
func sandboxConfig(found plugin.Found) *plugin.Sandbox {
	for _, current := range found.Active() {
		if current.Sandbox != nil {
			return current.Sandbox
		}
	}
	return nil
}

// sandboxImage is the image the container runs: a plugin's, else a
// Dockerfile in .harness/sandbox built into an image of the workspace's
// own, else KOU_CONVEYOR_SANDBOX_IMAGE.
func sandboxImage(ctx context.Context, workspace string, getenv func(string) string, found plugin.Found, report io.Writer) (string, error) {
	if config := sandboxConfig(found); config != nil && strings.TrimSpace(config.Image) != "" {
		return strings.TrimSpace(config.Image), nil
	}
	dockerfile := filepath.Join(workspace, sandboxDockerfile)
	if _, err := os.Stat(dockerfile); err == nil {
		image := "kou-sandbox-" + workspaceHash(workspace)
		fmt.Fprintf(report, "sandbox> building %s from %s\n", image, sandboxDockerfile)
		command := exec.CommandContext(ctx, "docker", "build", "-q", "-t", image, filepath.Dir(dockerfile))
		if out, err := command.CombinedOutput(); err != nil {
			return "", fmt.Errorf("build the sandbox image: %v: %s", err, strings.TrimSpace(string(out)))
		}
		return image, nil
	}
	if image := strings.TrimSpace(getenv(sandboxImageEnvironment)); image != "" {
		return image, nil
	}
	return "", fmt.Errorf("the container sandbox needs an image: name one in a plugin's \"sandbox\": {\"image\": ...}, put a Dockerfile in %s, or set %s", sandboxDockerfile, sandboxImageEnvironment)
}

func workspaceHash(workspace string) string {
	sum := sha256.Sum256([]byte(workspace))
	return fmt.Sprintf("%x", sum[:6])
}

// ensureContainer starts the workspace's container, or finds it running:
// the workspace mounted at its own path, and the setup commands run once.
func ensureContainer(ctx context.Context, workspace, image string, config *plugin.Sandbox, report io.Writer) (string, error) {
	if _, err := exec.LookPath("docker"); err != nil {
		return "", errors.New("the container sandbox needs docker on PATH (Docker or OrbStack)")
	}
	name := "kou-sandbox-" + workspaceHash(workspace)
	startContext, cancel := context.WithTimeout(ctx, containerStart)
	defer cancel()
	docker := func(arguments ...string) (string, error) {
		command := exec.CommandContext(startContext, "docker", arguments...)
		out, err := command.CombinedOutput()
		if err != nil {
			return strings.TrimSpace(string(out)), fmt.Errorf("docker %s: %v: %s", strings.Join(arguments, " "), err, strings.TrimSpace(string(out)))
		}
		return strings.TrimSpace(string(out)), nil
	}
	state, err := docker("inspect", "-f", "{{.State.Running}} {{.Config.Image}}", name)
	if err == nil {
		running, current, _ := strings.Cut(state, " ")
		if current == image {
			if running == "true" {
				return name, nil
			}
			if _, err := docker("start", name); err == nil {
				return name, nil
			}
		}
		// Another image, or one that will not start: made anew.
		docker("rm", "-f", name)
	}
	arguments := []string{"run", "-d", "--name", name, "-v", workspace + ":" + workspace, "-w", workspace, "--init"}
	if config != nil {
		for _, mount := range config.Mounts {
			arguments = append(arguments, "-v", mount)
		}
		for key, value := range config.Environment {
			arguments = append(arguments, "-e", key+"="+value)
		}
	}
	arguments = append(arguments, image, "sleep", "infinity")
	fmt.Fprintf(report, "sandbox> starting container %s from %s\n", name, image)
	if _, err := docker(arguments...); err != nil {
		return "", err
	}
	if config != nil {
		for _, step := range config.Setup {
			fmt.Fprintf(report, "sandbox> setup: %s\n", step)
			if out, err := docker("exec", "-w", workspace, name, "/bin/sh", "-c", step); err != nil {
				docker("rm", "-f", name)
				return "", fmt.Errorf("sandbox setup %q: %v\n%s", step, err, out)
			}
		}
	}
	return name, nil
}
