package agentrunner

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Verification. Once the model says it is done with a prompt, the
// workspace's checks run — its build, its tests, its linter — and what
// fails goes back to the model before the run ends: "done" means the checks
// pass. The checks come from the workspace's .harness/verify.json, from
// plugins (a "verify" list in plugin.json, as the manifest's extension
// fields allow), or from KOU_CONVEYOR_VERIFY, a command. They run in the
// workspace with the Bash tool's shell, one after another, and stop at the
// first that fails.

const (
	verifyEnvironment = "KOU_CONVEYOR_VERIFY"
	verifyFile        = ".harness/verify.json"
	// verifyTimeout bounds the checks together, in seconds.
	verifyTimeout = 20 * 60
	// verifyOutputLength bounds what the model reads of a failed check.
	verifyOutputLength = 12_000
)

// verifyConfig is .harness/verify.json: the commands that verify the work.
type verifyConfig struct {
	Commands []string `json:"commands"`
	// Timeout bounds the checks together, in seconds; 0 is the default.
	Timeout float64 `json:"timeout,omitzero"`
}

// verifier runs the workspace's checks as one shell operation.
type verifier struct {
	commands  []string
	shell     string
	workspace string
	base      string
	timeout   float64
}

// newVerifier finds the checks of a run; nil when there are none.
func newVerifier(getenv func(string) string, workspace, shell, base string, found plugin.Found) (*verifier, error) {
	var commands []string
	timeout := 0.0
	if value := strings.TrimSpace(getenv(verifyEnvironment)); value != "" {
		if strings.EqualFold(value, "off") {
			return nil, nil
		}
		commands = append(commands, value)
	}
	data, err := os.ReadFile(filepath.Join(workspace, verifyFile))
	switch {
	case err == nil:
		var config verifyConfig
		if err := json.Unmarshal(data, &config); err != nil {
			return nil, fmt.Errorf("read %s: %w", verifyFile, err)
		}
		for _, command := range config.Commands {
			if command = strings.TrimSpace(command); command != "" {
				commands = append(commands, command)
			}
		}
		timeout = config.Timeout
	case !errors.Is(err, os.ErrNotExist):
		return nil, fmt.Errorf("read %s: %w", verifyFile, err)
	}
	for _, current := range found.Active() {
		for _, command := range current.Verify {
			if command = strings.TrimSpace(command); command != "" {
				commands = append(commands, command)
			}
		}
	}
	if len(commands) == 0 {
		return nil, nil
	}
	if timeout <= 0 {
		timeout = verifyTimeout
	}
	return &verifier{commands: commands, shell: shell, workspace: workspace, base: base, timeout: timeout}, nil
}

// Spec is the shell operation that runs the checks: each says its name
// before it runs, and the first that fails ends the run with its code.
func (current *verifier) Spec() (operation.Spec, bool) {
	if current == nil || len(current.commands) == 0 {
		return operation.Spec{}, false
	}
	var script strings.Builder
	for _, command := range current.commands {
		fmt.Fprintf(&script, "printf '$ %%s\\n' %s\n%s || { code=$?; printf 'FAILED (exit %%d): %%s\\n' \"$code\" %s; exit \"$code\"; }\n",
			shellQuote(command), command, shellQuote(command))
	}
	script.WriteString("printf 'All checks passed.\\n'\n")
	spec, err := operation.NewShellSpec(operation.ShellInput{
		Command: script.String(), Shell: current.shell, Directory: current.workspace, Timeout: current.timeout,
	}, current.base, verifyOutputLength)
	if err != nil {
		return operation.Spec{}, false
	}
	return spec, true
}

// Report reads a finished check.
func (current *verifier) Report(check operation.Operation) (string, bool) {
	state, err := operation.DecodeShellState(check)
	if err != nil {
		return "The verification could not be read: " + err.Error(), false
	}
	switch check.Status {
	case operation.StatusCanceled:
		// A check that did not finish passed nothing.
		return "The verification was canceled before it finished, so the work is not checked: run the checks yourself.", false
	case operation.StatusFailed:
		return "The verification could not run: " + state.TerminalError, false
	}
	if state.Result == nil {
		return "The verification left no result.", false
	}
	if state.Result.ExitCode == 0 && !state.Result.TimedOut {
		return "All checks passed.", true
	}
	var text strings.Builder
	text.WriteString("The workspace's checks ran after you finished, and failed. Fix what they report (or explain why it is expected and unrelated to your change), then run them yourself to confirm.\n")
	if state.Result.TimedOut {
		fmt.Fprintf(&text, "The checks were stopped at their timeout of %.0f seconds.\n", state.Input.Timeout)
	}
	if out := strings.TrimSpace(state.Result.Out); out != "" {
		text.WriteString(out + "\n")
	}
	if errOut := strings.TrimSpace(state.Result.Err); errOut != "" {
		text.WriteString("Stderr:\n" + errOut + "\n")
	}
	return strings.TrimRight(text.String(), "\n"), false
}

func shellQuote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'"
}
