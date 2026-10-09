package tool

import (
	"slices"
	"testing"
)

func TestProfileForModel(t *testing.T) {
	for model, want := range map[string]Profile{
		"claude-opus-5": ProfileEdit, "anthropic/claude-sonnet-4.6": ProfileEdit, "gpt-6-astra": ProfilePatch,
		"openai/gpt-5.1-codex": ProfilePatch, "o3-pro": ProfilePatch, "codex-mini": ProfilePatch, "gemini-3": ProfileEdit, "": ProfileEdit,
		"llama-4": ProfileEdit, "ollama/gpt-oss": ProfilePatch,
	} {
		if got := ProfileForModel(model); got != want {
			t.Errorf("ProfileForModel(%q) = %q, want %q", model, got, want)
		}
	}
	if ProfileAuto.Resolve("gpt-6") != ProfilePatch || ProfileCode.Resolve("gpt-6") != ProfileCode || Profile("").Resolve("claude-opus-5") != ProfileEdit {
		t.Fatal("Resolve")
	}
}

func TestParseProfile(t *testing.T) {
	if profile, err := ParseProfile(" Code "); err != nil || profile != ProfileCode {
		t.Fatalf("ParseProfile = %q, %v", profile, err)
	}
	if profile, err := ParseProfile(""); err != nil || profile != ProfileAuto {
		t.Fatalf("ParseProfile = %q, %v", profile, err)
	}
	if _, err := ParseProfile("bogus"); err == nil {
		t.Fatal("bogus was accepted")
	}
	if ProfileAuto.Tools() != nil || !slices.Contains(ProfileEdit.Tools(), EditName) || !slices.Contains(ProfilePatch.Tools(), ApplyPatchName) || slices.Contains(ProfilePatch.Tools(), EditName) {
		t.Fatal("Tools")
	}
	if tools := ProfileCode.Tools(); len(tools) != 1 || tools[0] != CodeName {
		t.Fatalf("code tools = %v", tools)
	}
	all := AllProfileTools()
	for _, name := range []string{BashName, ReadName, EditName, WriteName, ApplyPatchName, CodeName, ViewImageName, SkillUseName, TranscriptSearchName} {
		if !slices.Contains(all, name) {
			t.Fatalf("all profile tools lack %s: %v", name, all)
		}
	}
}
