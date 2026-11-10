use super::*;
use crate::operation::{FileState, decode_file, file_spec};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::borrow::Cow;
mod journal;
#[cfg(test)]
mod tests;
use journal::Journal;
pub use journal::ReadJournalLimits;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FileConfig {
    pub directory: String,
    pub transcript: String,
}
pub struct File {
    kind: String,
    name: String,
    config: FileConfig,
    reads: Option<Journal>,
}
impl File {
    pub fn new(kind: impl Into<String>, name: impl Into<String>, config: FileConfig) -> Self {
        Self::with_journal_limits(kind, name, config, ReadJournalLimits::default())
    }
    pub fn with_journal_limits(
        kind: impl Into<String>,
        name: impl Into<String>,
        config: FileConfig,
        limits: ReadJournalLimits,
    ) -> Self {
        let kind = kind.into();
        let reads = (kind == "read").then(|| Journal::new(limits));
        Self {
            kind,
            name: name.into(),
            config,
            reads,
        }
    }
    fn state(&self, encoded: &str) -> Result<FileState> {
        let encoded = encoded.trim();
        let encoded = if encoded.is_empty() { "{}" } else { encoded };
        if !encoded.starts_with('{') || kou_domain::validate_json(encoded.as_bytes()).is_err() {
            return Err(Error(format!(
                "{} arguments must be a JSON object",
                self.name
            )));
        }
        let parsed = Parsed::parse(encoded)
            .map_err(|error| Error(format!("decode {} arguments: {error}", self.name)))?;
        let offset = count("offset", parsed.offset)
            .map_err(|error| Error(format!("{} {error}", self.name)))?;
        let limit = count("limit", parsed.limit)
            .map_err(|error| Error(format!("{} {error}", self.name)))?;
        let path = parsed.path.trim();
        let path = if path.is_empty() {
            parsed.file_path.trim()
        } else {
            path
        };
        if path.contains('\0') {
            return Err(Error(format!(
                "{} argument \"path\" contains a NUL byte",
                self.name
            )));
        }
        let need_path = || {
            if path.is_empty() {
                return Err(Error(format!(
                    "{} argument \"path\" must be set",
                    self.name
                )));
            }
            Ok(
                if !self.config.directory.is_empty() && !path.starts_with('/') {
                    format!("{}/{path}", self.config.directory)
                } else {
                    path.to_owned()
                },
            )
        };
        let state = match self.kind.as_str() {
            "read" => FileState {
                action: "read".into(),
                path: need_path()?,
                offset,
                limit,
                ..FileState::default()
            },
            "edit" => {
                let path = need_path()?;
                let (Some(old), Some(new)) = (parsed.old_string, parsed.new_string) else {
                    return Err(Error(format!(
                        "{} arguments \"old_string\" and \"new_string\" must be set",
                        self.name
                    )));
                };
                FileState {
                    action: "edit".into(),
                    path,
                    old_string: old.into_owned(),
                    new_string: new.into_owned(),
                    replace_all: parsed.replace_all,
                    ..FileState::default()
                }
            }
            "write" => FileState {
                action: "write".into(),
                path: need_path()?,
                content: parsed.content.into_owned(),
                ..FileState::default()
            },
            "apply_patch" => {
                let patch = if parsed.input.trim().is_empty() {
                    parsed.patch
                } else {
                    parsed.input
                };
                if patch.trim().is_empty() {
                    return Err(Error(format!(
                        "{} argument \"input\" must hold the patch",
                        self.name
                    )));
                }
                FileState {
                    action: "patch".into(),
                    root: self.config.directory.clone(),
                    patch: patch.into_owned(),
                    ..FileState::default()
                }
            }
            "transcript_search" => {
                if parsed.query.trim().is_empty() {
                    return Err(Error(format!(
                        "{} argument \"query\" must be set",
                        self.name
                    )));
                }
                if self.config.transcript.is_empty() {
                    return Err(Error("no transcript is available in this run".into()));
                }
                FileState {
                    action: "transcript_search".into(),
                    path: self.config.transcript.clone(),
                    query: parsed.query.into_owned(),
                    limit,
                    ..FileState::default()
                }
            }
            _ => return Err(Error(format!("unsupported file tool kind {:?}", self.kind))),
        };
        Ok(state)
    }
}
impl Translator for File {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        let state = match self.state(arguments) {
            Ok(state) => state,
            Err(error) => return error_status(error.to_string(), 0),
        };
        let spec = match file_spec(state) {
            Ok(spec) => spec,
            Err(error) => {
                return error_status(format!("build {} operation: {error}", self.name), 0);
            }
        };
        CallStatus {
            waiting_for: Some(vec![submit(spec)]),
            ..CallStatus::default()
        }
    }
    fn result(
        &self,
        call_id: &str,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<ToolResult> {
        if !status.error.is_empty() {
            if !operations.is_empty() {
                return Err(Error(format!(
                    "{} call {call_id:?} has both a validation error and operations",
                    self.name
                )));
            }
            return Ok(text_result(call_id, format!("Error: {}", status.error)));
        }
        let [current] = operations else {
            return Err(Error(format!(
                "{} call {call_id:?} has {} operations, want 1",
                self.name,
                operations.len()
            )));
        };
        let state = result_state(&self.name, call_id, current)?;
        let note = self.reads.as_ref().map_or_else(String::new, |journal| {
            journal.note(call_id, current, &state)
        });
        let mut output = render(&self.name, call_id, current, state)?;
        output.push_str(&note);
        Ok(text_result(call_id, output))
    }
}
pub fn format_file_result(name: &str, call_id: &str, current: &Operation) -> Result<String> {
    render(
        name,
        call_id,
        current,
        result_state(name, call_id, current)?,
    )
}
fn result_state(name: &str, call_id: &str, current: &Operation) -> Result<FileState> {
    decode_file(current)
        .map_err(|error| Error(format!("decode {name} call {call_id:?} result: {error}")))
}
fn render(name: &str, call_id: &str, current: &Operation, state: FileState) -> Result<String> {
    match current.status.as_str() {
        "ready" | "awaiting" | "canceling" => Ok("The file operation is still running.".into()),
        "completed" => state.result.map(|result| result.text).ok_or_else(|| {
            Error(format!(
                "{name} call {call_id:?} completed operation {:?} has no result",
                current.id
            ))
        }),
        "failed" | "canceled" => {
            if state.terminal_error.is_empty() {
                return Err(Error(format!(
                    "file operation {} without an error",
                    current.status
                )));
            }
            Ok(format!("Error: {}", state.terminal_error))
        }
        _ => Err(Error(format!(
            "{name} call {call_id:?} operation {:?} has invalid status {:?}",
            current.id, current.status
        ))),
    }
}
#[derive(Default)]
struct Parsed<'a> {
    path: Cow<'a, str>,
    file_path: Cow<'a, str>,
    content: Cow<'a, str>,
    input: Cow<'a, str>,
    patch: Cow<'a, str>,
    query: Cow<'a, str>,
    old_string: Option<Cow<'a, str>>,
    new_string: Option<Cow<'a, str>>,
    offset: Option<f64>,
    limit: Option<f64>,
    replace_all: bool,
}
impl<'a> Parsed<'a> {
    fn parse(encoded: &'a str) -> Result<Self> {
        let mut parsed = Self::default();
        for (name, raw) in arguments::ordered(encoded)? {
            let string = || arguments::borrowed_string(raw).map_err(|error| within(error, &name));
            match name.as_str() {
                "path" => parsed.path = string()?.unwrap_or_default(),
                "file_path" => parsed.file_path = string()?.unwrap_or_default(),
                "content" => parsed.content = string()?.unwrap_or_default(),
                "input" => parsed.input = string()?.unwrap_or_default(),
                "patch" => parsed.patch = string()?.unwrap_or_default(),
                "query" => parsed.query = string()?.unwrap_or_default(),
                "old_string" => parsed.old_string = string()?,
                "new_string" => parsed.new_string = string()?,
                "offset" => {
                    parsed.offset = number::number(raw).map_err(|error| within(error, &name))?
                }
                "limit" => {
                    parsed.limit = number::number(raw).map_err(|error| within(error, &name))?
                }
                "replace_all" => {
                    parsed.replace_all = boolean(raw).map_err(|error| within(error, &name))?
                }
                _ => {}
            }
        }
        Ok(parsed)
    }
}
fn boolean(raw: &RawValue) -> Result<bool> {
    match raw.get() {
        "true" => Ok(true),
        "false" | "null" => Ok(false),
        _ => Err(Error(type_error(raw.get(), "bool"))),
    }
}
fn within(error: Error, key: &str) -> Error {
    let pointer = format!("/{key}");
    if let Some((prefix, detail)) = error.0.split_once(": want a number") {
        Error(format!(
            "{prefix} within {pointer:?}: want a number{detail}"
        ))
    } else {
        Error(format!("{error} within {pointer:?}"))
    }
}
fn count(name: &str, value: Option<f64>) -> Result<i64> {
    let Some(value) = value else {
        return Ok(0);
    };
    if value.fract() != 0.0 || value.abs() > (1u64 << 53) as f64 {
        return Err(Error(format!(
            "argument {name:?} must be a whole number, not {}",
            go_float(value)
        )));
    }
    if value < 0.0 {
        return Err(Error(format!("argument {name:?} must not be negative")));
    }
    Ok(value as i64)
}
fn go_float(value: f64) -> String {
    if value == 0.0 || (1e-4..1e6).contains(&value.abs()) {
        return value.to_string();
    }
    let scientific = format!("{value:e}");
    let (mantissa, power) = scientific
        .split_once('e')
        .expect("finite scientific formatting has an exponent");
    let power: i32 = power.parse().expect("finite f64 exponent fits i32");
    format!("{mantissa}e{power:+03}")
}
