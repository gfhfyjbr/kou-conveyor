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
	// refetch: its results show what a call fetches again (plugin.Tool's
	// Refetch), as the plugin said last. Set and read on the coordinator's
	// loop alone: in apply, before a turn, and as the context is pruned.
	refetch bool
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

// Waits says whether the tool's calls wait (tool.Waiter), as the plugin
// that gives it says now, or said last.
func (live *liveTool) Waits() bool {
	live.mu.RLock()
	translator := live.current
	if translator == nil {
		translator = live.last
	}
	live.mu.RUnlock()
	return translator != nil && tool.Waits(translator)
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

// retiredTool stands for a tool the run has not whose calls the session
// holds from when it ran: a tool of a plugin the run found but does not
// run — one turned off, of a workspace not trusted, or that wants what the
// run has not, as the canvas's tools want a canvas — or a built-in tool the
// run's profile leaves out, as the patch profile of the GPT models leaves
// out Edit. A branch of a canvas agent's session, which runs off the
// canvas, holds the agent's calls of the canvas's tools. The results of
// the calls read as they did; a new call is turned away, saying why.
type retiredTool struct {
	translator tool.Translator
	refusal    string
	refetch    bool // as liveTool's
	// For a built-in tool the profile leaves out, the refusal is the
	// coordinator's for a tool there is not (tool.Unavailable), of the
	// tools the run has (unavailable) and the name the call gives (name):
	// it lists every tool, and is made only as a new call is turned away,
	// not as each call the session holds is resolved to read its result.
	unavailable tool.Registry
	name        string
}

func (retired retiredTool) Translate(tool.Context, llm.ToolCall) tool.CallStatus {
	if retired.unavailable != nil {
		return tool.ErrorStatus(tool.Unavailable(retired.unavailable, retired.name), 0)
	}
	return tool.ErrorStatus(retired.refusal, 0)
}

func (retired retiredTool) TranslateResult(callID string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	if status.Error != "" && len(status.WaitingFor) == 0 && len(operations) == 0 {
		// A call turned away reads as the coordinator gives one of a tool
		// there is not: the error alone, as such calls read before.
		return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: status.Error}}}, nil
	}
	return retired.translator.TranslateResult(callID, status, operations)
}

func (retired retiredTool) Waits() bool { return tool.Waits(retired.translator) }

// runTools are the tools the coordinator resolves calls with: the
// registry's, then the retired ones (retiredTool), which the model is not
// offered and the registry does not list.
type runTools struct {
	tool.Registry
	live     *livePlugins
	builtins map[string]tool.Translator // every built-in tool's, by name (builtinTranslators)
}

func (tools runTools) Resolve(name string) (tool.Translator, bool) {
	if translator, exists := tools.Registry.Resolve(name); exists {
		return translator, true
	}
	if retired, exists := tools.live.resolveRetired(name); exists {
		return retired, true
	}
	// A new call of a built-in tool the profile leaves out is told what
	// the coordinator tells a call of a tool there is not.
	if translator, exists := byName(tools.builtins, name); exists && translator != nil {
		return retiredTool{translator: translator, unavailable: tools.Registry, name: name}, true
	}
	return nil, false
}

// byName finds a tool by the name a call gives it, as the registry finds
// its own: by its name, or by the one of the names it ends with
// (tool.Aliases).
func byName[T any](tools map[string]T, name string) (T, bool) {
	if found, exists := tools[name]; exists {
		return found, true
	}
	names := make([]string, 0, len(tools))
	for known := range tools {
		names = append(names, known)
	}
	if matches := tool.Aliases(name, names); len(matches) == 1 {
		return tools[matches[0]], true
	}
	var none T
	return none, false
}

// builtinTranslators are the built-in tools' translators by name, those
// of the run's profile and the others.
func builtinTranslators(configured tool.StaticTranslators) map[string]tool.Translator {
	return map[string]tool.Translator{
		tool.BashName: configured.Bash, tool.ViewImageName: configured.ViewImage, tool.ReadName: configured.Read,
		tool.EditName: configured.Edit, tool.WriteName: configured.Write, tool.ApplyPatchName: configured.ApplyPatch,
		tool.CodeName: configured.Code, tool.TranscriptSearchName: configured.TranscriptSearch,
	}
}

// resolveRetired finds a retired tool by the name a call gives it (byName).
func (live *livePlugins) resolveRetired(name string) (tool.Translator, bool) {
	live.retiredMu.RLock()
	defer live.retiredMu.RUnlock()
	if retired, exists := byName(live.retired, name); exists {
		return retired, true
	}
	return nil, false
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

	// retired are the tools of the plugins found but not run, by name
	// (retiredTool); retiredMu guards them.
	retiredMu sync.RWMutex
	retired   map[string]retiredTool
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
			existing.refetch = definition.Refetch
			active[definition.Name] = true
			tools = append(tools, model)
		}
	}
	for name, existing := range live.tools {
		if !active[name] {
			existing.set(nil)
		}
	}
	live.retire(found)
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

// refetches reports whether a tool's results show what a call fetches
// again, as the plugin that gives it said last: those are pruned first
// (contextbuilder.PruneOptions.First). It runs on the coordinator's loop,
// as apply does.
func (live *livePlugins) refetches(name string) bool {
	if existing := live.tools[name]; existing != nil {
		return existing.refetch
	}
	live.retiredMu.RLock()
	defer live.retiredMu.RUnlock()
	return live.retired[name].refetch
}

// retire keeps the tools of the plugins found but not run, for the calls
// of them the session holds (retiredTool): those of the names no tool of
// the run has.
func (live *livePlugins) retire(found plugin.Found) {
	retired := map[string]retiredTool{}
	for _, current := range found.Plugins {
		if current.Active || ownTools(current) {
			continue
		}
		for _, definition := range current.Tools {
			if _, taken := retired[definition.Name]; taken || live.tools[definition.Name] != nil || slices.Contains(tool.AllProfileTools(), definition.Name) {
				continue
			}
			_, translator, err := pluginTool(current, definition, live.sessionID, live.workspace, live.operations)
			if err != nil {
				continue
			}
			refusal := fmt.Sprintf("tool %q is not available in this run: its plugin, %s, is not active", definition.Name, current.Name)
			if current.Reason != "" {
				refusal += " (" + current.Reason + ")"
			}
			retired[definition.Name] = retiredTool{translator: translator, refusal: refusal, refetch: definition.Refetch}
		}
	}
	live.retiredMu.Lock()
	live.retired = retired
	live.retiredMu.Unlock()
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
		Waits:           definition.Waits,
	})
	return model, translator, err
}
