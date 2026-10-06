package tool

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"uuid"

	"gopkg.in/yaml.v3"
)

const (
	BashName             = "Bash"
	ViewImageName        = "ViewImage"
	SkillUseName         = "SkillUse"
	ReadName             = "Read"
	EditName             = "Edit"
	WriteName            = "Write"
	ApplyPatchName       = "apply_patch"
	CodeName             = "Code"
	TranscriptSearchName = "TranscriptSearch"
)

type registry struct {
	// The selection is copied at construction and never mutated; reads need no lock.
	enabled map[string]struct{}

	mu                sync.RWMutex
	staticTranslators map[string]Translator
	registered        []registeredTool
	skills            map[RegistrationID]Skill
	skillIDsByPath    map[string]RegistrationID
	skillOrder        []RegistrationID
}

var _ Registry = (*registry)(nil)

type registeredTool struct {
	definition Definition
	translator Translator
}

type StaticTranslators struct {
	Bash             Translator
	ViewImage        Translator
	Read             Translator
	Edit             Translator
	Write            Translator
	ApplyPatch       Translator
	Code             Translator
	TranscriptSearch Translator
}

func NewRegistry(configured StaticTranslators, enabled ...string) Registry {
	current := &registry{
		enabled:        make(map[string]struct{}, len(enabled)),
		skills:         make(map[RegistrationID]Skill),
		skillIDsByPath: make(map[string]RegistrationID),
	}
	for _, name := range enabled {
		current.enabled[name] = struct{}{}
	}
	current.staticTranslators = map[string]Translator{
		BashName:             configured.Bash,
		ViewImageName:        configured.ViewImage,
		ReadName:             configured.Read,
		EditName:             configured.Edit,
		WriteName:            configured.Write,
		ApplyPatchName:       configured.ApplyPatch,
		CodeName:             configured.Code,
		TranscriptSearchName: configured.TranscriptSearch,
		SkillUseName:         &skillUseTranslator{registry: current},
	}
	for name, translator := range current.staticTranslators {
		if translator == nil {
			current.staticTranslators[name] = unavailableTranslator{name: name}
		}
	}
	return current
}

func (current *registry) StaticDefinitions() []Definition {
	var definitions []Definition
	for _, definition := range staticDefinitions() {
		if _, enabled := current.enabled[definition.Tool.Name]; enabled {
			definitions = append(definitions, definition)
		}
	}
	current.mu.RLock()
	defer current.mu.RUnlock()
	for _, registered := range current.registered {
		definitions = append(definitions, registered.definition)
	}
	return definitions
}

// Resolve finds the translator of the tool a call names. A gateway between
// the harness and the model may show the model the tools under names of its
// own and turn them back when the model calls them: CLIProxyAPI shows Claude
// Read as "mcp__<server>__<word>_Read". A model that shortens such a name
// sends one the gateway passes on as it is, and which no tool has; it
// resolves to the one tool its end names, if exactly one does (Aliases).
func (current *registry) Resolve(name string) (Translator, bool) {
	if translator, exists := current.resolve(name); exists {
		return translator, true
	}
	if matches := Aliases(name, current.names()); len(matches) == 1 {
		return current.resolve(matches[0])
	}
	return nil, false
}

// names are the names of the enabled tools.
func (current *registry) names() []string {
	var names []string
	for name := range current.staticTranslators {
		if _, enabled := current.enabled[name]; enabled {
			names = append(names, name)
		}
	}
	current.mu.RLock()
	defer current.mu.RUnlock()
	for _, registered := range current.registered {
		names = append(names, registered.definition.Tool.Name)
	}
	return names
}

// Aliases returns the tools of names that name, which none of them has, may
// stand for: the ends of name that follow an underscore, such as Read for
// "mcp__server__grace_Read", "server__grace_Read" or "grace_Read", and
// apply_patch for "grace_apply_patch". They come sorted, each once.
func Aliases(name string, names []string) []string {
	var matches []string
	for index := 1; index < len(name)-1; index++ {
		if name[index] != '_' || name[index+1] == '_' {
			continue
		}
		if end := name[index+1:]; slices.Contains(names, end) && !slices.Contains(matches, end) {
			matches = append(matches, end)
		}
	}
	slices.Sort(matches)
	return matches
}

// Unavailable is the error of a call to a tool no name resolves: which tools
// there are, or which of them the name may stand for.
func Unavailable(registry Registry, name string) string {
	var names []string
	for _, definition := range registry.StaticDefinitions() {
		names = append(names, definition.Tool.Name)
	}
	if matches := Aliases(name, names); len(matches) > 1 {
		return fmt.Sprintf("tool %q is not available: it may stand for any of %s; call the one you mean by its exact name", name, strings.Join(matches, ", "))
	}
	if len(names) == 0 {
		return fmt.Sprintf("tool %q is not available", name)
	}
	return fmt.Sprintf("tool %q is not available; the tools are %s", name, strings.Join(names, ", "))
}

func (current *registry) resolve(name string) (Translator, bool) {
	if translator, exists := current.staticTranslators[name]; exists {
		if _, enabled := current.enabled[name]; enabled {
			return translator, true
		}
	}
	current.mu.RLock()
	defer current.mu.RUnlock()
	for _, registered := range current.registered {
		if registered.definition.Tool.Name == name {
			return registered.translator, true
		}
	}
	return nil, false
}

func (current *registry) RegisterTool(definition Definition, translator Translator) error {
	name := definition.Tool.Name
	switch {
	case strings.TrimSpace(name) == "":
		return errors.New("tool name must be set")
	case translator == nil:
		return fmt.Errorf("tool %q has no translator", name)
	}
	if _, static := current.staticTranslators[name]; static {
		if _, enabled := current.enabled[name]; enabled {
			return fmt.Errorf("tool %q is built in", name)
		}
	}
	current.mu.Lock()
	defer current.mu.Unlock()
	for _, registered := range current.registered {
		if registered.definition.Tool.Name == name {
			return fmt.Errorf("tool %q is already registered", name)
		}
	}
	current.registered = append(current.registered, registeredTool{definition: definition, translator: translator})
	return nil
}

func (current *registry) RegisterSkill(skill Skill) (RegistrationID, error) {
	if err := validateSkill(skill); err != nil {
		return uuid.Nil(), err
	}
	current.mu.Lock()
	defer current.mu.Unlock()
	if _, exists := current.skillIDsByPath[skill.Path]; exists {
		return uuid.Nil(), fmt.Errorf("skill path %q is already registered", skill.Path)
	}
	for _, registered := range current.skills {
		if registered.Name == skill.Name {
			return uuid.Nil(), fmt.Errorf("skill name %q is already registered", skill.Name)
		}
	}
	id := uuid.New()
	current.skills[id] = skill
	current.skillIDsByPath[skill.Path] = id
	current.skillOrder = append(current.skillOrder, id)
	return id, nil
}

func validateSkill(skill Skill) error {
	if strings.TrimSpace(skill.Path) == "" {
		return errors.New("skill path must be set")
	}
	if strings.TrimSpace(skill.Name) == "" {
		return errors.New("skill name must be set")
	}
	if strings.TrimSpace(skill.Description) == "" {
		return errors.New("skill description must be set")
	}
	return nil
}

func (current *registry) UnregisterSkill(id RegistrationID) {
	if id == uuid.Nil() {
		return
	}
	current.mu.Lock()
	defer current.mu.Unlock()
	skill, exists := current.skills[id]
	if !exists {
		return
	}
	delete(current.skills, id)
	delete(current.skillIDsByPath, skill.Path)
	current.skillOrder = removeRegistrationID(current.skillOrder, id)
}

func (current *registry) Skills() []Skill {
	current.mu.RLock()
	defer current.mu.RUnlock()
	result := make([]Skill, 0, len(current.skills))
	for _, id := range current.skillOrder {
		result = append(result, current.skills[id])
	}
	return result
}

func (current *registry) resolveSkill(name string) (Skill, bool) {
	current.mu.RLock()
	defer current.mu.RUnlock()
	for _, id := range current.skillOrder {
		skill := current.skills[id]
		if skill.Name == name {
			return skill, true
		}
	}
	return Skill{}, false
}

// DiscoverSkills reads the skills of a directory: its subdirectories, or
// links to directories, with a SKILL.md. A directory that is not there
// holds none.
func DiscoverSkills(directory string) ([]Skill, []error) {
	entries, err := os.ReadDir(directory)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, []error{fmt.Errorf("find skills: %w", err)}
	}
	var paths []string
	for _, entry := range entries {
		path := filepath.Join(directory, entry.Name(), "SKILL.md")
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() {
			paths = append(paths, path)
		}
	}

	var skills []Skill
	var skillErrors []error
	names := make(map[string]struct{})
	for _, path := range paths {
		contents, err := os.ReadFile(path)
		if err != nil {
			skillErrors = append(skillErrors, fmt.Errorf("read skill %q: %w", path, err))
			continue
		}
		frontmatter, err := parseSkillFrontmatter(contents)
		if err != nil {
			skillErrors = append(skillErrors, fmt.Errorf("parse skill %q: %w", path, err))
			continue
		}
		skill := Skill{
			Name:        strings.TrimSpace(frontmatter.Name),
			Description: strings.TrimSpace(frontmatter.Description),
			Path:        path,
			Manual:      frontmatter.Manual,
		}
		if err := validateSkill(skill); err != nil {
			skillErrors = append(skillErrors, fmt.Errorf("validate skill %q: %w", path, err))
			continue
		}
		if _, exists := names[skill.Name]; exists {
			skillErrors = append(skillErrors, fmt.Errorf("skill %q: duplicate name %q", path, skill.Name))
			continue
		}
		names[skill.Name] = struct{}{}
		skills = append(skills, skill)
	}
	return skills, skillErrors
}

type skillFrontmatter struct {
	Name        string
	Description string
	Manual      bool
}

// parseSkillFrontmatter reads the metadata of a SKILL.md: the YAML between
// its first line, ---, and the next ---. Skills are written for many agents
// and by hand, so frontmatter that is not valid YAML — a description with an
// unquoted colon is common — is read a line at a time instead.
func parseSkillFrontmatter(contents []byte) (skillFrontmatter, error) {
	block, err := frontmatterBlock(string(contents))
	if err != nil {
		return skillFrontmatter{}, err
	}
	var metadata struct {
		Name        string `yaml:"name"`
		Description string `yaml:"description"`
		// DisableModelInvocation keeps the model from loading the skill on
		// its own, as other agents take it: it loads when the user asks.
		DisableModelInvocation bool `yaml:"disable-model-invocation"`
	}
	if err := yaml.Unmarshal([]byte(block), &metadata); err == nil {
		return skillFrontmatter{Name: metadata.Name, Description: metadata.Description, Manual: metadata.DisableModelInvocation}, nil
	}
	return lineFrontmatter(block), nil
}

// frontmatterBlock returns the text between the frontmatter's delimiters.
func frontmatterBlock(contents string) (string, error) {
	contents = strings.TrimPrefix(contents, "\uFEFF")
	first, rest, _ := strings.Cut(contents, "\n")
	if strings.TrimSpace(first) != "---" {
		return "", errors.New("missing opening YAML frontmatter delimiter")
	}
	var lines []string
	for rest != "" {
		var line string
		line, rest, _ = strings.Cut(rest, "\n")
		if strings.TrimSpace(line) == "---" {
			return strings.Join(lines, "\n"), nil
		}
		lines = append(lines, strings.TrimSuffix(line, "\r"))
	}
	return "", errors.New("missing closing YAML frontmatter delimiter")
}

// lineFrontmatter reads frontmatter that is not valid YAML as lines of
// key: value, a value in matching quotes taken without them.
func lineFrontmatter(block string) skillFrontmatter {
	var metadata skillFrontmatter
	for line := range strings.SplitSeq(block, "\n") {
		key, value, found := strings.Cut(line, ":")
		if !found {
			continue
		}
		value = unquote(strings.TrimSpace(value))
		switch key {
		case "name":
			metadata.Name = value
		case "description":
			metadata.Description = value
		case "disable-model-invocation":
			metadata.Manual = strings.EqualFold(value, "true")
		}
	}
	return metadata
}

func unquote(value string) string {
	if len(value) >= 2 && (value[0] == '"' || value[0] == '\'') && value[len(value)-1] == value[0] {
		return value[1 : len(value)-1]
	}
	return value
}

func removeRegistrationID(ids []RegistrationID, target RegistrationID) []RegistrationID {
	for index, id := range ids {
		if id != target {
			continue
		}
		copy(ids[index:], ids[index+1:])
		ids[len(ids)-1] = uuid.Nil()
		return ids[:len(ids)-1]
	}
	return ids
}
