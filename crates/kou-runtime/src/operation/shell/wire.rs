use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ShellInput {
    #[serde(deserialize_with = "null_default")]
    pub command: String,
    #[serde(deserialize_with = "null_default")]
    pub shell: String,
    #[serde(deserialize_with = "null_default")]
    pub directory: String,
    #[serde(skip_serializing_if = "zero_float", deserialize_with = "null_default")]
    pub timeout: f64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ShellResult {
    #[serde(deserialize_with = "null_default")]
    pub out: String,
    #[serde(deserialize_with = "null_default")]
    pub err: String,
    #[serde(deserialize_with = "null_default")]
    pub out_size: i64,
    #[serde(deserialize_with = "null_default")]
    pub err_size: i64,
    #[serde(deserialize_with = "null_default")]
    pub exit_code: i64,
    #[serde(
        skip_serializing_if = "super::super::no",
        deserialize_with = "null_default"
    )]
    pub timed_out: bool,
    #[serde(skip_serializing_if = "zero_float", deserialize_with = "null_default")]
    pub duration: f64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ShellState {
    #[serde(deserialize_with = "object_input")]
    pub input: ShellInput,
    #[serde(deserialize_with = "null_default")]
    pub base_directory: String,
    #[serde(deserialize_with = "null_default")]
    pub phase: String,
    #[serde(rename = "ProcessGroupID", deserialize_with = "null_default")]
    pub process_group_id: i64,
    #[serde(skip_serializing_if = "Timestamp::is_zero")]
    pub started_at: Timestamp,
    #[serde(
        skip_serializing_if = "super::super::no",
        deserialize_with = "null_default"
    )]
    pub timed_out: bool,
    pub pending_exit_code: Option<i64>,
    #[serde(deserialize_with = "null_default")]
    pub out_size: i64,
    #[serde(deserialize_with = "null_default")]
    pub err_size: i64,
    #[serde(with = "bytes")]
    pub inline_out: Vec<u8>,
    #[serde(with = "bytes")]
    pub inline_err: Vec<u8>,
    #[serde(with = "bytes")]
    pub inline_out_tail: Vec<u8>,
    #[serde(with = "bytes")]
    pub inline_err_tail: Vec<u8>,
    #[serde(deserialize_with = "object_result")]
    pub result: Option<ShellResult>,
    #[serde(deserialize_with = "null_default")]
    pub terminal_error: String,
    #[serde(deserialize_with = "null_default")]
    pub error_truncated: bool,
    #[serde(deserialize_with = "null_default")]
    pub out_truncated: bool,
    #[serde(deserialize_with = "null_default")]
    pub err_truncated: bool,
    #[serde(deserialize_with = "null_default")]
    pub out_path: String,
    #[serde(deserialize_with = "null_default")]
    pub err_path: String,
}
impl ShellState {
    pub fn validate(&self) -> Result<()> {
        let error = if !std::path::Path::new(&self.input.shell).is_absolute() {
            Some("shell path must be absolute")
        } else if !std::path::Path::new(&self.base_directory).is_absolute() {
            Some("base directory must be absolute")
        } else if self.process_group_id < 0 || self.process_group_id == 1 {
            Some("shell process group ID must be zero or greater than one")
        } else if self.out_size < 0 || self.err_size < 0 {
            Some("captured output sizes must not be negative")
        } else if !self.input.timeout.is_finite() || !(0.0..=14400.0).contains(&self.input.timeout)
        {
            Some("the timeout must be between 0 and 14400 seconds")
        } else {
            None
        };
        error.map_or(Ok(()), |text| Err(Error(text.into())))
    }
}
fn zero_float(value: &f64) -> bool {
    *value == 0.0
}
fn object_input<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<ShellInput, D::Error> {
    let value = Json::deserialize(de)?;
    decode_state(&value, "ShellInput").map_err(serde::de::Error::custom)
}
fn object_result<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<Option<ShellResult>, D::Error> {
    let value = Json::deserialize(de)?;
    if value.as_str() == Some("null") {
        return Ok(None);
    }
    decode_state(&value, "ShellResult")
        .map(Some)
        .map_err(serde::de::Error::custom)
}

pub(crate) mod bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &[u8],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Vec<u8>, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a string")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(Vec::new())
            }
            fn visit_str<E: serde::de::Error>(
                self,
                text: &str,
            ) -> std::result::Result<Self::Value, E> {
                decode(text).map_err(E::custom)
            }
        }
        deserializer.deserialize_any(Bytes)
    }
    pub(crate) fn decode(text: &str) -> std::result::Result<Vec<u8>, String> {
        let cleaned;
        let value = if text.contains(['\r', '\n']) {
            cleaned = text.replace(['\r', '\n'], "");
            &cleaned
        } else {
            text
        };
        let decoded = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
        )
        .decode(value)
        .map_err(|error| error.to_string())?;
        // encoding/json/v2 rejects CR/LF even though encoding/base64 permits it,
        // after checking that decoding itself succeeded (arshal_default.go).
        if let Some((offset, byte)) = text
            .bytes()
            .enumerate()
            .find(|(_, byte)| matches!(byte, b'\r' | b'\n'))
        {
            return Err(format!(
                "illegal character {:?} at offset {offset}",
                byte as char
            ));
        }
        Ok(decoded)
    }
}

// Only the failure path rescans fields to attach Go's exact JSON pointer. The
// visitor preserves source order; successful state decoding incurs no extra pass.
pub(super) fn binary_error(raw: &Json) -> Option<String> {
    struct Fields;
    impl<'de> serde::de::Visitor<'de> for Fields {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("shell state object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut found = None;
            while let Some((key, value)) =
                map.next_entry::<String, &'de serde_json::value::RawValue>()?
            {
                if found.is_none()
                    && matches!(
                        key.as_str(),
                        "InlineOut" | "InlineErr" | "InlineOutTail" | "InlineErrTail"
                    )
                    && let Ok(Some(text)) = serde_json::from_str::<Option<String>>(value.get())
                    && let Err(error) = bytes::decode(&text)
                    && error.starts_with("illegal character ")
                {
                    found = Some(format!(
                        "json: cannot unmarshal JSON string into Go []uint8 within \"/{key}\": {error}"
                    ));
                }
            }
            Ok(found)
        }
    }
    let mut de = serde_json::Deserializer::from_str(raw.as_str()?);
    serde::Deserializer::deserialize_map(&mut de, Fields)
        .ok()
        .flatten()
}

#[derive(Debug, Default)]
pub struct ShellEvent {
    pub kind: String,
    pub source: String,
    pub correlation_id: String,
    pub result: ShellEventResult,
}
#[derive(Debug, Default)]
pub enum ShellEventResult {
    #[default]
    Empty,
    Created {
        kind: u8,
    },
    Started {
        pid: i64,
    },
    Exited {
        exit_code: i64,
        signal: i64,
    },
    ReadOutput {
        offset: i64,
        data: Vec<u8>,
    },
    ReadCompleted {
        size: i64,
    },
    Failure(String),
    Raw(Json),
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateRequest {
    pub source: String,
    #[serde(rename = "CorrelationID")]
    pub correlation_id: String,
    pub kind: u8,
    pub path: String,
    pub mode: u32,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReadRequest {
    pub source: String,
    #[serde(rename = "CorrelationID")]
    pub correlation_id: String,
    pub path: String,
    pub offset: i64,
    pub count: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartRequest {
    pub source: String,
    #[serde(rename = "CorrelationID")]
    pub correlation_id: String,
    pub path: String,
    pub arguments: Vec<String>,
    pub directory: String,
    pub environment: Vec<String>,
    pub pipes: u8,
    pub stdout_path: String,
    pub stderr_path: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimerRequest {
    pub source: String,
    #[serde(rename = "CorrelationID")]
    pub correlation_id: String,
    pub deadline: Timestamp,
}
#[derive(Debug, Serialize)]
#[serde(tag = "Type", content = "Data")]
pub enum ShellDispatch {
    #[serde(rename = "io.create")]
    Create(CreateRequest),
    #[serde(rename = "io.read")]
    Read(ReadRequest),
    #[serde(rename = "process.start")]
    Start(StartRequest),
    #[serde(rename = "timer.schedule")]
    Timer(TimerRequest),
    /// Unlike Go's reducer, no signal is sent here. Execution must resolve this
    /// against the currently owned invocation, never a recovered numeric PID.
    #[serde(rename = "process.signal")]
    Signal { group_id: i64, signal: i32 },
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ShellStep {
    pub operation: Option<Operation>,
    pub dispatches: Vec<ShellDispatch>,
}
