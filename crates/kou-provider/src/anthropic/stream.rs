use super::response::{self, Block, Envelope};
use crate::{
    Error, Result, null_default,
    responses::{StreamEvent, optional_raw, text},
};
use kou_domain::{Json, Response, Usage};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{borrow::Cow, collections::BTreeMap};

pub const UNEXPECTED_EOF: &str = "Messages API stream ended before the message did: unexpected EOF";
#[derive(Clone, Copy, Debug)]
pub struct StreamLimits {
    pub frame_bytes: usize,
    pub retained_bytes: usize,
    pub content_blocks: usize,
}
impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            frame_bytes: 16 << 20,
            retained_bytes: 32 << 20,
            content_blocks: 4096,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Event<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    kind: Cow<'a, str>,
    #[serde(deserialize_with = "null_default")]
    index: i64,
    #[serde(borrow, deserialize_with = "optional_raw")]
    message: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    content_block: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    delta: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    usage: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Delta<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    kind: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    text: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    thinking: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    signature: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    partial_json: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    stop_reason: Cow<'a, str>,
}
pub struct StreamDecoder {
    limits: StreamLimits,
    frame: Vec<u8>,
    line_start: usize,
    state: State,
    failed: bool,
}
#[derive(Default)]
struct State {
    id: String,
    stop_reason: String,
    blocks: Vec<Block<'static>>,
    initial_inputs: BTreeMap<i64, String>,
    inputs: BTreeMap<i64, String>,
    sdk_inputs: Vec<SdkInput>,
    usage: Usage,
    raw_usage: Option<BTreeMap<String, Json>>,
    retained: usize,
    complete: bool,
    api_error: Option<Box<[u8]>>,
}
#[derive(Default)]
struct SdkInput {
    start: usize,
    initial: bool,
    placeholder: bool,
}
impl SdkInput {
    fn view<'a>(&self, initial: Option<&'a String>, streamed: Option<&'a String>) -> Cow<'a, str> {
        if self.placeholder {
            return Cow::Borrowed("{}");
        }
        let streamed = streamed.map_or("", String::as_str);
        let streamed = &streamed[self.start..];
        let initial = if self.initial {
            initial.map_or("", String::as_str)
        } else {
            ""
        };
        if initial.is_empty() {
            Cow::Borrowed(streamed)
        } else if streamed.is_empty() {
            Cow::Borrowed(initial)
        } else {
            Cow::Owned(format!("{initial}{streamed}"))
        }
    }
}
impl StreamDecoder {
    pub fn new(limits: StreamLimits) -> Result<Self> {
        if limits.frame_bytes == 0 || limits.retained_bytes == 0 || limits.content_blocks == 0 {
            return Err(Error("invalid Messages API stream limits".into()));
        }
        Ok(Self {
            limits,
            frame: Vec::new(),
            line_start: 0,
            state: State::default(),
            failed: false,
        })
    }
    pub fn api_error(&self) -> Option<&[u8]> {
        self.state.api_error.as_deref()
    }
    pub fn is_complete(&self) -> bool {
        self.state.complete
    }
    pub fn retained_bytes(&self) -> usize {
        self.state.retained
    }
    pub fn feed(&mut self, mut bytes: &[u8], mut report: impl FnMut(StreamEvent)) -> Result<bool> {
        if self.failed {
            return Err(Error("Messages API stream decoder has failed".into()));
        }
        if self.state.api_error.is_some() {
            return Ok(true);
        }
        let result = (|| {
            while !bytes.is_empty() {
                let end = bytes.iter().position(|byte| *byte == b'\n');
                let count = end.map_or(bytes.len(), |index| index + 1);
                if count > self.limits.frame_bytes - self.frame.len() {
                    return Err(Error(
                        "Messages API SSE frame exceeds the frame-byte limit".into(),
                    ));
                }
                let required = self.frame.len() + count;
                if required > self.frame.capacity() {
                    let target = required
                        .max(self.frame.capacity().saturating_mul(2))
                        .min(self.limits.frame_bytes);
                    self.frame.reserve_exact(target - self.frame.len());
                }
                self.frame.extend_from_slice(&bytes[..count]);
                bytes = &bytes[count..];
                if end.is_none() {
                    break;
                }
                let line = &self.frame[self.line_start..self.frame.len() - 1];
                if line.strip_suffix(b"\r").unwrap_or(line).is_empty() {
                    if let Some((kind, data)) = event_data(&self.frame[..self.line_start]) {
                        if kind == b"error" {
                            self.state.api_error = Some(data.into_owned().into_boxed_slice());
                            self.frame.clear();
                            self.line_start = 0;
                            return Ok(true);
                        }
                        if matches!(
                            kind,
                            b"message_start"
                                | b"message_delta"
                                | b"message_stop"
                                | b"content_block_start"
                                | b"content_block_delta"
                                | b"content_block_stop"
                                | b"completion"
                        ) {
                            self.state.observe(&data, self.limits, &mut report)?;
                        }
                    }
                    self.frame.clear();
                    self.line_start = 0;
                } else {
                    self.line_start = self.frame.len();
                }
            }
            Ok(false)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn finish(self) -> Result<Response> {
        if !self.state.complete {
            return Err(Error(UNEXPECTED_EOF.into()));
        }
        self.finish_partial()
    }
    pub(super) fn finish_partial(mut self) -> Result<Response> {
        self.frame = Vec::new();
        if self.failed {
            return Err(Error("Messages API stream decoder has failed".into()));
        }
        if self.state.api_error.is_some() {
            return Err(Error("Messages API stream contains an API error".into()));
        }
        // The Go SDK accepts duplicate tool-input keys, but its enclosing raw
        // message then fails the adapter's stricter usage JSON extraction.
        let raw_usage_valid = self
            .state
            .sdk_inputs
            .iter()
            .enumerate()
            .all(|(index, input)| {
                let raw = input.view(
                    self.state.initial_inputs.get(&(index as i64)),
                    self.state.inputs.get(&(index as i64)),
                );
                raw.is_empty() || kou_domain::validate_json(raw.as_bytes()).is_ok()
            });
        for (index, input) in self.state.initial_inputs {
            self.state.inputs.entry(index).or_insert(input);
        }
        if let Some(raw) = self.state.raw_usage.filter(|_| raw_usage_valid) {
            self.state.usage.raw = Json::parse(
                &serde_json::to_string(&raw).map_err(|error| Error(error.to_string()))?,
            )
            .map_err(|error| Error(error.to_string()))?;
        }
        response::from_blocks(
            self.state.id,
            &self.state.stop_reason,
            self.state.blocks,
            &self.state.inputs,
            self.state.usage,
        )
    }
}
fn event_data(frame: &[u8]) -> Option<(&[u8], Cow<'_, [u8]>)> {
    let mut kind = &b""[..];
    let mut data = None;
    for line in frame.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let (key, value) = line
            .iter()
            .position(|byte| *byte == b':')
            .map_or((line, &b""[..]), |at| (&line[..at], &line[at + 1..]));
        let value = value.strip_prefix(b" ").unwrap_or(value);
        match key {
            b"event" => kind = value,
            b"data" => {
                data = Some(match data {
                    None => Cow::Borrowed(value),
                    Some(Cow::Borrowed(previous)) => {
                        let mut joined = Vec::with_capacity(frame.len());
                        joined.extend_from_slice(previous);
                        joined.push(b'\n');
                        joined.extend_from_slice(value);
                        Cow::Owned(joined)
                    }
                    Some(Cow::Owned(mut previous)) => {
                        previous.push(b'\n');
                        previous.extend_from_slice(value);
                        Cow::Owned(previous)
                    }
                })
            }
            _ => {}
        }
    }
    data.map(|data| (kind, data))
}
impl State {
    fn charge(&mut self, bytes: usize, limits: StreamLimits) -> Result<()> {
        if bytes > limits.retained_bytes - self.retained {
            return Err(Error(
                "Messages API stream exceeds the retained-byte limit".into(),
            ));
        }
        self.retained += bytes;
        Ok(())
    }
    fn index(&self, kind: &str, index: i64) -> Result<usize> {
        let index_usize = usize::try_from(index)
            .ok()
            .filter(|index| *index < self.blocks.len());
        index_usize.ok_or_else(||Error(format!("read Messages API stream: received event of type {kind} for content block at index {index} but there are only {} content blocks",self.blocks.len())))
    }
    fn add_block(&mut self, raw: &RawValue, limits: StreamLimits) -> Result<()> {
        if self.blocks.len() >= limits.content_blocks {
            return Err(Error(
                "Messages API stream exceeds the content-block limit".into(),
            ));
        }
        let block: Block<'_> = response::decode(raw.get().as_bytes())?;
        let size = block.kind.len()
            + block.text.len()
            + block.thinking.len()
            + block.signature.len()
            + block.data.len()
            + block.id.len()
            + block.name.len()
            + block.input.map_or(0, |raw| raw.get().len());
        self.charge(size, limits)?;
        if let Some(input) = block.input.filter(|raw| raw.get() != "null") {
            self.initial_inputs
                .insert(self.blocks.len() as i64, input.get().into());
        }
        self.sdk_inputs.push(SdkInput {
            start: self
                .inputs
                .get(&(self.blocks.len() as i64))
                .map_or(0, String::len),
            initial: true,
            placeholder: false,
        });
        self.blocks.push(Block {
            kind: Cow::Owned(block.kind.into_owned()),
            text: Cow::Owned(block.text.into_owned()),
            thinking: Cow::Owned(block.thinking.into_owned()),
            signature: Cow::Owned(block.signature.into_owned()),
            data: Cow::Owned(block.data.into_owned()),
            id: Cow::Owned(block.id.into_owned()),
            name: Cow::Owned(block.name.into_owned()),
            input: None,
        });
        Ok(())
    }
    fn refresh_input(&mut self, index: usize) {
        let id = index as i64;
        let raw = self.sdk_inputs[index].view(self.initial_inputs.get(&id), self.inputs.get(&id));
        if !raw.is_empty() && serde_json::from_str::<&RawValue>(&raw).is_err() {
            self.sdk_inputs[index] = SdkInput {
                start: self.inputs.get(&id).map_or(0, String::len),
                initial: false,
                placeholder: true,
            };
        }
    }
    fn observe(
        &mut self,
        data: &[u8],
        limits: StreamLimits,
        report: &mut impl FnMut(StreamEvent),
    ) -> Result<()> {
        let event: Event<'_> = response::decode(data)?;
        match event.kind.as_ref() {
            "message_start" => {
                let raw = event.message.map_or("null", RawValue::get);
                let message: Envelope<'_> = response::decode(raw.as_bytes())?;
                // A replacement start resets the SDK message, but not the adapter's exact-input map.
                self.id = message.id.into_owned();
                self.stop_reason = message.stop_reason.into_owned();
                self.blocks.clear();
                self.sdk_inputs.clear();
                self.initial_inputs.clear();
                self.retained = self.inputs.values().map(String::len).sum();
                self.charge(self.id.len() + self.stop_reason.len(), limits)?;
                self.usage = response::usage_from_raw(message.usage.map(RawValue::get))?;
                self.usage.raw = Json::default();
                self.raw_usage = None;
                if let Some(raw) = message
                    .usage
                    .filter(|raw| raw.get().trim_start().starts_with('{'))
                {
                    self.charge(raw.get().len(), limits)?;
                    self.raw_usage = Some(
                        serde_json::from_str(raw.get())
                            .map_err(|error| Error(error.to_string()))?,
                    );
                }
                for raw in message.content {
                    self.add_block(raw, limits)?;
                }
            }
            "content_block_start" => {
                if event.index != self.blocks.len() as i64 {
                    return Err(Error(format!(
                        "read Messages API stream: received event of type content_block_start for content block at index {}, expected index {}",
                        event.index,
                        self.blocks.len()
                    )));
                }
                let empty = RawValue::from_string("null".into()).unwrap();
                self.add_block(event.content_block.unwrap_or(&empty), limits)?;
            }
            "content_block_delta" => {
                let index = self.index("content_block_delta", event.index)?;
                let delta: Delta<'_> =
                    response::decode(event.delta.map_or("null", RawValue::get).as_bytes())?;
                let (value, kind) = match delta.kind.as_ref() {
                    "text_delta" => (delta.text, Some("text")),
                    "thinking_delta" => (delta.thinking, Some("reasoning")),
                    "signature_delta" => (delta.signature, None),
                    "input_json_delta" => (delta.partial_json, None),
                    _ => return Ok(()),
                };
                self.charge(value.len(), limits)?;
                match delta.kind.as_ref() {
                    "text_delta" => self.blocks[index].text.to_mut().push_str(&value),
                    "thinking_delta" => self.blocks[index].thinking.to_mut().push_str(&value),
                    "signature_delta" => self.blocks[index].signature.to_mut().push_str(&value),
                    "input_json_delta" => {
                        if !value.is_empty()
                            && self.sdk_inputs[index].view(
                                self.initial_inputs.get(&event.index),
                                self.inputs.get(&event.index),
                            ) == "{}"
                        {
                            self.sdk_inputs[index] = SdkInput {
                                start: self.inputs.get(&event.index).map_or(0, String::len),
                                initial: false,
                                placeholder: false,
                            };
                        }
                        self.inputs.entry(event.index).or_default().push_str(&value);
                    }
                    _ => unreachable!(),
                }
                if let Some(kind) = kind {
                    report(StreamEvent {
                        index: event.index,
                        kind: kind.into(),
                        text: value.into_owned(),
                        reset: false,
                    });
                }
            }
            "content_block_stop" => {
                let index = self.index("content_block_stop", event.index)?;
                self.refresh_input(index);
            }
            "message_delta" => {
                let delta: Delta<'_> =
                    response::decode(event.delta.map_or("null", RawValue::get).as_bytes())?;
                self.charge(delta.stop_reason.len(), limits)?;
                self.stop_reason = delta.stop_reason.into_owned();
                let raw = event.usage.map_or("null", RawValue::get);
                let usage = response::usage_from_raw(Some(raw))?;
                self.usage.output_tokens = usage.output_tokens;
                if raw.trim_start().starts_with('{') {
                    let fields: BTreeMap<String, Json> =
                        serde_json::from_str(raw).map_err(|error| Error(error.to_string()))?;
                    self.charge(raw.len(), limits)?;
                    for (key, value) in &fields {
                        if value.as_str() == Some("null") {
                            continue;
                        }
                        match key.as_str() {
                            "input_tokens" => {
                                let cached = self.usage.cached_input_tokens;
                                let created = self.usage.cache_write_input_tokens;
                                self.usage.input_tokens = usage
                                    .input_tokens
                                    .wrapping_sub(usage.cached_input_tokens)
                                    .wrapping_sub(usage.cache_write_input_tokens)
                                    .wrapping_add(cached)
                                    .wrapping_add(created);
                            }
                            "cache_read_input_tokens" => {
                                self.usage.input_tokens = self
                                    .usage
                                    .input_tokens
                                    .wrapping_sub(self.usage.cached_input_tokens)
                                    .wrapping_add(usage.cached_input_tokens);
                                self.usage.cached_input_tokens = usage.cached_input_tokens;
                            }
                            "cache_creation_input_tokens" => {
                                self.usage.input_tokens = self
                                    .usage
                                    .input_tokens
                                    .wrapping_sub(self.usage.cache_write_input_tokens)
                                    .wrapping_add(usage.cache_write_input_tokens);
                                self.usage.cache_write_input_tokens =
                                    usage.cache_write_input_tokens;
                            }
                            "output_tokens_details" => {
                                self.usage.reasoning_tokens = usage.reasoning_tokens
                            }
                            _ => {}
                        }
                    }
                    if fields.values().any(|value| value.as_str() != Some("null")) {
                        self.raw_usage.get_or_insert_with(BTreeMap::new).extend(
                            fields
                                .into_iter()
                                .filter(|(_, value)| value.as_str() != Some("null")),
                        );
                    }
                }
            }
            "message_stop" => {
                for index in 0..self.blocks.len() {
                    self.refresh_input(index);
                }
                self.complete = true;
            }
            _ => {}
        }
        Ok(())
    }
}
