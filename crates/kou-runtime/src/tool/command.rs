use super::*;
use crate::operation::shell::{ShellInput, shell_spec};
use kou_domain::{ToolCall, validate_json, write_compact_json};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

const CALL_ID: &str = "KOU_CONVEYOR_TOOL_CALL_ID";
const TOOL_NAME: &str = "KOU_CONVEYOR_TOOL_NAME";
const DELIMITER: &str = "KOU_CONVEYOR_TOOL_ARGUMENTS";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CommandConfig {
    pub name: String,
    pub command: Vec<String>,
    pub directory: String,
    pub base_directory: String,
    pub environment: BTreeMap<String, String>,
    pub max_output_length: i64,
    pub waits: bool,
}

pub struct Command {
    config: CommandConfig,
}
impl Command {
    pub fn new(mut config: CommandConfig) -> Result<Self> {
        if config.command.first().is_none_or(String::is_empty) {
            return Err(Error(format!("tool {:?} has no command", config.name)));
        }
        for name in config.environment.keys() {
            let mut bytes = name.bytes();
            if !bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(Error(format!(
                    "tool {:?}: {name:?} is not a variable name",
                    config.name
                )));
            }
        }
        if config.max_output_length == 0 {
            config.max_output_length = DEFAULT_MAX_OUTPUT_LENGTH as i64;
        }
        Ok(Self { config })
    }
    fn translate_with_id(
        &self,
        call_id: &str,
        arguments: &str,
        submit: &mut dyn FnMut(Spec) -> String,
    ) -> CallStatus {
        let text = arguments.trim();
        let text = if text.is_empty() { "{}" } else { text };
        let limit = self.config.max_output_length.max(0) as usize;
        if !text.starts_with('{') || validate_json(text.as_bytes()).is_err() {
            return error_status(
                format!("{} arguments must be a JSON object", self.config.name),
                limit,
            );
        }
        let script = match self.script(call_id, text) {
            Ok(script) => script,
            Err(error) => {
                return error_status(
                    format!("encode {} arguments: {error}", self.config.name),
                    limit,
                );
            }
        };
        let spec = match shell_spec(
            ShellInput {
                command: script,
                shell: "/bin/sh".into(),
                directory: self.config.directory.clone(),
                timeout: 0.0,
            },
            self.config.base_directory.clone(),
            self.config.max_output_length,
        ) {
            Ok(spec) => spec,
            Err(error) => {
                return error_status(
                    format!("build {} operation: {error}", self.config.name),
                    limit,
                );
            }
        };
        CallStatus {
            waiting_for: Some(vec![submit(spec)]),
            ..CallStatus::default()
        }
    }
    fn script(&self, call_id: &str, text: &str) -> Result<String> {
        let mut environment: Vec<_> = self
            .config
            .environment
            .iter()
            .filter(|(key, _)| key.as_str() != CALL_ID && key.as_str() != TOOL_NAME)
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .chain([(CALL_ID, call_id), (TOOL_NAME, self.config.name.as_str())])
            .collect();
        environment.sort_unstable_by_key(|(key, _)| *key);
        let mut script = String::from("export");
        for (key, value) in environment {
            script.push(' ');
            script.push_str(key);
            script.push('=');
            quote(&mut script, value);
        }
        script.push_str("\nexec");
        for argument in &self.config.command {
            script.push(' ');
            quote(&mut script, argument);
        }
        script.push_str(" <<'");
        script.push_str(DELIMITER);
        script.push_str("'\n");
        let raw: &RawValue =
            serde_json::from_str(text).map_err(|error| Error(error.to_string()))?;
        let mut script = script.into_bytes();
        write_compact_json(&mut script, raw).map_err(|error| Error(error.to_string()))?;
        script.push(b'\n');
        script.extend_from_slice(DELIMITER.as_bytes());
        script.push(b'\n');
        String::from_utf8(script).map_err(|error| Error(error.to_string()))
    }
}
impl Translator for Command {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        self.translate_with_id("", arguments, submit)
    }
    fn translate_call(
        &self,
        call: &ToolCall,
        submit: &mut dyn FnMut(Spec) -> String,
    ) -> CallStatus {
        self.translate_with_id(&call.call_id, &call.arguments, submit)
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
                    self.config.name
                )));
            }
            return Ok(text_result(call_id, format!("Error: {}", status.error)));
        }
        if operations.len() != 1 {
            return Err(Error(format!(
                "{} call {call_id:?} has {} operations, want 1",
                self.config.name,
                operations.len()
            )));
        }
        Bash::new(BashConfig::default()).result(call_id, status, operations)
    }
    fn waits(&self) -> bool {
        self.config.waits
    }
}
fn quote(output: &mut String, word: &str) {
    output.push('\'');
    let mut parts = word.split('\'');
    output.push_str(parts.next().unwrap_or_default());
    for part in parts {
        output.push_str("'\\''");
        output.push_str(part);
    }
    output.push('\'');
}
