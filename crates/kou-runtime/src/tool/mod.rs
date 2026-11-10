//! Pure model-call translation. Submission records specs; translators perform no I/O.
mod arguments;
mod bash;
mod file;
mod number;
#[cfg(test)]
mod tests;
use crate::operation::{Error, Result, Spec};
pub use bash::{Bash, BashConfig};
pub use file::{File, FileConfig, ReadJournalLimits, format_file_result};
use kou_domain::{CallStatus, Operation, ToolResult, ToolResultOutput};
use std::sync::Arc;

pub const DEFAULT_MAX_OUTPUT_LENGTH: usize = 40_000;
pub trait Translator: Send + Sync {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus;
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
fn error_status(message: String, limit: usize) -> CallStatus {
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
