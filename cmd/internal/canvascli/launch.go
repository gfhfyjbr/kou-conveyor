package canvascli

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"strings"
)

// kou-canvas launch starts the harness of the terminal node it runs in: the
// node's shell types it at its first prompt, and it asks the server how —
// the program and its arguments, the files its preset writes, its
// variables — so that none of it, the node's token least of all, shows in
// the shell's history or in a process's arguments. The program replaces
// kou-canvas; when it ends, the node's shell is back.

type launchSpec struct {
	Argv  []string `json:"argv"`
	Env   []string `json:"env"`
	Dir   string   `json:"dir"`
	Title string   `json:"title"`
}

func (a *app) launch(ctx context.Context, args []string) int {
	p, err := parseFlags(args, map[string]flagKind{"resume": boolFlag})
	if err == nil && len(p.args) != 0 {
		err = errors.New("usage: kou-canvas launch [--resume]")
	}
	if err != nil {
		fmt.Fprintln(a.err, "kou-canvas launch:", err)
		return 2
	}
	c, err := a.canvasClient()
	if err != nil {
		fmt.Fprintln(a.err, "kou-canvas:", err)
		return 1
	}
	q := url.Values{}
	if p.bool("resume") {
		q.Set("resume", "1")
	}
	var spec launchSpec
	if _, err := c.call(ctx, "GET", "/api/canvas/self/launch", q, nil, &spec); err != nil {
		fmt.Fprintln(a.err, "kou-canvas launch:", err)
		return 1
	}
	if len(spec.Argv) == 0 || spec.Argv[0] == "" {
		fmt.Fprintln(a.err, "kou-canvas launch: the node runs no program")
		return 1
	}
	for _, variable := range spec.Env {
		if name, value, ok := strings.Cut(variable, "="); ok && name != "" {
			os.Setenv(name, value)
		}
	}
	path, err := exec.LookPath(spec.Argv[0])
	if err != nil {
		fmt.Fprintf(a.err, "kou-canvas launch: %s is not installed, or not on the PATH: %v\n", spec.Argv[0], err)
		return 127
	}
	if spec.Dir != "" {
		if err := os.Chdir(spec.Dir); err != nil {
			fmt.Fprintln(a.err, "kou-canvas launch:", err)
			return 1
		}
	}
	if title := strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f || r >= 0x80 && r < 0xa0 {
			return -1
		}
		return r
	}, spec.Title); title != "" {
		fmt.Fprintf(a.out, "\x1b]2;%s\x07", title)
	}
	return execProgram(a, path, spec.Argv)
}
