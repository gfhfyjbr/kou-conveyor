// Package runconfig holds what the cockpits and the runner agree on about
// a run's configuration beyond the connection: the environment variables
// that carry the tool profile and the sandbox mode, and the sandbox modes.
package runconfig

import (
	"fmt"
	"strings"
)

// The environment variables the cockpits set for a run.
const (
	// ToolProfileEnvironment names the tool profile (tool.Profile).
	ToolProfileEnvironment = "KOU_CONVEYOR_TOOL_PROFILE"
	// SandboxEnvironment names the sandbox mode.
	SandboxEnvironment = "KOU_CONVEYOR_SANDBOX"
	// WorktreeEnvironment carries what the sandbox's worktree gets beside
	// git's files, as the JSON of worktree.Options: untracked paths to copy
	// or link in, and whether to leave the submodules out.
	WorktreeEnvironment = "KOU_CONVEYOR_WORKTREE"
	// WorkdirEnvironment names, out of a sandbox, the folder a run works
	// in instead of its workspace: its tools', its shell's. Its session
	// stays the workspace's.
	WorkdirEnvironment = "KOU_CONVEYOR_WORKDIR"
	// ContextWindowEnvironment sets the model's context window, in tokens
	// (200000, 200k, 1M), over the one the runner knows or assumes.
	ContextWindowEnvironment = "KOU_CONVEYOR_CONTEXT_WINDOW"
)

// Sandbox modes: where a run works.
const (
	SandboxOff       = "off"
	SandboxWorktree  = "worktree"
	SandboxContainer = "container"
)

// SandboxModes lists the modes.
var SandboxModes = []string{SandboxOff, SandboxWorktree, SandboxContainer}

// SandboxDescription says what a mode is, for the cockpits.
func SandboxDescription(mode string) string {
	switch mode {
	case SandboxOff:
		return "In the workspace, on this machine"
	case SandboxWorktree:
		return "A git worktree and branch per session, on this machine"
	case SandboxContainer:
		return "A worktree per session; commands run in the workspace's container"
	}
	return ""
}

// ParseSandboxMode reads a mode; empty is off.
func ParseSandboxMode(value string) (string, error) {
	value = strings.ToLower(strings.TrimSpace(value))
	switch value {
	case "", "0", "false", "no":
		return SandboxOff, nil
	case "1", "true", "yes":
		return SandboxContainer, nil
	case SandboxOff, SandboxWorktree, SandboxContainer:
		return value, nil
	}
	return "", fmt.Errorf("unknown sandbox mode %q; the modes are off, worktree and container", value)
}
