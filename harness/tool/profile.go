package tool

import (
	"fmt"
	"strings"
)

// Profile is the set of built-in tools the model works with. The models
// were trained on different ways to change files: the Claude models on
// Read, Edit and Write, the GPT models on apply_patch; a profile gives
// each its own, and the user may choose another in the cockpits. In code
// mode the model has one tool, Code, and calls the others from the code it
// writes.
type Profile string

const (
	// ProfileAuto picks the profile by the model: ProfileEdit for the
	// Claude models, ProfilePatch for the GPT models.
	ProfileAuto Profile = "auto"
	// ProfileEdit is Bash with Read, Edit and Write.
	ProfileEdit Profile = "edit"
	// ProfilePatch is Bash with Read and apply_patch.
	ProfilePatch Profile = "patch"
	// ProfileCode is the Code tool alone.
	ProfileCode Profile = "code"
	// ProfileShell is Bash alone, as the harness began: files through
	// the shell.
	ProfileShell Profile = "shell"
)

// Profiles lists the profiles, ProfileAuto first.
var Profiles = []Profile{ProfileAuto, ProfileEdit, ProfilePatch, ProfileCode, ProfileShell}

// ParseProfile reads a profile's name; empty is ProfileAuto.
func ParseProfile(name string) (Profile, error) {
	name = strings.ToLower(strings.TrimSpace(name))
	if name == "" {
		return ProfileAuto, nil
	}
	for _, profile := range Profiles {
		if string(profile) == name {
			return profile, nil
		}
	}
	return "", fmt.Errorf("unknown tool profile %q; the profiles are %s", name, ProfileNames())
}

// ProfileNames lists the profiles' names, comma-separated.
func ProfileNames() string {
	names := make([]string, len(Profiles))
	for index, profile := range Profiles {
		names[index] = string(profile)
	}
	return strings.Join(names, ", ")
}

func (profile Profile) Valid() bool {
	_, err := ParseProfile(string(profile))
	return err == nil
}

// Resolve is the profile ProfileAuto stands for with a model, and any
// other profile itself.
func (profile Profile) Resolve(model string) Profile {
	if profile != ProfileAuto && profile != "" {
		return profile
	}
	return ProfileForModel(model)
}

// ProfileForModel is the profile a model was trained for: ProfilePatch for
// the GPT models and OpenAI's reasoning models, ProfileEdit otherwise.
func ProfileForModel(model string) Profile {
	name := strings.ToLower(strings.TrimSpace(model))
	if at := strings.LastIndex(name, "/"); at >= 0 {
		name = name[at+1:]
	}
	switch {
	case strings.HasPrefix(name, "gpt"), strings.HasPrefix(name, "codex"), strings.HasPrefix(name, "chatgpt"):
		return ProfilePatch
	case len(name) >= 2 && name[0] == 'o' && name[1] >= '1' && name[1] <= '9':
		return ProfilePatch
	}
	return ProfileEdit
}

// Tools are the built-in tools of a profile, in the order the model sees
// them; SkillUse comes last, offered while there are skills. ProfileAuto
// has none of its own: resolve it first.
func (profile Profile) Tools() []string {
	switch profile {
	case ProfileEdit:
		return []string{BashName, ReadName, EditName, WriteName, ViewImageName, TranscriptSearchName, SkillUseName}
	case ProfilePatch:
		return []string{BashName, ReadName, ApplyPatchName, ViewImageName, TranscriptSearchName, SkillUseName}
	case ProfileCode:
		return []string{CodeName}
	case ProfileShell:
		return []string{BashName, ViewImageName, TranscriptSearchName, SkillUseName}
	}
	return nil
}

// Description says what a profile is, for the cockpits.
func (profile Profile) Description() string {
	switch profile {
	case ProfileAuto:
		return "By the model: edit for Claude, patch for GPT"
	case ProfileEdit:
		return "Bash, Read, Edit, Write, ViewImage, TranscriptSearch"
	case ProfilePatch:
		return "Bash, Read, apply_patch, ViewImage, TranscriptSearch"
	case ProfileCode:
		return "Code only: JavaScript that calls the tools, transcriptSearch() among them"
	case ProfileShell:
		return "Bash, ViewImage and TranscriptSearch: files through the shell"
	}
	return ""
}

// AllProfileTools are the built-in tools any profile uses.
func AllProfileTools() []string {
	seen := map[string]bool{}
	var names []string
	for _, profile := range Profiles {
		for _, name := range profile.Tools() {
			if !seen[name] {
				seen[name] = true
				names = append(names, name)
			}
		}
	}
	return names
}
