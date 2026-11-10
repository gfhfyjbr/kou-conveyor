use super::*;
use crate::operation::shell::{ShellInput, ShellState, decode_shell, shell_spec};
use chrono::{DateTime, SecondsFormat, Utc};
use kou_domain::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BashConfig {
    pub shell: String,
    pub directory: String,
    pub base_directory: String,
    pub default_timeout: f64,
}
pub struct Bash {
    config: BashConfig,
}
impl Bash {
    pub fn new(config: BashConfig) -> Self {
        Self { config }
    }
    fn arguments(&self, text: &str) -> std::result::Result<(String, f64, usize), CallStatus> {
        let values = arguments::object(text)
            .map_err(|error| error_status(format!("decode Bash arguments: {error}"), 0))?;
        let limit = arguments::output_limit(&values)
            .map_err(|error| error_status(format!("bash argument: {error}"), 0))?;
        let timeout = match values.get("timeout").filter(|raw| raw.get() != "null") {
            Some(value) => {
                let seconds = number::number(value)
                    .map_err(|error| {
                        error_status(format!("decode Bash argument \"timeout\": {error}"), limit)
                    })?
                    .unwrap_or(0.0);
                if !(0.0..=14400.0).contains(&seconds) {
                    return Err(error_status(
                        "bash argument \"timeout\" must be between 0 and 14400 seconds".into(),
                        limit,
                    ));
                }
                seconds
            }
            None => self.config.default_timeout,
        };
        let Some(encoded) = values.get("command") else {
            return Err(error_status(
                "bash argument \"command\" must be set".into(),
                limit,
            ));
        };
        let command = arguments::string(encoded).map_err(|error| {
            error_status(format!("decode Bash argument \"command\": {error}"), limit)
        })?;
        let Some(command) = command else {
            return Err(error_status(
                "bash argument \"command\" must be a string".into(),
                limit,
            ));
        };
        if let Some(offset) = command.find('\0') {
            return Err(error_status(
                format!("bash argument \"command\" contains a NUL byte at offset {offset}"),
                limit,
            ));
        }
        Ok((command, timeout, limit))
    }
    pub fn result_at(
        &self,
        call_id: &str,
        status: &CallStatus,
        operations: &[Arc<Operation>],
        at: Timestamp,
    ) -> Result<ToolResult> {
        if !status.error.is_empty() {
            if !operations.is_empty() {
                return Err(Error(format!(
                    "bash tool call {call_id:?} has both a validation error and operations"
                )));
            }
            return Ok(text_result(call_id, format!("Error: {}", status.error)));
        }
        if operations.len() != 1 {
            return Err(Error(format!(
                "bash tool call {call_id:?} has {} operations, want 1",
                operations.len()
            )));
        }
        let current = &operations[0];
        if current.kind != "shell" {
            return Err(Error(format!(
                "bash tool call {call_id:?} operation {:?} has type {:?}, want \"shell\"",
                current.id, current.kind
            )));
        }
        let state = decode_shell(current).map_err(|error| {
            Error(format!(
                "decode Bash operation {:?} state: {error}",
                current.id
            ))
        })?;
        let text = match current.status.as_str() {
            "ready" | "awaiting" | "canceling" => {
                let elapsed = if state.started_at.is_zero() {
                    String::new()
                } else {
                    let start = DateTime::parse_from_rfc3339(&state.started_at.to_string())
                        .map_err(|error| Error(error.to_string()))?;
                    let end = DateTime::parse_from_rfc3339(&at.to_string())
                        .map_err(|error| Error(error.to_string()))?;
                    let duration = end.signed_duration_since(start);
                    let nanos = duration.num_nanoseconds().unwrap_or(
                        if duration < chrono::TimeDelta::zero() {
                            i64::MIN
                        } else {
                            i64::MAX
                        },
                    );
                    format!(" ({} so far)", seconds(nanos as f64 / 1e9))
                };
                format!(
                    "Command is still running{elapsed}.{}",
                    running_detail(&state)
                )
            }
            "completed" if state.result.is_none() => {
                return Err(Error(format!(
                    "bash tool call {call_id:?} completed operation {:?} has no result",
                    current.id
                )));
            }
            "completed" | "failed" | "canceled" => completed(&state, &current.status),
            _ => {
                return Err(Error(format!(
                    "bash tool call {call_id:?} operation {:?} has invalid status {:?}",
                    current.id, current.status
                )));
            }
        };
        Ok(text_result(call_id, text))
    }
}
impl Translator for Bash {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        let (command, timeout, limit) = match self.arguments(arguments) {
            Ok(value) => value,
            Err(status) => return status,
        };
        let input = ShellInput {
            command,
            shell: self.config.shell.clone(),
            directory: self.config.directory.clone(),
            timeout,
        };
        let spec = match shell_spec(input, self.config.base_directory.clone(), limit as i64) {
            Ok(value) => value,
            Err(error) => return error_status(format!("build Bash operation: {error}"), limit),
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
        let now = DateTime::<Utc>::from(SystemTime::now());
        let now = Timestamp::parse(&now.to_rfc3339_opts(SecondsFormat::Nanos, true))
            .map_err(|error| Error(error.to_string()))?;
        self.result_at(call_id, status, operations, now)
    }
    fn running(&self, operations: &[Arc<Operation>]) -> String {
        let [operation] = operations else {
            return String::new();
        };
        let Ok(state) = decode_shell(operation) else {
            return String::new();
        };
        running_detail(&state).trim().to_owned()
    }
}
fn running_detail(state: &ShellState) -> String {
    let mut text = String::new();
    if !state.out_path.is_empty() {
        text = format!(
            " Its output so far is in {} (stdout) and {} (stderr): read them with tail.",
            state.out_path, state.err_path
        );
    }
    if state.process_group_id > 0 {
        text.push_str(&format!(
            " To stop it: kill -TERM -- -{}",
            state.process_group_id
        ));
    }
    text
}
fn completed(state: &ShellState, status: &str) -> String {
    let mut parts = Vec::new();
    if let Some(result) = &state.result {
        if !result.out.is_empty() {
            parts.push(result.out.clone());
        }
        if !result.err.is_empty() {
            parts.push(format!("Stderr:\n{}", result.err));
        }
        if result.exit_code != 0 {
            parts.push(format!("Exit code: {}", result.exit_code));
        }
        if result.timed_out {
            parts.push(format!(
                "The command was stopped at its timeout of {}.",
                seconds(state.input.timeout)
            ));
        }
        if result.duration >= 2.0 {
            parts.push(format!("Took {}.", seconds(result.duration)));
        }
    } else {
        if !state.out_path.is_empty() {
            parts.push(format!("Stdout capture: {}", state.out_path));
        }
        if !state.err_path.is_empty() {
            parts.push(format!("Stderr capture: {}", state.err_path));
        }
    }
    if !state.terminal_error.is_empty() {
        parts.push(format!("Error: {}", state.terminal_error));
    } else if matches!(status, "failed" | "canceled") {
        parts.push(format!("Error: shell operation {status}"));
    }
    if parts.is_empty() {
        "(no output)".into()
    } else {
        parts.join("\n")
    }
}
fn seconds(value: f64) -> String {
    if value < 10.0 {
        return format!("{value:.1} s");
    }
    if value < 60.0 {
        return format!("{value:.0} s");
    }
    let nanos = (value * 1e9) as i64;
    let rounded = nanos.saturating_add(500_000_000) / 1_000_000_000;
    let hours = rounded / 3600;
    let minutes = rounded % 3600 / 60;
    let seconds = rounded % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else {
        format!("{minutes}m{seconds}s")
    }
}
