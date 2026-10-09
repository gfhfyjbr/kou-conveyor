package canvascli

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
)

// The command line's commands are the tools' calls: each makes the
// arguments of a tool of its own.

type flagKind int

const (
	boolFlag flagKind = iota
	valueFlag
	// optionalFlag takes a value when one follows: --worktree [name].
	optionalFlag
)

// parsed is a command's arguments, its flags apart.
type parsed struct {
	args   []string
	values map[string]string
	set    map[string]bool
}

func (p parsed) bool(name string) bool { return p.set[name] }

func (p parsed) value(name string) string { return p.values[name] }

// number reads a flag's number, nil when it is not given.
func (p parsed) number(name string) (number, error) {
	value, ok := p.values[name]
	if !ok {
		return nil, nil
	}
	n, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
	if err != nil {
		return nil, fmt.Errorf("--%s is a number", name)
	}
	return &n, nil
}

// isNumber reports whether an argument is a number, such as -100: not a
// flag.
func isNumber(arg string) bool {
	_, err := strconv.ParseFloat(arg, 64)
	return err == nil
}

// parseFlags reads a command's arguments: flags (--name, --name value,
// --name=value) of spec, and --json, wherever they are.
func parseFlags(args []string, spec map[string]flagKind) (parsed, error) {
	p := parsed{values: map[string]string{}, set: map[string]bool{}}
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			p.args = append(p.args, args[i+1:]...)
			break
		}
		if !strings.HasPrefix(arg, "-") || arg == "-" || isNumber(arg) {
			p.args = append(p.args, arg)
			continue
		}
		name := strings.TrimLeft(arg, "-")
		value, hasValue := "", false
		if key, v, ok := strings.Cut(name, "="); ok {
			name, value, hasValue = key, v, true
		}
		kind, ok := spec[name]
		if name == "json" {
			kind, ok = boolFlag, true
		}
		if !ok {
			return p, fmt.Errorf("no flag --%s", name)
		}
		switch kind {
		case boolFlag:
			if hasValue {
				return p, fmt.Errorf("--%s takes no value", name)
			}
		case valueFlag:
			if !hasValue {
				if i+1 >= len(args) {
					return p, fmt.Errorf("--%s needs a value", name)
				}
				i++
				value = args[i]
			}
			p.values[name] = value
		case optionalFlag:
			if !hasValue && i+1 < len(args) && !strings.HasPrefix(args[i+1], "-") && len(p.args) > 0 {
				i++
				value = args[i]
			}
			p.values[name] = value
		}
		p.set[name] = true
	}
	return p, nil
}

type command struct {
	flags map[string]flagKind
	run   func(ctx context.Context, c *client, p parsed) (result, error)
}

// args checks a command's arguments: at least least, at most most (-1 for
// any).
func (p parsed) need(least, most int, what string) error {
	if len(p.args) < least || most >= 0 && len(p.args) > most {
		return errors.New("usage: " + what)
	}
	return nil
}

var commands = map[string]command{
	"self": {
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(0, 0, "kou-canvas self"); err != nil {
				return result{}, err
			}
			return c.self(ctx)
		},
	},
	"view": {
		flags: map[string]flagKind{"full": boolFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(0, 0, "kou-canvas view [--full] [--json]"); err != nil {
				return result{}, err
			}
			a := viewArgs{}
			if p.bool("full") {
				a.Detail = "full"
			}
			return c.view(ctx, a)
		},
	},
	"spawn": {
		flags: map[string]flagKind{
			"title": valueFlag, "cmd": valueFlag, "command": valueFlag, "cwd": valueFlag, "worktree": optionalFlag, "base": valueFlag,
			"prompt": valueFlag, "config": valueFlag, "access": valueFlag, "near": valueFlag, "side": valueFlag,
			"right-of": valueFlag, "below": valueFlag, "left-of": valueFlag, "above": valueFlag,
			"x": valueFlag, "y": valueFlag, "w": valueFlag, "h": valueFlag,
			"connect-to": valueFlag, "connect-from": valueFlag,
		},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 1, "kou-canvas spawn <preset> [--title T] [--cmd C] [--worktree [name]] [--prompt P] [--right-of N] …"); err != nil {
				return result{}, err
			}
			a := spawnArgs{
				Preset: p.args[0], Title: p.value("title"), Command: p.value("cmd"), Cwd: p.value("cwd"),
				Base: p.value("base"), Prompt: p.value("prompt"), Access: p.value("access"), Near: p.value("near"), Side: p.value("side"),
			}
			if a.Command == "" {
				a.Command = p.value("command")
			}
			if p.bool("worktree") {
				name := p.value("worktree")
				if name == "" {
					name = "true"
				}
				encoded, _ := jsontext.AppendQuote(nil, name)
				a.Worktree = encoded
			}
			if config := p.value("config"); config != "" {
				a.Config = jsontext.Value(config)
			}
			for flag, side := range map[string]string{"right-of": "right", "below": "below", "left-of": "left", "above": "above"} {
				if p.bool(flag) {
					a.Near, a.Side = p.value(flag), side
				}
			}
			var err error
			for _, field := range []struct {
				name string
				to   *number
			}{{"x", &a.X}, {"y", &a.Y}, {"w", &a.W}, {"h", &a.H}} {
				if *field.to, err = p.number(field.name); err != nil {
					return result{}, err
				}
			}
			if p.bool("connect-to") || p.bool("connect-from") {
				a.Connect = &struct {
					From string `json:"from"`
					To   string `json:"to"`
				}{From: p.value("connect-from"), To: p.value("connect-to")}
			}
			return c.spawn(ctx, a)
		},
	},
	"rm": {
		flags: map[string]flagKind{"keep-session": boolFlag, "keep-worktree": boolFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 1, "kou-canvas rm <node> [--keep-session]"); err != nil {
				return result{}, err
			}
			return c.remove(ctx, removeArgs{Node: p.args[0], KeepSession: p.bool("keep-session"), KeepWorktree: true})
		},
	},
	"connect": {
		flags: map[string]flagKind{"template": valueFlag, "approve": boolFlag, "now": boolFlag, "mode": valueFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(2, 2, "kou-canvas connect <node>[:port] <node>[:port] [--template T] [--approve] [--now]"); err != nil {
				return result{}, err
			}
			a := connectArgs{From: p.args[0], To: p.args[1], Template: p.value("template"), Mode: p.value("mode")}
			if p.bool("approve") {
				a.Mode = "approve"
			}
			if p.bool("now") {
				a.Deliver = "now"
			}
			return c.connect(ctx, a)
		},
	},
	"disconnect": {
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 2, "kou-canvas disconnect <edge> | <node>[:port] <node>[:port]"); err != nil {
				return result{}, err
			}
			if len(p.args) == 1 {
				return c.disconnect(ctx, disconnectArgs{Edge: p.args[0]})
			}
			return c.disconnect(ctx, disconnectArgs{From: p.args[0], To: p.args[1]})
		},
	},
	"move": {
		flags: map[string]flagKind{"x": valueFlag, "y": valueFlag, "w": valueFlag, "h": valueFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 1, "kou-canvas move <node> [--x X] [--y Y] [--w W] [--h H]"); err != nil {
				return result{}, err
			}
			a := moveArgs{Node: p.args[0]}
			var err error
			for _, field := range []struct {
				name string
				to   *number
			}{{"x", &a.X}, {"y", &a.Y}, {"w", &a.W}, {"h", &a.H}} {
				if *field.to, err = p.number(field.name); err != nil {
					return result{}, err
				}
			}
			return c.move(ctx, a)
		},
	},
	"send": {
		flags: map[string]flagKind{"no-enter": boolFlag, "now": boolFlag, "wait": boolFlag, "timeout": valueFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(2, -1, `kou-canvas send <node> "text" [--wait [--timeout 600]] [--no-enter] [--now]`); err != nil {
				return result{}, err
			}
			a := sendArgs{Node: p.args[0], Text: strings.Join(p.args[1:], " "), Wait: loose(p.bool("wait"))}
			if p.bool("no-enter") {
				submit := false
				a.Submit = &submit
			}
			if p.bool("now") {
				a.When = "now"
			}
			if p.bool("timeout") {
				seconds, err := parseSeconds(p.value("timeout"))
				if err != nil {
					return result{}, err
				}
				a.TimeoutS = &seconds
			}
			return c.send(ctx, a)
		},
	},
	"keys": {
		flags: map[string]flagKind{"now": boolFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(2, -1, "kou-canvas keys <node> C-c Escape Up Enter …"); err != nil {
				return result{}, err
			}
			submit := false
			a := sendArgs{Node: p.args[0], Keys: p.args[1:], Submit: &submit, When: "now"}
			return c.send(ctx, a)
		},
	},
	"read": {
		flags: map[string]flagKind{
			"screen": boolFlag, "tail": valueFlag, "output": boolFlag, "answer": boolFlag, "lines": valueFlag,
			"wait": valueFlag, "after": valueFlag, "timeout": valueFlag,
		},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 1, "kou-canvas read <node> [--screen|--tail N|--output|--answer] [--wait idle|output|exit] [--timeout 600]"); err != nil {
				return result{}, err
			}
			a := readArgs{Node: p.args[0], Wait: p.value("wait")}
			switch {
			case p.bool("screen"):
				a.What = "screen"
			case p.bool("output"):
				a.What = "output"
			case p.bool("answer"):
				a.What = "answer"
			case p.bool("tail"):
				a.What = "tail"
			}
			var err error
			if a.Lines, err = p.number("lines"); err != nil {
				return result{}, err
			}
			if p.bool("tail") {
				if a.Lines, err = p.number("tail"); err != nil {
					return result{}, err
				}
			}
			if a.After, err = p.number("after"); err != nil {
				return result{}, err
			}
			if p.bool("timeout") {
				seconds, err := parseSeconds(p.value("timeout"))
				if err != nil {
					return result{}, err
				}
				a.TimeoutS = &seconds
			}
			return c.read(ctx, a)
		},
	},
	"wait": {
		flags: map[string]flagKind{"until": valueFlag, "after": valueFlag, "timeout": valueFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(1, 1, "kou-canvas wait <node> [--until idle|output|exit] [--after N] [--timeout 600]"); err != nil {
				return result{}, err
			}
			timeout := 600 * time.Second
			if p.bool("timeout") {
				seconds, err := parseSeconds(p.value("timeout"))
				if err != nil {
					return result{}, err
				}
				timeout = time.Duration(seconds) * time.Second
			}
			after, err := p.number("after")
			if err != nil {
				return result{}, err
			}
			return c.wait(ctx, p.args[0], p.value("until"), intOf(after), timeout)
		},
	},
	"emit": {
		flags: map[string]flagKind{"port": valueFlag, "title": valueFlag, "data": valueFlag},
		run: func(ctx context.Context, c *client, p parsed) (result, error) {
			if err := p.need(0, -1, `kou-canvas emit [--port out] [--title T] "text" [--data JSON]`); err != nil {
				return result{}, err
			}
			a := emitArgs{Text: strings.Join(p.args, " "), Port: p.value("port"), Title: p.value("title")}
			if data := p.value("data"); data != "" {
				a.Data = jsontext.Value(data)
			}
			return c.emit(ctx, a)
		},
	},
}

// parseSeconds reads a timeout: seconds, or a duration such as 10m.
func parseSeconds(value string) (float64, error) {
	value = strings.TrimSpace(value)
	if seconds, err := strconv.ParseFloat(value, 64); err == nil && seconds > 0 {
		return seconds, nil
	}
	if d, err := time.ParseDuration(value); err == nil && d > 0 {
		return d.Seconds(), nil
	}
	return 0, fmt.Errorf("--timeout is seconds, or a duration such as 10m")
}
