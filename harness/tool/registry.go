package tool

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"uuid"
)

const (
	BashName      = "Bash"
	ViewImageName = "ViewImage"
	SkillUseName  = "SkillUse"
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
	Bash      Translator
	ViewImage Translator
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
	if configured.Bash == nil {
		configured.Bash = unavailableTranslator{name: BashName}
	}
	if configured.ViewImage == nil {
		configured.ViewImage = unavailableTranslator{name: ViewImageName}
	}
	current.staticTranslators = map[string]Translator{
		BashName:      configured.Bash,
		ViewImageName: configured.ViewImage,
		SkillUseName:  &skillUseTranslator{registry: current},
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

func (current *registry) Resolve(name string) (Translator, bool) {
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

func DiscoverSkills(directory string) ([]Skill, []error) {
	paths, err := filepath.Glob(filepath.Join(directory, "*", "SKILL.md"))
	if err != nil {
		return nil, []error{fmt.Errorf("find skills: %w", err)}
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
}

func parseSkillFrontmatter(contents []byte) (skillFrontmatter, error) {
	scanner := bufio.NewScanner(bytes.NewReader(contents))
	if !scanner.Scan() || strings.TrimSpace(scanner.Text()) != "---" {
		return skillFrontmatter{}, errors.New("missing opening YAML frontmatter delimiter")
	}

	var metadata skillFrontmatter
	for scanner.Scan() {
		line := scanner.Text()
		if strings.TrimSpace(line) == "---" {
			return metadata, nil
		}
		key, value, found := strings.Cut(line, ":")
		if !found {
			continue
		}
		switch key {
		case "name":
			metadata.Name = strings.TrimSpace(value)
		case "description":
			metadata.Description = strings.TrimSpace(value)
		}
	}
	if err := scanner.Err(); err != nil {
		return skillFrontmatter{}, fmt.Errorf("read YAML frontmatter: %w", err)
	}
	return skillFrontmatter{}, errors.New("missing closing YAML frontmatter delimiter")
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
