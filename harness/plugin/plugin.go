// Package plugin loads plugins: directories whose plugin.json declares what
// they add to the harness and to its cockpits. A plugin can give the agent
// tools, which run as commands, skills and instructions for the system
// prompt, and give the cockpits slash commands, and scripts and styles that
// the browser cockpit builds its interface from. Everything the project
// adds is a plugin: the harness's own tools are the built-in core plugin,
// and the browser cockpit is nothing but built-in plugins on the same API
// as any other.
//
// Plugins come from three places, later ones replacing earlier ones of the
// same name: built in, the user's configuration directory, and the
// workspace. A workspace's plugins run code from wherever the workspace came
// from, so they load only once the user trusts the workspace. Plugins are
// read again whenever their files change (see Fingerprint), so what runs
// follows them without a restart.
package plugin

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"maps"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

// ManifestName is the file that makes a directory a plugin.
const ManifestName = "plugin.json"

// maxPromptBytes bounds the instructions a plugin adds to the system prompt.
const maxPromptBytes = 64 << 10

// Source says where a plugin comes from.
type Source string

const (
	SourceBuiltin   Source = "builtin"
	SourceUser      Source = "user"
	SourceWorkspace Source = "workspace"
)

// Manifest is a plugin's plugin.json. Paths are relative to the plugin's
// directory and may not leave it.
type Manifest struct {
	// Name identifies the plugin: lowercase letters, digits and dashes.
	Name        string `json:"name"`
	Version     string `json:"version,omitzero"`
	Description string `json:"description,omitzero"`
	// Tools are tools the agent can call.
	Tools []Tool `json:"tools,omitzero"`
	// Skills is a directory of skills, one per subdirectory with a SKILL.md.
	Skills string `json:"skills,omitzero"`
	// Prompt is a file whose text joins the system prompt.
	Prompt string `json:"prompt,omitzero"`
	// Commands are slash commands for the cockpits.
	Commands []Command `json:"commands,omitzero"`
	// Web is what the browser cockpit loads.
	Web *Web `json:"web,omitzero"`
}

// Tool is a tool the agent can call. Run is the command the call runs, in
// the workspace: the arguments arrive on its standard input as a JSON
// object, and what it prints is the result. Elements of Run that start with
// ./ or ../ are paths in the plugin's directory.
type Tool struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	// Parameters is the JSON schema of the arguments, an object.
	Parameters      map[string]any `json:"parameters,omitzero"`
	Run             []string       `json:"run,omitzero"`
	MaxOutputLength int            `json:"max_output_length,omitzero"`
}

// Command is a slash command. Prompt is what it asks the agent: {{args}}
// stands for the text typed after the command, which is otherwise added at
// the end.
type Command struct {
	Name        string `json:"name"`
	Args        string `json:"args,omitzero"`
	Description string `json:"description"`
	Prompt      string `json:"prompt"`
}

// Web is what the browser cockpit loads: an ES module whose default export
// is called with the cockpit's plugin API, and a style sheet. After names
// plugins to load before this one when they are there, so what they provide
// is in place when it starts.
type Web struct {
	Script string   `json:"script,omitzero"`
	Style  string   `json:"style,omitzero"`
	After  []string `json:"after,omitzero"`
}

// Plugin is a plugin as loaded.
type Plugin struct {
	Manifest
	Source Source
	// Directory holds the plugin on disk; it is empty for a plugin compiled
	// into the program.
	Directory string
	// Files are the plugin's files: its directory's, or those compiled in.
	// It is nil for a plugin without files, such as core.
	Files fs.FS
	// Instructions is the text of the prompt file.
	Instructions string
	// Active is set for a plugin that runs; Reason says why another does not.
	Active bool
	Reason string
}

var (
	pluginName  = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,63}$`)
	toolName    = regexp.MustCompile(`^[A-Za-z][A-Za-z0-9_-]{0,63}$`)
	commandName = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)
)

// Expand is the prompt a command sends for the text typed after it.
func (command Command) Expand(args string) string {
	args = strings.TrimSpace(args)
	if strings.Contains(command.Prompt, "{{args}}") {
		return strings.TrimSpace(strings.ReplaceAll(command.Prompt, "{{args}}", args))
	}
	if args == "" {
		return strings.TrimSpace(command.Prompt)
	}
	return strings.TrimSpace(command.Prompt) + "\n\n" + args
}

// Resolve returns the path of a file of the plugin; the path may not leave
// the plugin's directory.
func (current Plugin) Resolve(relative string) (string, error) {
	if current.Directory == "" {
		return "", fmt.Errorf("plugin %q has no files on disk", current.Name)
	}
	clean, err := inside(relative)
	if err != nil {
		return "", err
	}
	return filepath.Join(current.Directory, filepath.FromSlash(clean)), nil
}

// inside cleans a path of the plugin, which may not leave it, to the form
// fs.FS takes.
func inside(relative string) (string, error) {
	clean := path.Clean(filepath.ToSlash(relative))
	if relative == "" || path.IsAbs(clean) || filepath.IsAbs(relative) || clean == ".." || strings.HasPrefix(clean, "../") || !fs.ValidPath(clean) {
		return "", fmt.Errorf("path %q is not inside the plugin", relative)
	}
	return clean, nil
}

// Stat describes a file of the plugin.
func (current Plugin) Stat(relative string) (fs.FileInfo, error) {
	clean, err := inside(relative)
	if err != nil {
		return nil, err
	}
	if current.Files == nil {
		return nil, fmt.Errorf("plugin %q has no files", current.Name)
	}
	return fs.Stat(current.Files, clean)
}

// ToolCommand is a tool's command with its paths resolved.
func (current Plugin) ToolCommand(definition Tool) ([]string, error) {
	command := slices.Clone(definition.Run)
	for index, argument := range command {
		if strings.HasPrefix(argument, "./") || strings.HasPrefix(argument, "../") {
			path, err := current.Resolve(argument)
			if err != nil {
				return nil, fmt.Errorf("tool %q: %w", definition.Name, err)
			}
			command[index] = path
		}
	}
	return command, nil
}

// Read loads the plugin in directory, from its plugin.json.
func Read(directory string, source Source) (Plugin, error) {
	absolute, err := filepath.Abs(directory)
	if err != nil {
		return Plugin{}, err
	}
	return read(os.DirFS(absolute), absolute, absolute, source)
}

// ReadFS loads the plugin whose files are fsys, such as one compiled into
// the program; name says where it comes from in errors.
func ReadFS(fsys fs.FS, name string, source Source) (Plugin, error) {
	return read(fsys, "", name, source)
}

func read(fsys fs.FS, directory, where string, source Source) (Plugin, error) {
	encoded, err := fs.ReadFile(fsys, ManifestName)
	if err != nil {
		return Plugin{}, err
	}
	var manifest Manifest
	if err := json.Unmarshal(encoded, &manifest, json.RejectUnknownMembers(true)); err != nil {
		return Plugin{}, fmt.Errorf("plugin %s: %s: %w", where, ManifestName, err)
	}
	loaded := Plugin{Manifest: manifest, Source: source, Directory: directory, Files: fsys}
	if err := loaded.validate(); err != nil {
		return Plugin{}, fmt.Errorf("plugin %s: %w", where, err)
	}
	if manifest.Prompt != "" {
		clean, _ := inside(manifest.Prompt)
		text, err := fs.ReadFile(fsys, clean)
		if err != nil {
			return Plugin{}, fmt.Errorf("plugin %q: read the prompt: %w", manifest.Name, err)
		}
		if len(text) > maxPromptBytes {
			return Plugin{}, fmt.Errorf("plugin %q: the prompt is over %d KiB", manifest.Name, maxPromptBytes>>10)
		}
		loaded.Instructions = strings.TrimSpace(string(text))
	}
	return loaded, nil
}

func (current Plugin) validate() error {
	if !pluginName.MatchString(current.Name) {
		return fmt.Errorf("name %q must be lowercase letters, digits and dashes", current.Name)
	}
	var problems []error
	tools := map[string]bool{}
	for index, definition := range current.Tools {
		switch {
		case !toolName.MatchString(definition.Name):
			problems = append(problems, fmt.Errorf("tools[%d]: name %q must start with a letter and hold letters, digits, _ and - only", index, definition.Name))
		case tools[definition.Name]:
			problems = append(problems, fmt.Errorf("tools[%d]: %q is declared twice", index, definition.Name))
		case strings.TrimSpace(definition.Description) == "":
			problems = append(problems, fmt.Errorf("tool %q needs a description", definition.Name))
		case len(definition.Run) == 0 || strings.TrimSpace(definition.Run[0]) == "":
			problems = append(problems, fmt.Errorf("tool %q needs the command it runs", definition.Name))
		case definition.Parameters != nil && definition.Parameters["type"] != "object":
			problems = append(problems, fmt.Errorf(`tool %q: parameters must be a JSON schema of "type": "object"`, definition.Name))
		case definition.MaxOutputLength < 0 || definition.MaxOutputLength > operation.MaxOutputLength:
			problems = append(problems, fmt.Errorf("tool %q: max_output_length must be between 1 and %d", definition.Name, operation.MaxOutputLength))
		}
		tools[definition.Name] = true
		if _, err := current.ToolCommand(definition); err != nil {
			problems = append(problems, err)
		}
	}
	commands := map[string]bool{}
	for index, command := range current.Commands {
		switch {
		case !commandName.MatchString(command.Name):
			problems = append(problems, fmt.Errorf("commands[%d]: name %q must be lowercase letters, digits and dashes", index, command.Name))
		case commands[command.Name]:
			problems = append(problems, fmt.Errorf("commands[%d]: /%s is declared twice", index, command.Name))
		case strings.TrimSpace(command.Description) == "" || strings.TrimSpace(command.Prompt) == "":
			problems = append(problems, fmt.Errorf("command /%s needs a description and a prompt", command.Name))
		}
		commands[command.Name] = true
	}
	files := map[string]string{"skills": current.Skills, "prompt": current.Prompt}
	if current.Web != nil {
		files["web.script"], files["web.style"] = current.Web.Script, current.Web.Style
		if current.Web.Script != "" && !strings.HasSuffix(current.Web.Script, ".js") && !strings.HasSuffix(current.Web.Script, ".mjs") {
			problems = append(problems, errors.New("web.script must be a .js or .mjs module"))
		}
		if current.Web.Style != "" && !strings.HasSuffix(current.Web.Style, ".css") {
			problems = append(problems, errors.New("web.style must be a .css file"))
		}
		for _, name := range current.Web.After {
			if !pluginName.MatchString(name) {
				problems = append(problems, fmt.Errorf("web.after: %q is not a plugin name", name))
			}
		}
	}
	for _, field := range slices.Sorted(maps.Keys(files)) {
		relative := files[field]
		if relative == "" {
			continue
		}
		info, err := current.Stat(relative)
		switch {
		case err != nil:
			problems = append(problems, fmt.Errorf("%s: %w", field, err))
		case field == "skills" && !info.IsDir():
			problems = append(problems, fmt.Errorf("skills: %s is not a directory", relative))
		case field != "skills" && info.IsDir():
			problems = append(problems, fmt.Errorf("%s: %s is a directory", field, relative))
		}
	}
	return errors.Join(problems...)
}

// ReadDirectory loads every plugin in directory, one per subdirectory. A
// missing directory holds none.
func ReadDirectory(directory string, source Source) ([]Plugin, []error) {
	entries, err := os.ReadDir(directory)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, []error{fmt.Errorf("list plugins: %w", err)}
	}
	var plugins []Plugin
	var problems []error
	for _, entry := range entries {
		path := filepath.Join(directory, entry.Name())
		if strings.HasPrefix(entry.Name(), ".") {
			continue
		}
		if info, err := os.Stat(path); err != nil || !info.IsDir() {
			continue
		}
		loaded, err := Read(path, source)
		switch {
		case errors.Is(err, fs.ErrNotExist) && !fileExists(filepath.Join(path, ManifestName)):
			continue // not a plugin
		case err != nil:
			problems = append(problems, err)
		default:
			plugins = append(plugins, loaded)
		}
	}
	return plugins, problems
}

// ReadDirectoryFS loads every plugin in fsys, one per directory, such as
// the plugins compiled into a program.
func ReadDirectoryFS(fsys fs.FS, source Source) ([]Plugin, []error) {
	entries, err := fs.ReadDir(fsys, ".")
	if err != nil {
		return nil, []error{fmt.Errorf("list plugins: %w", err)}
	}
	var plugins []Plugin
	var problems []error
	for _, entry := range entries {
		if !entry.IsDir() || strings.HasPrefix(entry.Name(), ".") {
			continue
		}
		sub, err := fs.Sub(fsys, entry.Name())
		if err != nil {
			problems = append(problems, err)
			continue
		}
		loaded, err := ReadFS(sub, entry.Name(), source)
		switch {
		case errors.Is(err, fs.ErrNotExist) && !fsFileExists(sub, ManifestName):
			continue // not a plugin
		case err != nil:
			problems = append(problems, err)
		default:
			plugins = append(plugins, loaded)
		}
	}
	return plugins, problems
}

func fsFileExists(fsys fs.FS, name string) bool {
	_, err := fs.Stat(fsys, name)
	return err == nil
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}
