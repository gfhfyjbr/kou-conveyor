//! Durable operation transitions are pure. Publish the returned state before
//! executing its dispatch; awaiting Edit/Patch operations fail closed on recovery.
mod file;
mod manager;
pub mod shell;
mod skill;
mod value;

pub use file::{FileResult, FileState, advance_file, decode_file, file_spec, run_file};
pub use manager::{Executor, Limits, LocalManager, NativeFiles, Stats, Update, Updates};
pub use skill::{
    SkillEvent, SkillEventResult, SkillState, SkillStep, advance_skill, decode_skill, skill_spec,
};
pub use value::{advance_value, decode_value, value_spec};

use kou_domain::{Json, Operation};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Spec {
    #[serde(skip_serializing_if = "zero")]
    pub max_output_length: i64,
    #[serde(rename = "Type")]
    pub kind: String,
    pub version: u32,
    pub state: Json,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub idempotency: Json,
}
impl Spec {
    pub fn operation(self, id: String) -> Operation {
        Operation {
            id,
            max_output_length: self.max_output_length,
            kind: self.kind,
            version: self.version,
            status: "ready".into(),
            state: self.state,
            idempotency: self.idempotency,
        }
    }
}

#[derive(Debug)]
pub struct Step {
    pub operation: Operation,
    pub dispatches: Vec<ComputeDispatch>,
}
#[derive(Debug)]
pub struct ComputeDispatch {
    pub source: String,
    pub correlation_id: &'static str,
    pub file: Box<FileState>,
}

#[derive(Debug, Default)]
pub struct Event {
    pub kind: String,
    pub source: String,
    pub result: EventResult,
}
#[derive(Debug, Default)]
pub enum EventResult {
    #[default]
    Empty,
    Failure(String),
    File(FileResult),
    Computed(Json),
    Raw(Json),
}

fn encode(value: &impl Serialize) -> Result<Json> {
    let text = serde_json::to_string(value).map_err(|error| Error(error.to_string()))?;
    Json::parse(&text).map_err(|error| Error(error.to_string()))
}
fn decode_state<T: DeserializeOwned + Default>(value: &Json, name: &str) -> Result<T> {
    let text = value
        .as_str()
        .ok_or_else(|| Error("jsontext: unexpected EOF".into()))?;
    let text = text.trim();
    if text == "null" {
        return Ok(T::default());
    }
    let shape = match text.as_bytes().first() {
        Some(b'[') => Some("array"),
        Some(b'"') => Some("string"),
        Some(b't' | b'f') => Some("boolean"),
        Some(b'-' | b'0'..=b'9') => Some("number"),
        _ => None,
    };
    if let Some(shape) = shape {
        return Err(Error(format!(
            "json: cannot unmarshal JSON {shape} into Go operation.{name}"
        )));
    }
    value.decode().map_err(|error| Error(error.to_string()))
}
fn null_default<'de, D, T>(de: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}
fn zero(value: &i64) -> bool {
    *value == 0
}
fn no(value: &bool) -> bool {
    !value
}
