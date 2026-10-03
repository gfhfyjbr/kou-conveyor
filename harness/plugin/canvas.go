package plugin

import (
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strings"
	"time"
)

// Canvas is what a plugin adds to the browser cockpit's canvas: harnesses
// its terminals run, sources of events, and templates of whole canvases.
// The canvas reads it; the harness itself does not.
type Canvas struct {
	// Harnesses are presets of terminal nodes: a program, how it starts
	// and takes input, and how the canvas learns what it does.
	Harnesses []CanvasHarness `json:"harnesses,omitzero"`
	// Sources are nodes that bring events: a program the canvas runs that
	// prints them, or one the canvas has built in (Builtin).
	Sources []CanvasSource `json:"sources,omitzero"`
	// Templates is a directory of canvases as JSON files, without what
	// runs.
	Templates string `json:"templates,omitzero"`
}

// CanvasHarness is a preset of a terminal node. Command and Args are its
// program's argv; in the arguments, the values of files and env, {{brief}}
// is what the node is told of the canvas, {{files.<name>}} the path of a
// file of Files, {{node.title}} and {{node.id}} the node's,
// {{runtime.agent_session}} the program's own session and {{config.<key>}}
// a value of the node's configuration, which Config describes.
type CanvasHarness struct {
	ID          string `json:"id"`
	Title       string `json:"title"`
	Icon        string `json:"icon,omitzero"`
	Description string `json:"description,omitzero"`
	// Command is the program and its fixed arguments.
	Command []string `json:"command,omitzero"`
	Args    []string `json:"args,omitzero"`
	// Files are written for the program before it starts: an object is
	// written as JSON, a string as text.
	Files map[string]jsontext.Value `json:"files,omitzero"`
	Env   map[string]string         `json:"env,omitzero"`
	// Launch is how it starts: launcher (the default for a preset with a
	// command) has the shell run kou-canvas launch, which runs it; type
	// types the command at the first prompt; shell runs only the shell.
	Launch string       `json:"launch,omitzero"`
	Input  *CanvasInput `json:"input,omitzero"`
	// Status lists how the canvas learns whether it works or waits:
	// hooks, notify, osc133, idle.
	Status []string `json:"status,omitzero"`
	// IdleMS is how long without output means it waits; Ready, a pattern
	// its screen shows when it does.
	IdleMS int    `json:"idle_ms,omitzero"`
	Ready  string `json:"ready,omitzero"`
	// Output is where its answer comes from: hooks, notify, osc133,
	// screen or none.
	Output string `json:"output,omitzero"`
	// SessionArg gives the program the session the canvas names for it;
	// Resume, what takes the session up again.
	SessionArg []string `json:"session_arg,omitzero"`
	Resume     []string `json:"resume,omitzero"`
	// Config is the JSON schema of the node's configuration.
	Config map[string]any `json:"config,omitzero"`
	// Check is a command that says whether the program is installed.
	Check      []string `json:"check,omitzero"`
	MinVersion string   `json:"min_version,omitzero"`
}

// CanvasInput says how text is typed into the program.
type CanvasInput struct {
	// Paste is bracketed (as a paste, when the program asked for that) or
	// type.
	Paste string `json:"paste,omitzero"`
	// Newline is what a line break becomes: cr or lf.
	Newline string `json:"newline,omitzero"`
	// Submit is what is typed after the text, SubmitDelayMS later.
	Submit        string `json:"submit,omitzero"`
	SubmitDelayMS int    `json:"submit_delay_ms,omitzero"`
}

// CanvasSource is a node that brings events. Run is the program that
// prints them as lines of JSON, in the workspace; elements that start with
// ./ or ../ are paths in the plugin's directory. In poll mode it runs every
// Interval and exits; in stream mode it runs on.
type CanvasSource struct {
	ID          string   `json:"id"`
	Title       string   `json:"title"`
	Description string   `json:"description,omitzero"`
	Icon        string   `json:"icon,omitzero"`
	Run         []string `json:"run,omitzero"`
	// Builtin is set for the sources the canvas has itself: manual, timer,
	// files, webhook.
	Builtin  bool   `json:"builtin,omitzero"`
	Mode     string `json:"mode,omitzero"`
	Interval string `json:"interval,omitzero"`
	// Config is the JSON schema of the node's configuration.
	Config map[string]any `json:"config,omitzero"`
	// Outputs are its ports, which its events name.
	Outputs  []CanvasPort `json:"outputs,omitzero"`
	Template string       `json:"template,omitzero"`
}

// CanvasPort is an output of a source.
type CanvasPort struct {
	ID    string `json:"id"`
	Title string `json:"title,omitzero"`
}

// Requires says what a plugin needs to run; without it, it is not active.
type Requires struct {
	// Env names environment variables that must be set: the canvas's
	// plugin for agents runs only for an agent on a canvas.
	Env []string `json:"env,omitzero"`
}

// MinSourceInterval bounds how often a polling source runs.
const MinSourceInterval = 10 * time.Second

var (
	canvasID     = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)
	variableName = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
)

// SourceCommand is a source's command with its paths resolved.
func (current Plugin) SourceCommand(source CanvasSource) ([]string, error) {
	command := slices.Clone(source.Run)
	for index, argument := range command {
		if strings.HasPrefix(argument, "./") || strings.HasPrefix(argument, "../") {
			path, err := current.Resolve(argument)
			if err != nil {
				return nil, fmt.Errorf("source %q: %w", source.ID, err)
			}
			command[index] = path
		}
	}
	return command, nil
}

// SourceInterval is how often a polling source runs.
func (source CanvasSource) SourceInterval() time.Duration {
	interval, err := time.ParseDuration(source.Interval)
	if err != nil || interval < MinSourceInterval {
		return time.Minute
	}
	return interval
}

// validateCanvas checks what the plugin adds to the canvas.
func (current Plugin) validateCanvas() []error {
	var problems []error
	if current.Requires != nil {
		for _, name := range current.Requires.Env {
			if !variableName.MatchString(name) {
				problems = append(problems, fmt.Errorf("requires.env: %q is not a variable name", name))
			}
		}
	}
	canvas := current.Canvas
	if canvas == nil {
		return problems
	}
	harnesses := map[string]bool{}
	for index, harness := range canvas.Harnesses {
		where := fmt.Sprintf("canvas.harnesses[%d]", index)
		if harness.ID != "" {
			where = fmt.Sprintf("canvas harness %q", harness.ID)
		}
		switch {
		case !canvasID.MatchString(harness.ID):
			problems = append(problems, fmt.Errorf("%s: id %q must be lowercase letters, digits and dashes", where, harness.ID))
		case harnesses[harness.ID]:
			problems = append(problems, fmt.Errorf("%s is declared twice", where))
		case strings.TrimSpace(harness.Title) == "":
			problems = append(problems, fmt.Errorf("%s needs a title", where))
		}
		harnesses[harness.ID] = true
		switch harness.Launch {
		case "", "launcher", "type", "shell":
		default:
			problems = append(problems, fmt.Errorf("%s: launch must be launcher, type or shell", where))
		}
		if (harness.Launch == "" || harness.Launch == "launcher") && (len(harness.Command) == 0 || strings.TrimSpace(harness.Command[0]) == "") {
			problems = append(problems, fmt.Errorf("%s needs the command it runs", where))
		}
		for _, way := range harness.Status {
			if !slices.Contains([]string{"hooks", "notify", "osc133", "idle"}, way) {
				problems = append(problems, fmt.Errorf("%s: status %q is not hooks, notify, osc133 or idle", where, way))
			}
		}
		switch harness.Output {
		case "", "hooks", "notify", "osc133", "screen", "none":
		default:
			problems = append(problems, fmt.Errorf("%s: output must be hooks, notify, osc133, screen or none", where))
		}
		if harness.Ready != "" {
			if _, err := regexp.Compile(harness.Ready); err != nil {
				problems = append(problems, fmt.Errorf("%s: ready: %w", where, err))
			}
		}
		if harness.IdleMS < 0 || harness.IdleMS > 600000 {
			problems = append(problems, fmt.Errorf("%s: idle_ms must be between 0 and 600000", where))
		}
		if input := harness.Input; input != nil {
			if input.Paste != "" && input.Paste != "bracketed" && input.Paste != "type" {
				problems = append(problems, fmt.Errorf("%s: input.paste must be bracketed or type", where))
			}
			if input.Newline != "" && input.Newline != "cr" && input.Newline != "lf" {
				problems = append(problems, fmt.Errorf("%s: input.newline must be cr or lf", where))
			}
			if input.SubmitDelayMS < 0 || input.SubmitDelayMS > 5000 {
				problems = append(problems, fmt.Errorf("%s: input.submit_delay_ms must be between 0 and 5000", where))
			}
		}
		for _, name := range slices.Sorted(maps.Keys(harness.Files)) {
			value := harness.Files[name]
			if !canvasID.MatchString(name) {
				problems = append(problems, fmt.Errorf("%s: file %q must be named with lowercase letters, digits and dashes", where, name))
			}
			if kind := value.Kind(); kind != '{' && kind != '[' && kind != '"' {
				problems = append(problems, fmt.Errorf("%s: file %q must be a JSON object, an array or a string", where, name))
			}
		}
		for name := range harness.Env {
			if !variableName.MatchString(name) {
				problems = append(problems, fmt.Errorf("%s: env: %q is not a variable name", where, name))
			}
		}
		if err := validateSchema(harness.Config); err != nil {
			problems = append(problems, fmt.Errorf("%s: config: %w", where, err))
		}
	}
	sources := map[string]bool{}
	for index, source := range canvas.Sources {
		where := fmt.Sprintf("canvas.sources[%d]", index)
		if source.ID != "" {
			where = fmt.Sprintf("canvas source %q", source.ID)
		}
		switch {
		case !canvasID.MatchString(source.ID):
			problems = append(problems, fmt.Errorf("%s: id %q must be lowercase letters, digits and dashes", where, source.ID))
		case sources[source.ID]:
			problems = append(problems, fmt.Errorf("%s is declared twice", where))
		case strings.TrimSpace(source.Title) == "":
			problems = append(problems, fmt.Errorf("%s needs a title", where))
		case !source.Builtin && (len(source.Run) == 0 || strings.TrimSpace(source.Run[0]) == ""):
			problems = append(problems, fmt.Errorf("%s needs the command it runs", where))
		case len(source.Outputs) == 0:
			problems = append(problems, fmt.Errorf("%s needs its outputs", where))
		}
		sources[source.ID] = true
		switch source.Mode {
		case "", "poll", "stream":
		default:
			problems = append(problems, fmt.Errorf("%s: mode must be poll or stream", where))
		}
		if source.Interval != "" {
			interval, err := time.ParseDuration(source.Interval)
			switch {
			case err != nil:
				problems = append(problems, fmt.Errorf("%s: interval: %w", where, err))
			case interval < MinSourceInterval:
				problems = append(problems, fmt.Errorf("%s: interval must be %s or longer", where, MinSourceInterval))
			}
		}
		ports := map[string]bool{}
		for _, port := range source.Outputs {
			if !canvasID.MatchString(port.ID) || ports[port.ID] {
				problems = append(problems, fmt.Errorf("%s: output %q must be lowercase letters, digits and dashes, and named once", where, port.ID))
			}
			ports[port.ID] = true
		}
		if !source.Builtin {
			if _, err := current.SourceCommand(source); err != nil {
				problems = append(problems, err)
			}
		}
		if err := validateSchema(source.Config); err != nil {
			problems = append(problems, fmt.Errorf("%s: config: %w", where, err))
		}
	}
	if canvas.Templates != "" {
		info, err := current.Stat(canvas.Templates)
		switch {
		case err != nil:
			problems = append(problems, fmt.Errorf("canvas.templates: %w", err))
		case !info.IsDir():
			problems = append(problems, fmt.Errorf("canvas.templates: %s is not a directory", canvas.Templates))
		}
	}
	return problems
}

// validateSchema checks a configuration's JSON schema against what the
// canvas's forms show: an object of strings, numbers, integers, booleans,
// and arrays of strings, with enum, default, title, description, pattern
// and required.
func validateSchema(schema map[string]any) error {
	if schema == nil {
		return nil
	}
	if schema["type"] != "object" {
		return errors.New(`the schema must be of "type": "object"`)
	}
	properties, _ := schema["properties"].(map[string]any)
	if raw, ok := schema["properties"]; ok && properties == nil && raw != nil {
		return errors.New("properties must be an object")
	}
	var problems []error
	for _, name := range slices.Sorted(maps.Keys(properties)) {
		property, ok := properties[name].(map[string]any)
		if !ok {
			problems = append(problems, fmt.Errorf("%s must be an object", name))
			continue
		}
		kind, _ := property["type"].(string)
		switch kind {
		case "string", "number", "integer", "boolean":
		case "array":
			items, _ := property["items"].(map[string]any)
			if items == nil || items["type"] != "string" {
				problems = append(problems, fmt.Errorf(`%s: an array must be of "type": "string" items`, name))
			}
		default:
			if _, isEnum := property["enum"]; !isEnum || kind != "" {
				problems = append(problems, fmt.Errorf("%s: type %q is not one the canvas's forms show", name, kind))
			}
		}
		if enum, ok := property["enum"]; ok {
			if list, ok := enum.([]any); !ok || len(list) == 0 {
				problems = append(problems, fmt.Errorf("%s: enum must be a list", name))
			}
		}
		if pattern, ok := property["pattern"].(string); ok {
			if _, err := regexp.Compile(pattern); err != nil {
				problems = append(problems, fmt.Errorf("%s: pattern: %w", name, err))
			}
		}
	}
	if required, ok := schema["required"]; ok {
		list, isList := required.([]any)
		if !isList {
			problems = append(problems, errors.New("required must be a list"))
		}
		for _, item := range list {
			name, _ := item.(string)
			if _, declared := properties[name]; !declared {
				problems = append(problems, fmt.Errorf("required: %v is not a property", item))
			}
		}
	}
	return errors.Join(problems...)
}

// missingEnv names the variables of Requires.Env that getenv has no value
// for.
func (current Plugin) missingEnv(getenv func(string) string) []string {
	if current.Requires == nil {
		return nil
	}
	var missing []string
	for _, name := range current.Requires.Env {
		if strings.TrimSpace(getenv(name)) == "" {
			missing = append(missing, name)
		}
	}
	return missing
}

// requiresReason says why a plugin that needs variables does not run.
func requiresReason(missing []string) string {
	if slices.Contains(missing, "KOU_CANVAS_TOKEN") {
		return "only agents on a canvas have it"
	}
	return "it needs " + strings.Join(missing, ", ") + " set"
}
