//! Tool profiles (Go `harness/tool/profile.go`): the set of built-in tools the
//! model works with. The models were trained on different ways to change
//! files: the Claude models on Read, Edit and Write, the GPT models on
//! apply_patch. In code mode the model has one tool, Code, and calls the
//! others from the code it writes.
use super::*;
use skills::quoted;
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Picks the profile by the model: `Edit` for the Claude models, `Patch`
    /// for the GPT models.
    #[default]
    Auto,
    /// Bash with Read, Edit and Write.
    Edit,
    /// Bash with Read and apply_patch.
    Patch,
    /// The Code tool alone.
    Code,
    /// Bash alone, as the harness began: files through the shell.
    Shell,
}

/// The profiles, `Auto` first.
pub const PROFILES: [Profile; 5] = [
    Profile::Auto,
    Profile::Edit,
    Profile::Patch,
    Profile::Code,
    Profile::Shell,
];

impl Profile {
    /// Reads a profile's name; empty is `Auto`.
    pub fn parse(name: &str) -> Result<Self> {
        let name = lower(name.trim());
        if name.is_empty() {
            return Ok(Self::Auto);
        }
        PROFILES
            .into_iter()
            .find(|profile| profile.name() == name)
            .ok_or_else(|| {
                Error(format!(
                    "unknown tool profile {}; the profiles are {}",
                    quoted(&name),
                    profile_names()
                ))
            })
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Edit => "edit",
            Self::Patch => "patch",
            Self::Code => "code",
            Self::Shell => "shell",
        }
    }

    /// The profile `Auto` stands for with a model, and any other profile itself.
    pub fn resolve(self, model: &str) -> Self {
        if self == Self::Auto {
            Self::for_model(model)
        } else {
            self
        }
    }

    /// The profile a model was trained for: `Patch` for the GPT models and
    /// OpenAI's reasoning models, `Edit` otherwise.
    pub fn for_model(model: &str) -> Self {
        let name = lower(model.trim());
        let name = name
            .rsplit_once('/')
            .map_or(name.as_str(), |(_, name)| name);
        let reasoning = matches!(name.as_bytes(), [b'o', b'1'..=b'9', ..]);
        if reasoning
            || ["gpt", "codex", "chatgpt"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        {
            Self::Patch
        } else {
            Self::Edit
        }
    }

    /// The built-in tools of a profile, in the order the model sees them;
    /// SkillUse comes last, offered while there are skills. `Auto` has none of
    /// its own: resolve it first.
    pub const fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::Edit => &[
                BASH_NAME,
                READ_NAME,
                EDIT_NAME,
                WRITE_NAME,
                VIEW_IMAGE_NAME,
                TRANSCRIPT_SEARCH_NAME,
                SKILL_USE_NAME,
            ],
            Self::Patch => &[
                BASH_NAME,
                READ_NAME,
                APPLY_PATCH_NAME,
                VIEW_IMAGE_NAME,
                TRANSCRIPT_SEARCH_NAME,
                SKILL_USE_NAME,
            ],
            Self::Code => &[CODE_NAME],
            Self::Shell => &[
                BASH_NAME,
                VIEW_IMAGE_NAME,
                TRANSCRIPT_SEARCH_NAME,
                SKILL_USE_NAME,
            ],
        }
    }

    /// Says what a profile is, for the cockpits.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Auto => "By the model: edit for Claude, patch for GPT",
            Self::Edit => "Bash, Read, Edit, Write, ViewImage, TranscriptSearch",
            Self::Patch => "Bash, Read, apply_patch, ViewImage, TranscriptSearch",
            Self::Code => {
                "Code only: JavaScript that calls the tools, transcriptSearch() among them"
            }
            Self::Shell => "Bash, ViewImage and TranscriptSearch: files through the shell",
        }
    }
}
impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl FromStr for Profile {
    type Err = Error;
    fn from_str(name: &str) -> Result<Self> {
        Self::parse(name)
    }
}

/// The profiles' names, comma-separated.
pub fn profile_names() -> String {
    PROFILES.map(Profile::name).join(", ")
}

/// The built-in tools any profile uses, each once, in the order the profiles name them.
pub fn all_profile_tools() -> Vec<&'static str> {
    let mut names = Vec::new();
    for name in PROFILES.iter().flat_map(|profile| profile.tools()) {
        if !names.contains(name) {
            names.push(*name);
        }
    }
    names
}

/// Go's `strings.ToLower` maps each scalar to one scalar, without the context
/// rules of `str::to_lowercase`. U+0130 alone has a longer full mapping, whose
/// first scalar is the one Go gives.
fn lower(text: &str) -> String {
    text.chars()
        .map(|scalar| scalar.to_lowercase().next().unwrap_or(scalar))
        .collect()
}

#[cfg(test)]
mod tests;
