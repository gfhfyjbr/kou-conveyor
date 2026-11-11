//! Pure model-call translation. Submission records specs; translators perform no I/O.
mod arguments;
mod bash;
mod command;
mod definitions;
mod file;
mod number;
mod output;
mod profile;
mod registry;
mod skill_use;
mod skills;
#[cfg(test)]
mod tests;
use crate::operation::{Error, Result, Spec};
pub use bash::{Bash, BashConfig};
pub use command::{Command, CommandConfig};
pub use file::{File, FileConfig, ReadJournalLimits, format_file_result};
use kou_domain::{CallStatus, Operation, ToolCall, ToolResult, ToolResultOutput};
pub use skill_use::{SKILL_USE_NAME, SkillUse};
pub use skills::{RegistrationId, Skill, SkillCatalog};
// The production registry: definitions, profiles and name resolution.
pub use definitions::{
    APPLY_PATCH_DESCRIPTION, APPLY_PATCH_NAME, BASH_NAME, CODE_DESCRIPTION, CODE_NAME,
    DEFAULT_SHELL_TIMEOUT, Definition, EDIT_DESCRIPTION, EDIT_NAME, READ_DESCRIPTION, READ_NAME,
    STATIC_NAMES, TOOL_FUNCTION, TRANSCRIPT_SEARCH_DESCRIPTION, TRANSCRIPT_SEARCH_NAME, Tool,
    VIEW_IMAGE_NAME, WRITE_DESCRIPTION, WRITE_NAME,
};
pub use output::parse_max_output_length;
pub use profile::{PROFILES, Profile, all_profile_tools, profile_names};
pub use registry::{Registry, StaticTranslators, aliases, unavailable};
use std::sync::Arc;

pub const DEFAULT_MAX_OUTPUT_LENGTH: usize = 40_000;
pub trait Translator: Send + Sync {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus;
    /// Preserve call identity for translators that export it to a capability.
    fn translate_call(
        &self,
        call: &ToolCall,
        submit: &mut dyn FnMut(Spec) -> String,
    ) -> CallStatus {
        self.translate(&call.arguments, submit)
    }
    fn waits(&self) -> bool {
        false
    }
    fn result(
        &self,
        call_id: &str,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<ToolResult>;
    fn running(&self, _operations: &[Arc<Operation>]) -> String {
        String::new()
    }
}
fn text_result(call_id: &str, text: String) -> ToolResult {
    ToolResult {
        call_id: call_id.into(),
        output: vec![ToolResultOutput {
            kind: "text".into(),
            value: text,
        }],
    }
}
/// An error the model reads in place of a call's result, bounded to `limit`
/// characters; zero is the default limit (Go `tool.ErrorStatus`).
pub fn error_status(message: String, limit: usize) -> CallStatus {
    let limit = if limit == 0 {
        DEFAULT_MAX_OUTPUT_LENGTH
    } else {
        limit
    };
    let (error, truncated) = bound_text(message, limit);
    CallStatus {
        error,
        error_truncated: truncated,
        ..CallStatus::default()
    }
}
fn bound_text(text: String, limit: usize) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text, false);
    }
    let head = text
        .char_indices()
        .nth(limit / 2)
        .map_or(text.len(), |(n, _)| n);
    let tail = text
        .char_indices()
        .rev()
        .nth(limit - limit / 2 - 1)
        .map_or(0, |(n, _)| n);
    (
        format!(
            "{}...{} bytes truncated...{}",
            &text[..head],
            tail - head,
            &text[tail..]
        ),
        true,
    )
}
fn type_error(value: &str, target: &str) -> String {
    let shape = match value.trim().as_bytes().first() {
        Some(b'{') => "object",
        Some(b'[') => "array",
        Some(b'\"') => "string",
        Some(b't' | b'f') => "boolean",
        Some(b'n') => "null",
        _ => "number",
    };
    format!("json: cannot unmarshal JSON {shape} into Go {target}")
}
