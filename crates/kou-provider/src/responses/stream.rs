use super::policy::{raw_json, reset_time};
use super::{
    decode_response,
    response::{optional_raw, text},
};
use crate::{Error, Result, null_default, object_or_null, sse};
use kou_domain::{Json, Response, validate_json};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use std::{borrow::Cow, collections::BTreeMap, fmt, time::SystemTime};

#[derive(Clone, Copy, Debug)]
pub struct StreamLimits {
    pub frame_bytes: usize,
    pub retained_bytes: usize,
    pub output_items: usize,
}
impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            frame_bytes: 16 << 20,
            retained_bytes: 32 << 20,
            output_items: 4096,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct StreamEvent {
    pub index: i64,
    pub kind: String,
    pub text: String,
    pub reset: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ApiError {
    pub status_code: u16,
    pub code: String,
    pub message: String,
    pub param: String,
    pub kind: String,
    pub resets_at: Json,
    pub resets_in_seconds: Json,
    pub reset_at: Option<SystemTime>,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.code.is_empty() {
            write!(f, "responses API error {}: {}", self.code, self.message)
        } else if self.status_code != 0 {
            write!(
                f,
                "responses API request failed with status {}: {}",
                self.status_code, self.message
            )
        } else {
            write!(f, "responses API request failed: {}", self.message)
        }
    }
}
impl std::error::Error for ApiError {}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Event<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    kind: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    response: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    item: Option<&'a RawValue>,
    output_index: Option<i64>,
    #[serde(borrow, deserialize_with = "text")]
    code: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    message: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    param: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "crate::optional_object")]
    error: Option<WireError<'a>>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    resets_at: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    resets_in_seconds: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct WireError<'a> {
    #[serde(borrow, deserialize_with = "text")]
    pub(super) code: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub(super) message: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub(super) param: Cow<'a, str>,
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    pub(super) kind: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    pub(super) resets_at: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    pub(super) resets_in_seconds: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Piece<'a> {
    #[serde(borrow, deserialize_with = "text")]
    delta: Cow<'a, str>,
    #[serde(deserialize_with = "null_default")]
    summary_index: i64,
}

pub struct ResponseState {
    limits: StreamLimits,
    output: BTreeMap<i64, Box<RawValue>>,
    retained: usize,
    terminal: bool,
    failed_terminal: bool,
    body: Option<Box<RawValue>>,
    error: Option<ApiError>,
}
impl ResponseState {
    pub fn new(limits: StreamLimits) -> Result<Self> {
        if limits.frame_bytes == 0 || limits.retained_bytes == 0 || limits.output_items == 0 {
            return Err(Error("invalid response stream limits".into()));
        }
        Ok(Self {
            limits,
            output: BTreeMap::new(),
            retained: 0,
            terminal: false,
            failed_terminal: false,
            body: None,
            error: None,
        })
    }
    pub fn is_finished(&self) -> bool {
        self.terminal || self.error.is_some()
    }
    pub fn api_error(&self) -> Option<&ApiError> {
        self.error.as_ref()
    }
    pub fn failed_terminal(&self) -> bool {
        self.failed_terminal
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
    pub fn observe(&mut self, raw: &[u8], mut report: impl FnMut(StreamEvent)) -> Result<()> {
        if let Some(error) = &self.error {
            return Err(Error(error.to_string()));
        }
        if self.terminal {
            return Ok(());
        }
        validate_json(raw)
            .map_err(|e| Error(format!("invalid Responses stream event JSON: {e}")))?;
        let event: Event<'_> = object_or_null(&mut serde_json::Deserializer::from_slice(raw))
            .map_err(|e| Error(format!("invalid Responses stream event JSON: {e}")))?;
        if let Some(index) = event.output_index.filter(|index| *index >= 0) {
            let kind = match event.kind.as_ref() {
                "response.output_text.delta" => Some("text"),
                "response.reasoning_summary_text.delta"
                | "response.reasoning_summary_part.added" => Some("reasoning"),
                _ => None,
            };
            if let Some(kind) = kind
                && let Ok(piece) =
                    object_or_null::<_, Piece<'_>>(&mut serde_json::Deserializer::from_slice(raw))
            {
                let paragraph = event.kind == "response.reasoning_summary_part.added";
                if !paragraph || piece.summary_index > 0 {
                    report(StreamEvent {
                        index,
                        kind: kind.into(),
                        text: if paragraph {
                            "\n\n".into()
                        } else {
                            piece.delta.into_owned()
                        },
                        reset: false,
                    });
                }
            }
        }
        match event.kind.as_ref() {
            "response.output_item.done" => {
                if let (Some(index), Some(item)) = (
                    event.output_index.filter(|index| *index >= 0),
                    event.item.filter(|item| item.get().starts_with('{')),
                ) {
                    if !self.output.contains_key(&index)
                        && self.output.len() >= self.limits.output_items
                    {
                        return Err(Error(
                            "response stream exceeds the output-item limit".into(),
                        ));
                    }
                    let previous = self.output.get(&index).map_or(0, |value| value.get().len());
                    let remaining = self.limits.retained_bytes - (self.retained - previous);
                    if item.get().len() > remaining {
                        return Err(Error(
                            "response stream exceeds the retained-byte limit".into(),
                        ));
                    }
                    self.retained = self.retained - previous + item.get().len();
                    self.output.insert(index, item.to_owned());
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let size = event.response.map_or(0, |value| value.get().len());
                if size > self.limits.retained_bytes - self.retained {
                    return Err(Error(
                        "response stream exceeds the retained-byte limit".into(),
                    ));
                }
                self.body = event.response.map(ToOwned::to_owned);
                self.retained += size;
                self.terminal = true;
                self.failed_terminal = event.kind == "response.failed";
            }
            "error" => {
                let nested = event.error.unwrap_or_default();
                let now = SystemTime::now();
                let reset_at = reset_time(
                    &raw_json(event.resets_at),
                    &raw_json(event.resets_in_seconds),
                    now,
                )
                .or_else(|| {
                    reset_time(
                        &raw_json(nested.resets_at),
                        &raw_json(nested.resets_in_seconds),
                        now,
                    )
                });
                let code = if event.code.is_empty() {
                    nested.code
                } else {
                    event.code
                };
                let message = if event.message.is_empty() {
                    nested.message
                } else {
                    event.message
                };
                let param = if event.param.is_empty() {
                    nested.param
                } else {
                    event.param
                };
                let kind = if nested.kind.is_empty() {
                    event.kind
                } else {
                    nested.kind
                };
                self.error = Some(ApiError {
                    status_code: 200,
                    code: code.into_owned(),
                    message: message.into_owned(),
                    param: param.into_owned(),
                    kind: kind.into_owned(),
                    resets_at: raw_json(event.resets_at.or(nested.resets_at)),
                    resets_in_seconds: raw_json(
                        event.resets_in_seconds.or(nested.resets_in_seconds),
                    ),
                    reset_at,
                });
            }
            _ => {}
        }
        Ok(())
    }
    pub fn into_body(self) -> Result<Box<RawValue>> {
        if let Some(error) = self.error {
            return Err(Error(error.to_string()));
        }
        let body = self.body.ok_or_else(unexpected_eof)?;
        if !body.get().starts_with('{') {
            return Err(unexpected_eof());
        }
        if self.output.is_empty() {
            return Ok(body);
        }
        let fields: BTreeMap<String, &RawValue> = serde_json::from_str(body.get())
            .map_err(|e| Error(format!("decode terminal response: {e}")))?;
        let output: Option<Vec<&RawValue>> = fields
            .get("output")
            .map(|value| serde_json::from_str(value.get()))
            .transpose()
            .map_err(|e| Error(format!("decode terminal output: {e}")))?
            .flatten();
        if output.is_some_and(|output| !output.is_empty()) {
            return Ok(body);
        }
        let mut fields: BTreeMap<&str, Field<'_>> = fields
            .iter()
            .map(|(key, value)| (key.as_str(), Field::Raw(value)))
            .collect();
        fields.insert("output", Field::Items(&self.output));
        let encoded = serde_json::to_string(&fields)
            .map_err(|e| Error(format!("encode terminal response: {e}")))?;
        if encoded.len() > self.limits.retained_bytes {
            return Err(Error(
                "assembled response exceeds the retained-byte limit".into(),
            ));
        }
        RawValue::from_string(encoded).map_err(|e| Error(e.to_string()))
    }
    pub fn into_response(self) -> Result<Response> {
        let body = self.into_body()?;
        decode_response(body.get().as_bytes())
    }
}
fn unexpected_eof() -> Error {
    Error("responses stream ended without a terminal response: unexpected EOF".into())
}
enum Field<'a> {
    Raw(&'a RawValue),
    Items(&'a BTreeMap<i64, Box<RawValue>>),
}
impl Serialize for Field<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Raw(value) => value.serialize(serializer),
            Self::Items(items) => serializer.collect_seq(items.values().map(Box::as_ref)),
        }
    }
}

pub struct StreamDecoder {
    framer: sse::Framer,
    state: ResponseState,
}
impl StreamDecoder {
    pub fn new(limits: StreamLimits) -> Result<Self> {
        Ok(Self {
            framer: sse::Framer::new(limits.frame_bytes)?,
            state: ResponseState::new(limits)?,
        })
    }
    pub fn state(&self) -> &ResponseState {
        &self.state
    }
    pub fn feed(&mut self, bytes: &[u8], mut report: impl FnMut(StreamEvent)) -> Result<bool> {
        if self.state.is_finished() {
            return Ok(true);
        }
        let state = &mut self.state;
        self.framer.feed(bytes, |frame| {
            let Some(data) = sse::data(frame) else {
                return Ok(false);
            };
            let data = std::str::from_utf8(&data)
                .map_err(|e| Error(format!("invalid Responses stream event JSON: {e}")))?
                .trim();
            if data.is_empty() || data == "[DONE]" {
                return Ok(false);
            }
            state.observe(data.as_bytes(), &mut report)?;
            Ok(state.is_finished())
        })
    }
    pub fn into_body(mut self) -> Result<Box<RawValue>> {
        self.framer.discard_partial();
        drop(self.framer);
        self.state.into_body()
    }
    pub fn finish(self) -> Result<Response> {
        let body = self.into_body()?;
        decode_response(body.get().as_bytes())
    }
}
