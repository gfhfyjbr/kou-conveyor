use crate::{Json, Timestamp, null_default};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Session {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Snapshot {
    #[serde(deserialize_with = "null_default")]
    pub session: Session,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Turn {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "PreviousTurnID", deserialize_with = "null_default")]
    pub previous_turn_id: String,
    #[serde(rename = "Type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub model: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Input {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub payload: Json,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub delivery: String,
}

impl Input {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("input ID is empty".into());
        }
        match self.kind.as_str() {
            "external" | "crash" => {}
            "control" => {
                self.control()?;
            }
            _ => return Err(format!("unsupported input kind {:?}", self.kind)),
        }
        match self.delivery.as_str() {
            "" => {}
            "after_tools" if self.kind == "external" => {}
            _ => return Err("unsupported delivery for input kind".into()),
        }
        Ok(())
    }

    pub fn control(&self) -> Result<ControlMessage, String> {
        if self.kind != "control" {
            return Err("input is not a control message".into());
        }
        let control: ControlMessage = self.payload.decode().map_err(|e| e.to_string())?;
        if control.mode != "settings" && !control.parameters.is_missing() {
            return Err("control mode does not accept parameters".into());
        }
        match control.mode.as_str() {
            "hard" | "when_idle" | "compact" => {}
            "heartbeat" if !control.reason.is_empty() => {}
            "settings" => {
                let settings: Settings = control.parameters.decode().map_err(|e| e.to_string())?;
                if !matches!(
                    settings.reasoning_effort.as_str(),
                    "low" | "medium" | "high" | "xhigh" | "max"
                ) {
                    return Err("unsupported reasoning effort".into());
                }
            }
            _ => return Err("unsupported control mode or empty heartbeat reason".into()),
        }
        Ok(control)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "PascalCase")]
pub struct ControlMessage {
    #[serde(deserialize_with = "null_default")]
    pub mode: String,
    #[serde(deserialize_with = "null_default")]
    pub reason: String,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub parameters: Json,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "PascalCase")]
pub struct Settings {
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub reasoning_effort: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Fork {
    #[serde(rename = "ParentID", deserialize_with = "null_default")]
    pub parent_id: String,
    #[serde(rename = "PreviousTurnID", deserialize_with = "null_default")]
    pub previous_turn_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Operation {
    #[serde(skip_serializing_if = "is_zero_i64", deserialize_with = "null_default")]
    pub max_output_length: i64,
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "Type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub version: u32,
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub state: Json,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub idempotency: Json,
}

impl Operation {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.kind.is_empty() || self.version == 0 {
            return Err("operation ID/type is empty or version is zero".into());
        }
        if !matches!(
            self.status.as_str(),
            "ready" | "awaiting" | "canceling" | "completed" | "failed" | "canceled"
        ) {
            return Err(format!("unsupported operation status {:?}", self.status));
        }
        Ok(())
    }

    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "canceled")
    }
}

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CallStatus {
    #[serde(deserialize_with = "null_default")]
    pub error: String,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "null_default")]
    pub error_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<Vec<String>>,
}

impl CallStatus {
    pub fn waiting(&self) -> &[String] {
        self.waiting_for.as_deref().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ToolCallStatus {
    #[serde(rename = "TurnID", deserialize_with = "null_default")]
    pub turn_id: String,
    #[serde(rename = "CallID", deserialize_with = "null_default")]
    pub call_id: String,
    #[serde(deserialize_with = "null_default")]
    pub status: CallStatus,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_default"
    )]
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Usage {
    #[serde(deserialize_with = "null_default")]
    pub input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    pub cached_input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    pub cache_write_input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    pub output_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    pub reasoning_tokens: i64,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub raw: Json,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Failure {
    #[serde(deserialize_with = "null_default")]
    pub code: String,
    #[serde(deserialize_with = "null_default")]
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Response {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub stop: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<LlmItem>>,
    #[serde(deserialize_with = "null_default")]
    pub usage: Usage,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ModelResponse {
    #[serde(rename = "TurnID", deserialize_with = "null_default")]
    pub turn_id: String,
    #[serde(deserialize_with = "null_default")]
    pub response: Response,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compacted: Option<Compacted>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Compacted {
    #[serde(deserialize_with = "null_default")]
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Message {
    #[serde(deserialize_with = "null_default")]
    pub role: String,
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    #[serde(deserialize_with = "null_default")]
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<Image>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Image {
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    pub label: String,
    #[serde(rename = "URL", deserialize_with = "null_default")]
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ToolCall {
    #[serde(rename = "CallID", deserialize_with = "null_default")]
    pub call_id: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub arguments: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ToolResultOutput {
    #[serde(deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ToolResult {
    #[serde(rename = "CallID", deserialize_with = "null_default")]
    pub call_id: String,
    #[serde(deserialize_with = "null_default")]
    pub output: Vec<ToolResultOutput>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub raw: Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmItem {
    pub provider_id: String,
    pub model: String,
    pub data: LlmData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LlmData {
    Message(Message),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Reasoning(Reasoning),
}

impl LlmData {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::ToolCall(_) => "tool_call",
            Self::ToolResult(_) => "tool_result",
            Self::Reasoning(_) => "reasoning",
        }
    }
}
