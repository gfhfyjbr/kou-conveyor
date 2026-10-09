package agentrunner

import (
	"fmt"
	"io"
	"slices"
	"strings"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/skill"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/code"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/command"
)

// A run follows its plugins and its skills while it goes on. Before every
// turn the runner looks at where plugins come from — a stat per file — and
// at the skill directories — a stat per skill — and when something changed
// it reads them again: a tool added, changed or removed, other
// instructions, other skills, a plugin turned on or off reach the agent in
// the very next request, and the run goes on. Calls already made keep
// running as they were, and their results come back even when their tool
// has gone.

// WatchPluginsEnvironment set to 0 keeps a run's plugins as they were when
// it started.
const WatchPluginsEnvironment = "KOU_CONVEYOR_WATCH_PLUGINS"

// liveTool stands for a plugin's tool in the registry, which cannot forget
// a tool: it passes calls on to the tool's current translator, and turns
// new calls away once the tool is gone.
type liveTool struct {
	name    string
	mu      sync.RWMutex
	current tool.Translator // nil once the tool is gone
	last    tool.Translator // for the results of calls made before
}

func (live *liveTool) set(translator tool.Translator) {
	live.mu.Lock()
	defer live.mu.Unlock()
	live.current = translator
	if translator != nil {
		live.last = translator
	}
}

func (live *liveTool) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	live.mu.RLock()
	current := live.current
	live.mu.RUnlock()
	if current == nil {
		return tool.ErrorStatus(fmt.Sprintf("tool %q is not available any more: the plugin that gave it changed", live.name), 0)
	}
	return current.Translate(ctx, call)
}

func (live *liveTool) TranslateResult(callID string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	live.mu.RLock()
	translator := live.current
	if translator == nil {
		translator = live.last
	}
	live.mu.RUnlock()
	return translator.TranslateResult(callID, status, operations)
}

// livePlugins is what a run took from plugins and skill directories, to
// change as they change.
type livePlugins struct {
	options      plugin.Options
	skillOptions skill.Options // where skills come from, besides the plugins
	parsed       Request
	sessionID    session.ID
	workspace    string
	operations   string
	registry     tool.Registry
	builder      contextbuilder.Builder
	output       io.Writer

	systemPrompt string          // the system prompt without the plugins' instructions
	static       []llm.Tool      // the built-in tools the run enabled, when core is on
	skillUse     bool            // SkillUse resolves: skills can be registered
	profile      tool.Profile    // the built-in tools the model works with
	code         tool.Translator // the Code tool, whose declarations the prompt shows in code mode
	commandTools []codevm.CommandTool

	fingerprint      string       // of the plugins' files
	skillFingerprint string       // of the skill directories
	found            plugin.Found // the plugins as last read
	tools            map[string]*liveTool
	skills           []tool.Skill          // the skills registered, in order
	skillIDs         []tool.RegistrationID // their registrations
	shown            string                // what the plugins added, as last said
}

// apply makes the run's tools, instructions and skills those of found and
// of the skill directories. It returns what went wrong with the plugins,
// and with the skills.
func (live *livePlugins) apply(found plugin.Found) (pluginProblems, skillProblems []error) {
	live.found = found
	skillProblems = live.applySkills(found)
	problems := slices.Clone(found.Errors)
	var tools []llm.Tool
	if activePlugin(found, plugin.CoreName) {
		for _, definition := range live.static {
			// SkillUse is offered only while there are skills to use.
			if definition.Name != tool.SkillUseName || len(live.registry.Skills()) > 0 {
				tools = append(tools, definition)
			}
		}
	}
	active := map[string]bool{}
	var commandTools []codevm.CommandTool
	for _, current := range found.Active() {
		if ownTools(current) {
			continue // core's tools come with the registry
		}
		for _, definition := range current.Tools {
			if slices.Contains(live.parsed.DisallowedTools, definition.Name) {
				continue
			}
			model, translator, err := pluginTool(current, definition, live.sessionID, live.workspace, live.operations)
			if err != nil {
				problems = append(problems, fmt.Errorf("plugin %q: %w", current.Name, err))
				continue
			}
			if run, err := current.ToolCommand(definition); err == nil {
				commandTools = append(commandTools, codevm.CommandTool{
					Name: definition.Name, Description: definition.Description, Parameters: definition.Parameters, Command: run,
					Directory: live.workspace, Environment: pluginEnvironment(current, live.sessionID, live.workspace),
				})
			}
			// In code mode the plugins' tools are functions of the code.
			if live.profile == tool.ProfileCode {
				continue
			}
			existing := live.tools[definition.Name]
			if existing == nil {
				existing = &liveTool{name: definition.Name}
				if err := live.registry.RegisterTool(tool.Definition{Tool: model}, existing); err != nil {
					problems = append(problems, fmt.Errorf("plugin %q: %w", current.Name, err))
					continue
				}
				live.tools[definition.Name] = existing
			}
			existing.set(translator)
			active[definition.Name] = true
			tools = append(tools, model)
		}
	}
	for name, existing := range live.tools {
		if !active[name] {
			existing.set(nil)
		}
	}
	live.commandTools = commandTools
	live.builder.SetTools(tools)
	systemPrompt := live.systemPrompt
	if instructions := pluginInstructions(found); instructions != "" {
		systemPrompt = strings.TrimSpace(systemPrompt) + "\n\n" + instructions
	}
	if live.profile == tool.ProfileCode && live.code != nil {
		systemPrompt = strings.TrimSpace(systemPrompt) + "\n\n" + codePrompt(live.code)
	}
	live.builder.SetSystemPrompt(systemPrompt)
	return problems, skillProblems
}

// ownTools reports whether a plugin's tools are the runner's own, which
// the registry has from the start: core's. The other built-in plugins'
// tools — the canvas's, canvas-agent — run as any plugin's do.
func ownTools(current plugin.Plugin) bool {
	return current.Source == plugin.SourceBuiltin && current.Name == plugin.CoreName
}

// skillPaths are the skills by name, for the code's skill().
func (live *livePlugins) skillPaths() map[string]string {
	paths := make(map[string]string, len(live.skills))
	for _, current := range live.registry.Skills() {
		paths[current.Name] = current.Path
	}
	return paths
}

// pluginEnvironment is what a plugin's tool runs with.
func pluginEnvironment(current plugin.Plugin, sessionID session.ID, workspace string) map[string]string {
	return map[string]string{
		"KOU_CONVEYOR_PLUGIN_NAME": current.Name,
		"KOU_CONVEYOR_PLUGIN_DIR":  current.Directory,
		"KOU_CONVEYOR_WORKSPACE":   workspace,
		"KOU_CONVEYOR_SESSION_ID":  string(sessionID),
	}
}

// codePrompt is what the system prompt says of the Code tool: the
// functions the code may call, as TypeScript declarations.
func codePrompt(translator tool.Translator) string {
	return "## Code tool\n\nYou have one tool, Code, which runs the JavaScript you write in an isolated VM. The VM has no file system, network or modules of its own: only the functions declared below, which run the harness's tools and resolve to their results. Write the body of an async function; await the calls; run independent work at once with Promise.all; console.log what should be reported; return the value the result should end with. The result lists every call the code made and how it ended, then what the code logged and returned; a call's output is shown only when the call failed or when the code neither logs nor returns anything, so log or return what you need to read. Calls the code does not await are stopped when it returns. Prefer one well-planned call that does several steps to many small calls: each call is a turn.\n\n```ts\n" + code.Declarations(translator) + "\n```"
}

// applySkills registers the skills the agent has now — the project's and
// the system-wide ones, the active plugins' among them — in place of those
// registered before.
func (live *livePlugins) applySkills(found plugin.Found) []error {
	if !live.skillUse {
		return nil
	}
	discovered := discoverSkills(live.skillOptions, found)
	problems := discovered.Errors
	wanted := discovered.Active()
	if slices.Equal(wanted, live.skills) {
		return problems
	}
	for _, id := range live.skillIDs {
		live.registry.UnregisterSkill(id)
	}
	live.skills, live.skillIDs = nil, nil
	for _, current := range wanted {
		id, err := live.registry.RegisterSkill(current)
		if err != nil {
			problems = append(problems, fmt.Errorf("register skill %q: %w", current.Path, err))
			continue
		}
		live.skills, live.skillIDs = append(live.skills, current), append(live.skillIDs, id)
	}
	live.builder.SetSkills(live.registry.Skills())
	return problems
}

// skillChanges says which skills came (+), went (-) or changed (~), or
// nothing when none did.
func skillChanges(before, after []tool.Skill) string {
	was := make(map[string]tool.Skill, len(before))
	for _, current := range before {
		was[current.Name] = current
	}
	is := make(map[string]bool, len(after))
	var changes []string
	for _, current := range after {
		is[current.Name] = true
		switch previous, known := was[current.Name]; {
		case !known:
			changes = append(changes, "+"+current.Name)
		case previous != current:
			changes = append(changes, "~"+current.Name)
		}
	}
	for _, current := range before {
		if !is[current.Name] {
			changes = append(changes, "-"+current.Name)
		}
	}
	if len(changes) == 0 {
		return ""
	}
	return fmt.Sprintf("the skills changed; the agent has %d from its next turn: %s", len(after), strings.Join(changes, " "))
}

// describe says what the plugins add to the run: a line to compare, and
// to tell the user when it changed.
func (live *livePlugins) describe(found plugin.Found) string {
	var parts []string
	for _, current := range found.Active() {
		var adds []string
		for _, definition := range current.Tools {
			if !ownTools(current) && !slices.Contains(live.parsed.DisallowedTools, definition.Name) {
				adds = append(adds, definition.Name)
			}
		}
		if current.Instructions != "" {
			adds = append(adds, "instructions")
		}
		if current.Skills != "" {
			adds = append(adds, "skills")
		}
		if len(adds) > 0 {
			parts = append(parts, current.Name+" ("+strings.Join(adds, ", ")+")")
		}
	}
	if len(parts) == 0 {
		return "none"
	}
	return strings.Join(parts, "; ")
}

// beforeTurn reads the plugins and the skills again if their files changed
// since the last turn, and has the next request carry what they add now.
func (live *livePlugins) beforeTurn() {
	plugins := plugin.Fingerprint(plugin.Sources(live.options)...)
	skills := skill.Fingerprint(live.skillOptions)
	if plugins == live.fingerprint && skills == live.skillFingerprint {
		return
	}
	pluginsChanged := plugins != live.fingerprint
	live.fingerprint, live.skillFingerprint = plugins, skills
	found := live.found
	if pluginsChanged {
		found = plugin.Discover(live.options)
	}
	before := live.skills
	pluginProblems, skillProblems := live.apply(found)
	if shown := live.describe(found); shown != live.shown {
		live.shown = shown
		fmt.Fprintf(live.output, "plugin> the plugins changed; the agent works with them from its next turn: %s\n", shown)
	}
	if pluginsChanged {
		for _, problem := range pluginProblems {
			fmt.Fprintf(live.output, "plugin error> %s\n", problem)
		}
	}
	if changes := skillChanges(before, live.skills); changes != "" {
		fmt.Fprintf(live.output, "skill> %s\n", changes)
	}
	for _, problem := range skillProblems {
		fmt.Fprintf(live.output, "skill error> %s\n", problem)
	}
}

// pluginTool is the model's view of a plugin's tool, and its translator.
func pluginTool(current plugin.Plugin, definition plugin.Tool, sessionID session.ID, workspace, operations string) (llm.Tool, tool.Translator, error) {
	run, err := current.ToolCommand(definition)
	if err != nil {
		return llm.Tool{}, nil, err
	}
	parameters := definition.Parameters
	if parameters == nil {
		parameters = map[string]any{"type": "object", "properties": map[string]any{}}
	}
	model := llm.Tool{Type: llm.ToolFunction, Name: definition.Name, Description: definition.Description, Parameters: parameters}
	translator, err := command.New(command.Config{
		Tool: model, Command: run, Directory: workspace, BaseDirectory: operations,
		MaxOutputLength: definition.MaxOutputLength,
		Environment:     pluginEnvironment(current, sessionID, workspace),
	})
	return model, translator, err
}
