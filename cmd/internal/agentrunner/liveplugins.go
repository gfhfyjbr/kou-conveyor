package agentrunner

import (
	"fmt"
	"io"
	"maps"
	"slices"
	"strings"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/command"
)

// A run follows its plugins while it goes on. Before every turn the runner
// looks at where plugins come from — a stat per file — and when something
// changed it reads them again: a tool added, changed or removed, other
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

// livePlugins is what a run took from plugins, to change as they change.
type livePlugins struct {
	options    plugin.Options
	parsed     Request
	sessionID  session.ID
	workspace  string
	operations string
	registry   tool.Registry
	builder    contextbuilder.Builder
	output     io.Writer

	systemPrompt string     // the system prompt without the plugins' instructions
	static       []llm.Tool // the built-in tools the run enabled, when core is on
	skillUse     bool       // SkillUse resolves: skills can be registered

	fingerprint string
	tools       map[string]*liveTool
	skills      map[string]tool.RegistrationID // plugin skills registered, by path
	shown       string                         // what the plugins added, as last said
}

// apply makes the run's tools, instructions and skills those of found.
func (live *livePlugins) apply(found plugin.Found) []error {
	problems := slices.Clone(found.Errors)
	problems = append(problems, live.applySkills(found)...)
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
	for _, current := range found.Active() {
		if current.Source == plugin.SourceBuiltin {
			continue // built-in tools come with the registry
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
	live.builder.SetTools(tools)
	systemPrompt := live.systemPrompt
	if instructions := pluginInstructions(found); instructions != "" {
		systemPrompt = strings.TrimSpace(systemPrompt) + "\n\n" + instructions
	}
	live.builder.SetSystemPrompt(systemPrompt)
	return problems
}

// applySkills registers the skills of the active plugins, and forgets
// those of plugins that went.
func (live *livePlugins) applySkills(found plugin.Found) []error {
	if !live.skillUse {
		return nil
	}
	skills, problems := pluginSkills(found)
	wanted := map[string]tool.Skill{}
	for _, skill := range skills {
		wanted[skill.Path] = skill
	}
	for path, id := range live.skills {
		if _, keep := wanted[path]; !keep {
			live.registry.UnregisterSkill(id)
			delete(live.skills, path)
		}
	}
	for _, path := range slices.Sorted(maps.Keys(wanted)) {
		skill := wanted[path]
		if _, registered := live.skills[path]; registered {
			// A skill that changed its name or description is registered anew.
			live.registry.UnregisterSkill(live.skills[path])
			delete(live.skills, path)
		}
		id, err := live.registry.RegisterSkill(skill)
		if err != nil {
			problems = append(problems, fmt.Errorf("register skill %q: %w", skill.Path, err))
			continue
		}
		live.skills[path] = id
	}
	live.builder.SetSkills(live.registry.Skills())
	return problems
}

// describe says what the plugins add to the run: a line to compare, and
// to tell the user when it changed.
func (live *livePlugins) describe(found plugin.Found) string {
	var parts []string
	for _, current := range found.Active() {
		var adds []string
		for _, definition := range current.Tools {
			if current.Source != plugin.SourceBuiltin && !slices.Contains(live.parsed.DisallowedTools, definition.Name) {
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

// beforeTurn reads the plugins again if their files changed since the
// last turn, and has the next request carry what they add now.
func (live *livePlugins) beforeTurn() {
	now := plugin.Fingerprint(plugin.Sources(live.options)...)
	if now == live.fingerprint {
		return
	}
	live.fingerprint = now
	found := plugin.Discover(live.options)
	problems := live.apply(found)
	if shown := live.describe(found); shown != live.shown {
		live.shown = shown
		fmt.Fprintf(live.output, "plugin> the plugins changed; the agent works with them from its next turn: %s\n", shown)
	}
	for _, problem := range problems {
		fmt.Fprintf(live.output, "plugin error> %s\n", problem)
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
		Environment: map[string]string{
			"KOU_CONVEYOR_PLUGIN_NAME": current.Name,
			"KOU_CONVEYOR_PLUGIN_DIR":  current.Directory,
			"KOU_CONVEYOR_WORKSPACE":   workspace,
			"KOU_CONVEYOR_SESSION_ID":  string(sessionID),
		},
	})
	return model, translator, err
}
