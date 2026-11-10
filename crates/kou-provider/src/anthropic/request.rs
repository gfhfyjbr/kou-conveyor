use super::{Breakpoint, DEFAULT_MODEL, Prefix};
use crate::{Error, History, RequestData, Result, Tool};
use base64::{
    alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig},
};
use kou_domain::{Json, LlmData, LlmItem, ToolCall, ToolResultOutput, validate_json};
use serde::{
    Deserialize, Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::value::RawValue;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

const IMAGE_OMITTED: &str =
    "[image omitted: the Messages API accepts PNG, JPEG, GIF and WebP images]";
const UNRECORDED: &str = "No result was recorded for this tool call.";
pub const CONTEXT_BETA: &str = "context-management-2025-06-27";
pub const CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

#[derive(Clone, Debug)]
pub struct RequestConfig {
    pub official: bool,
    pub max_tokens: i64,
    pub cache_ttl: String,
    pub thinking_turns: i64,
    pub server_fallbacks: bool,
}
impl Default for RequestConfig {
    fn default() -> Self {
        Self {
            official: true,
            max_tokens: 0,
            cache_ttl: String::new(),
            thinking_turns: 2,
            server_fallbacks: true,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct AttemptOptions {
    pub keep_thinking: bool,
    pub room: i64,
    pub no_context_management: bool,
    pub no_fallbacks: bool,
}
impl Default for AttemptOptions {
    fn default() -> Self {
        Self {
            keep_thinking: true,
            room: 0,
            no_context_management: false,
            no_fallbacks: false,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelFeatures {
    pub claude: bool,
    pub adaptive: bool,
    pub no_xhigh: bool,
    pub thinks: bool,
    pub max_output: i64,
    pub context_window: i64,
    pub fallbacks: bool,
}
pub fn describe_model(id: &str) -> ModelFeatures {
    let id = id.to_lowercase();
    let Some(start) = id.find("claude-") else {
        return ModelFeatures::default();
    };
    let name = id[start..].replace('.', "-");
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
    let mut model = ModelFeatures {
        claude: true,
        thinks: true,
        max_output: 64000,
        context_window: 200_000,
        ..Default::default()
    };
    if starts(&[
        "claude-opus-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-sonnet-5",
        "claude-mythos-preview",
        "claude-opus-4-8",
        "claude-opus-4-7",
    ]) {
        model.adaptive = true;
        model.max_output = 128000;
        model.context_window = 1_000_000;
        model.fallbacks = starts(&["claude-opus-5", "claude-fable-5"]);
    } else if starts(&["claude-opus-4-6", "claude-sonnet-4-6"]) {
        model.adaptive = true;
        model.no_xhigh = true;
        model.max_output = 128000;
    } else if starts(&["claude-3-haiku", "claude-3-5"]) {
        model.thinks = false;
        model.max_output = if name.starts_with("claude-3-haiku") {
            4096
        } else {
            8192
        };
    } else if starts(&["claude-opus-4-0", "claude-opus-4-1", "claude-opus-4-2"]) {
        model.max_output = 32000;
    }
    model
}
pub fn context_window(id: &str) -> i64 {
    describe_model(id).context_window
}

pub struct WireRequest<'a> {
    pub model: &'a str,
    pub max_tokens: i64,
    system: Vec<Block<'a>>,
    messages: Vec<Message<'a>>,
    tools: Vec<Function<'a>>,
    thinking: Thinking,
    effort: &'a str,
    edits: Option<i64>,
    fallbacks: bool,
    betas: Vec<&'static str>,
}
enum Thinking {
    None,
    Adaptive,
    Enabled(i64),
}
struct Message<'a> {
    role: &'static str,
    content: Vec<Block<'a>>,
}
struct Block<'a> {
    content: Content<'a>,
    cache: Option<&'a str>,
}
enum Content<'a> {
    Text(Cow<'a, str>),
    Image(Image<'a>),
    Thinking {
        signature: Cow<'a, str>,
        thinking: Cow<'a, str>,
    },
    Redacted(Cow<'a, str>),
    Call {
        call: &'a ToolCall,
        input: Cow<'a, RawValue>,
    },
    Result {
        id: &'a str,
        content: Vec<Block<'a>>,
        error: bool,
    },
}
enum Image<'a> {
    Url(&'a str),
    Base64 { data: &'a str, media: &'static str },
}
struct Function<'a> {
    tool: &'a Tool,
    eager: bool,
}
struct Blocks<'a, 'b> {
    blocks: &'b [Block<'a>],
    markers: bool,
}
struct BlockView<'a, 'b> {
    block: &'b Block<'a>,
    markers: bool,
}
struct MessageView<'a, 'b> {
    message: &'b Message<'a>,
    markers: bool,
}

impl Serialize for Blocks<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.blocks.len()))?;
        for block in self.blocks {
            seq.serialize_element(&BlockView {
                block,
                markers: self.markers,
            })?;
        }
        seq.end()
    }
}
impl Serialize for MessageView<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(
            "content",
            &Blocks {
                blocks: &self.message.content,
                markers: self.markers,
            },
        )?;
        map.serialize_entry("role", self.message.role)?;
        map.end()
    }
}
impl Serialize for BlockView<'_, '_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let kind = match &self.block.content {
            Content::Text(text) => {
                map.serialize_entry("text", text)?;
                "text"
            }
            Content::Image(image) => {
                map.serialize_entry("source", image)?;
                "image"
            }
            Content::Thinking {
                signature,
                thinking,
            } => {
                map.serialize_entry("signature", signature)?;
                map.serialize_entry("thinking", thinking)?;
                "thinking"
            }
            Content::Redacted(data) => {
                map.serialize_entry("data", data)?;
                "redacted_thinking"
            }
            Content::Call { call, input } => {
                map.serialize_entry("id", &call.call_id)?;
                map.serialize_entry("input", input)?;
                map.serialize_entry("name", &call.name)?;
                "tool_use"
            }
            Content::Result { id, error, .. } => {
                map.serialize_entry("tool_use_id", id)?;
                if *error {
                    map.serialize_entry("is_error", &true)?;
                }
                "tool_result"
            }
        };
        if self.markers
            && let Some(ttl) = self.block.cache
        {
            map.serialize_entry("cache_control", &CacheControl(ttl))?;
        }
        if let Content::Result { content, .. } = &self.block.content {
            map.serialize_entry(
                "content",
                &Blocks {
                    blocks: content,
                    markers: true,
                },
            )?;
        }
        map.serialize_entry("type", kind)?;
        map.end()
    }
}
struct CacheControl<'a>(&'a str);
impl Serialize for CacheControl<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if !self.0.is_empty() {
            map.serialize_entry("ttl", self.0)?;
        }
        map.serialize_entry("type", "ephemeral")?;
        map.end()
    }
}
impl Serialize for Image<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Url(url) => {
                map.serialize_entry("url", url)?;
                map.serialize_entry("type", "url")?;
            }
            Self::Base64 { data, media } => {
                map.serialize_entry("data", data)?;
                map.serialize_entry("media_type", media)?;
                map.serialize_entry("type", "base64")?;
            }
        }
        map.end()
    }
}
impl Serialize for Function<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("input_schema", &Schema(self.tool))?;
        map.serialize_entry("name", &self.tool.name)?;
        if self.eager {
            map.serialize_entry("eager_input_streaming", &true)?;
        }
        if !self.tool.description.is_empty() {
            map.serialize_entry("description", &self.tool.description)?;
        }
        map.end()
    }
}
struct Schema<'a>(&'a Tool);
impl Serialize for Schema<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        let mut inserted = false;
        if let Some(parameters) = &self.0.parameters {
            for (key, value) in parameters {
                if key.as_str() >= "type" && !inserted {
                    map.serialize_entry("type", "object")?;
                    inserted = true;
                }
                if key != "type" {
                    map.serialize_entry(key, &Sorted(value.as_str().unwrap_or("null"), 0))?;
                }
            }
        }
        if !inserted {
            map.serialize_entry("type", "object")?;
        }
        map.end()
    }
}
struct Sorted<'a>(&'a str, usize);
impl Serialize for Sorted<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if self.1 >= 128 {
            return Err(serde::ser::Error::custom(
                "Messages API tool schema exceeds the depth limit",
            ));
        }
        match self.0.trim_start().as_bytes().first() {
            Some(b'{') => {
                let values: BTreeMap<Cow<'_, str>, &RawValue> =
                    serde_json::from_str(self.0).map_err(serde::ser::Error::custom)?;
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(&key, &Sorted(value.get(), self.1 + 1))?;
                }
                map.end()
            }
            Some(b'[') => {
                let values: Vec<&RawValue> =
                    serde_json::from_str(self.0).map_err(serde::ser::Error::custom)?;
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&Sorted(value.get(), self.1 + 1))?;
                }
                seq.end()
            }
            _ => <&RawValue>::deserialize(&mut serde_json::Deserializer::from_str(self.0))
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
        }
    }
}
#[derive(Serialize)]
struct Edit {
    keep: Keep,
    #[serde(rename = "type")]
    kind: &'static str,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Keep {
    All(&'static str),
    Turns {
        value: i64,
        #[serde(rename = "type")]
        kind: &'static str,
    },
}
fn edits(turns: i64) -> [Edit; 1] {
    [Edit {
        keep: if turns == 0 {
            Keep::All("all")
        } else {
            Keep::Turns {
                value: turns,
                kind: "thinking_turns",
            }
        },
        kind: "clear_thinking_20251015",
    }]
}
impl Serialize for WireRequest<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("max_tokens", &self.max_tokens)?;
        struct Messages<'a, 'b>(&'b [Message<'a>]);
        impl Serialize for Messages<'_, '_> {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                let mut seq = s.serialize_seq(Some(self.0.len()))?;
                for message in self.0 {
                    seq.serialize_element(&MessageView {
                        message,
                        markers: true,
                    })?;
                }
                seq.end()
            }
        }
        map.serialize_entry("messages", &Messages(&self.messages))?;
        map.serialize_entry("model", self.model)?;
        if self.fallbacks {
            map.serialize_entry("fallbacks", "default")?;
        }
        if let Some(turns) = self.edits {
            #[derive(Serialize)]
            struct Management {
                edits: [Edit; 1],
            }
            map.serialize_entry(
                "context_management",
                &Management {
                    edits: edits(turns),
                },
            )?;
        }
        if !self.effort.is_empty() {
            #[derive(Serialize)]
            struct Effort<'a> {
                effort: &'a str,
            }
            map.serialize_entry(
                "output_config",
                &Effort {
                    effort: self.effort,
                },
            )?;
        }
        if !self.system.is_empty() {
            map.serialize_entry(
                "system",
                &Blocks {
                    blocks: &self.system,
                    markers: true,
                },
            )?;
        }
        match self.thinking {
            Thinking::None => {}
            Thinking::Adaptive => {
                #[derive(Serialize)]
                struct Adaptive {
                    display: &'static str,
                    #[serde(rename = "type")]
                    kind: &'static str,
                }
                map.serialize_entry(
                    "thinking",
                    &Adaptive {
                        display: "summarized",
                        kind: "adaptive",
                    },
                )?;
            }
            Thinking::Enabled(budget) => {
                #[derive(Serialize)]
                struct Enabled {
                    budget_tokens: i64,
                    #[serde(rename = "type")]
                    kind: &'static str,
                }
                map.serialize_entry(
                    "thinking",
                    &Enabled {
                        budget_tokens: budget,
                        kind: "enabled",
                    },
                )?;
            }
        }
        map.serialize_entry("tools", &self.tools)?;
        map.serialize_entry("stream", &true)?;
        map.end()
    }
}
impl WireRequest<'_> {
    pub fn betas(&self) -> &[&'static str] {
        &self.betas
    }
    pub fn has_context_management(&self) -> bool {
        self.edits.is_some()
    }
    pub fn has_fallbacks(&self) -> bool {
        self.fallbacks
    }
    pub fn encode(&self, limit: usize) -> Result<Vec<u8>> {
        struct Bounded {
            bytes: Vec<u8>,
            limit: usize,
        }
        impl std::io::Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > self.limit - self.bytes.len() {
                    return Err(std::io::Error::other(
                        "Messages API request exceeds the request-byte limit",
                    ));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Bounded {
            bytes: Vec::new(),
            limit,
        };
        kou_domain::write_compact_json(&mut writer, self)
            .map_err(|error| Error(format!("encode Messages API request: {error}")))?;
        Ok(writer.bytes)
    }
    pub fn prefix(&self) -> Result<Prefix> {
        use super::fingerprint::{Rolling, fingerprint};
        let mut prefix = Prefix::default();
        if !self.tools.is_empty() {
            prefix.tools = fingerprint(&self.tools)?;
        }
        for block in &self.system {
            if let Some(ttl) = block.cache {
                prefix.breakpoints.push(Breakpoint::new(-1, ttl));
            }
            prefix.system.push(fingerprint(&BlockView {
                block,
                markers: false,
            })?);
        }
        let mut rolling = Rolling::default();
        for (index, message) in self.messages.iter().enumerate() {
            if let Some(ttl) = message.content.last().and_then(|block| block.cache) {
                prefix.breakpoints.push(Breakpoint::new(index as i64, ttl));
            }
            prefix.messages.push(rolling.observe(&MessageView {
                message,
                markers: false,
            })?);
        }
        if let Some(turns) = self.edits {
            prefix.edits = Json::parse(
                &serde_json::to_string(&edits(turns)).map_err(|error| Error(error.to_string()))?,
            )
            .map_err(|error| Error(error.to_string()))?;
        }
        Ok(prefix)
    }
}
pub fn prepare<'a>(
    request: &'a RequestData<impl History>,
    config: &'a RequestConfig,
    attempt: AttemptOptions,
) -> Result<WireRequest<'a>> {
    let id = request.model.id.trim();
    let id = if id.is_empty() { DEFAULT_MODEL } else { id };
    let mut effort = request.model.reasoning_effort.as_str();
    if !matches!(effort, "" | "low" | "medium" | "high" | "xhigh" | "max") {
        return Err(Error(format!("unsupported reasoning effort {effort:?}")));
    }
    let model = describe_model(id);
    let (mut system, mut messages) = conversation(&request.input, id, attempt.keep_thinking)?;
    if messages.is_empty() {
        return Err(Error("request has no messages for the model".into()));
    }
    let mut tools = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        if tool.kind != "function" {
            return Err(Error(format!(
                "tool {:?}: the Messages API client supports function tools only, not {:?}",
                tool.name, tool.kind
            )));
        }
        tools.push(Function {
            tool,
            eager: config.official && model.claude,
        });
    }
    let mut max_tokens = request
        .model
        .max_output_tokens
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            if config.max_tokens > 0 {
                config.max_tokens
            } else if !model.claude {
                16384
            } else {
                model.max_output.min(if matches!(effort, "xhigh" | "max") {
                    128000
                } else {
                    64000
                })
            }
        });
    if attempt.room > 0 {
        max_tokens = max_tokens.min(attempt.room);
    }
    let thinking = if model.adaptive {
        if model.no_xhigh && effort == "xhigh" {
            effort = "high";
        }
        Thinking::Adaptive
    } else {
        let budget = match effort {
            "low" => 4096,
            "medium" => 10000,
            "high" => 20000,
            "xhigh" => 32000,
            "max" => 48000,
            _ => 0,
        }
        .min(max_tokens.wrapping_mul(3) / 4);
        effort = "";
        if model.thinks && budget >= 1024 {
            Thinking::Enabled(budget)
        } else {
            Thinking::None
        }
    };
    let mut betas = Vec::new();
    if model.claude {
        if let Some(block) = system.last_mut() {
            block.cache = Some(&config.cache_ttl);
        }
        let latest = messages.len() - 1;
        for (index, message) in messages
            .iter_mut()
            .enumerate()
            .rev()
            .filter(|(index, message)| *index == latest || message.role == "user")
            .take(2)
        {
            let _ = index;
            if let Some(block) = message.content.last_mut()
                && !matches!(
                    block.content,
                    Content::Thinking { .. } | Content::Redacted(_)
                )
            {
                block.cache = Some(&config.cache_ttl);
            }
        }
        if config.cache_ttl == "1h" {
            betas.push(CACHE_BETA);
        }
    }
    let edits = (!matches!(thinking, Thinking::None) && !attempt.no_context_management)
        .then_some(config.thinking_turns);
    if edits.is_some() {
        betas.push(CONTEXT_BETA);
    }
    let fallbacks = model.fallbacks && config.server_fallbacks && !attempt.no_fallbacks;
    if fallbacks {
        betas.push(FALLBACK_BETA);
    }
    Ok(WireRequest {
        model: id,
        max_tokens,
        system,
        messages,
        tools,
        thinking,
        effort,
        edits,
        fallbacks,
        betas,
    })
}

fn text(value: &str) -> Option<Block<'_>> {
    (!value.trim().is_empty()).then_some(Block {
        content: Content::Text(Cow::Borrowed(value)),
        cache: None,
    })
}
fn image(reference: &str) -> Option<Block<'_>> {
    let content = if let Some(rest) = reference.strip_prefix("data:") {
        let (header, data) = rest.split_once(',')?;
        let (media, encoding) = header.split_once(';').unwrap_or((header, ""));
        if encoding != "base64" {
            return None;
        }
        let media = match media.to_ascii_lowercase().as_str() {
            "image/png" => "image/png",
            "image/jpg" | "image/jpeg" => "image/jpeg",
            "image/gif" => "image/gif",
            "image/webp" => "image/webp",
            _ => return None,
        };
        // Go's standard decoder ignores CR/LF and accepts unused trailing bits.
        let decoder = GeneralPurpose::new(
            &alphabet::STANDARD,
            GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
        );
        fn validate(reader: impl std::io::Read, decoder: &GeneralPurpose) -> std::io::Result<u64> {
            std::io::copy(
                &mut base64::read::DecoderReader::new(reader, decoder),
                &mut std::io::sink(),
            )
        }
        if data.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
            struct ImageBytes<'a>(std::str::Bytes<'a>);
            impl std::io::Read for ImageBytes<'_> {
                fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                    let mut written = 0;
                    for target in output {
                        let Some(byte) = self.0.find(|byte| !matches!(byte, b'\r' | b'\n')) else {
                            break;
                        };
                        *target = byte;
                        written += 1;
                    }
                    Ok(written)
                }
            }
            validate(ImageBytes(data.bytes()), &decoder).ok()?;
        } else {
            validate(data.as_bytes(), &decoder).ok()?;
        }
        Content::Image(Image::Base64 { data, media })
    } else if reference.starts_with("https://") || reference.starts_with("http://") {
        Content::Image(Image::Url(reference))
    } else {
        return None;
    };
    Some(Block {
        content,
        cache: None,
    })
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Replayed<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "crate::responses::text")]
    kind: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "crate::responses::text")]
    signature: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "crate::responses::text")]
    thinking: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "crate::responses::text")]
    data: Cow<'a, str>,
}
fn replay(raw: &Json) -> Option<Replayed<'_>> {
    let raw = raw.as_str()?;
    crate::object_or_null(&mut serde_json::Deserializer::from_str(raw)).ok()
}
fn is_foreign(item: &LlmItem, model: &str) -> bool {
    matches!(item.data, LlmData::Reasoning(_)) && !item.model.is_empty() && item.model != model
}
fn conversation<'a>(
    items: &'a impl History,
    model: &str,
    keep: bool,
) -> Result<(Vec<Block<'a>>, Vec<Message<'a>>)> {
    let reset = items
        .iter()
        .enumerate()
        .filter(|(_, item)| !is_foreign(item, model))
        .filter_map(|(index, item)| {
            if let LlmData::Reasoning(reasoning) = &item.data
                && replay(&reasoning.raw).is_some_and(|raw| raw.kind == "thinking_reset")
            {
                Some(index)
            } else {
                None
            }
        })
        .next_back();
    let mut turns = Turns::default();
    let mut filtered_index = 0;
    for (index, item) in items.iter().enumerate() {
        if is_foreign(item, model) {
            continue;
        }
        let keep = keep && reset.is_none_or(|reset| index > reset);
        turns
            .add(item, keep)
            .map_err(|error| Error(format!("input item {filtered_index}: {error}")))?;
        filtered_index += 1;
    }
    turns.close();
    Ok((turns.system, turns.messages))
}
#[derive(Default)]
struct Turns<'a> {
    system: Vec<Block<'a>>,
    messages: Vec<Message<'a>>,
    started: bool,
    role: &'static str,
    assistant: Vec<Block<'a>>,
    calls: Vec<&'a str>,
    results: HashMap<&'a str, Block<'a>>,
    user: Vec<Block<'a>>,
    made: HashMap<&'a str, &'a ToolCall>,
}
impl<'a> Turns<'a> {
    fn add(&mut self, item: &'a LlmItem, keep: bool) -> Result<()> {
        match &item.data {
            LlmData::Message(message) => {
                let block = text(&message.text);
                match message.role.as_str() {
                    "system" if !self.started => {
                        if let Some(block) = block {
                            self.system.push(block);
                        }
                    }
                    "system" => self.add_user(block),
                    "user" => {
                        for image_ref in message.images.as_deref().unwrap_or_default() {
                            self.add_user(text(&image_ref.label));
                            self.add_user(image(&image_ref.url).or_else(|| text(IMAGE_OMITTED)));
                        }
                        self.add_user(block);
                    }
                    "assistant" => self.add_assistant(block),
                    role => return Err(Error(format!("unsupported message role {role:?}"))),
                }
            }
            LlmData::Reasoning(reasoning) if keep => {
                if let Some(raw) = replay(&reasoning.raw) {
                    let content = match raw.kind.as_ref() {
                        "thinking" if !raw.signature.is_empty() => Some(Content::Thinking {
                            signature: raw.signature,
                            thinking: raw.thinking,
                        }),
                        "redacted_thinking" if !raw.data.is_empty() => {
                            Some(Content::Redacted(raw.data))
                        }
                        _ => None,
                    };
                    self.add_assistant(content.map(|content| Block {
                        content,
                        cache: None,
                    }));
                }
            }
            LlmData::Reasoning(_) => {}
            LlmData::ToolCall(call) => {
                let input = if call.arguments.trim_start().starts_with('{')
                    && validate_json(call.arguments.as_bytes()).is_ok()
                {
                    Cow::Borrowed(
                        <&RawValue>::deserialize(&mut serde_json::Deserializer::from_str(
                            &call.arguments,
                        ))
                        .map_err(|error| Error(error.to_string()))?,
                    )
                } else {
                    Cow::Owned(
                        serde_json::value::to_raw_value(&BTreeMap::from([(
                            "invalid_arguments",
                            &call.arguments,
                        )]))
                        .map_err(|error| Error(error.to_string()))?,
                    )
                };
                self.add_assistant(Some(Block {
                    content: Content::Call { call, input },
                    cache: None,
                }));
                self.calls.push(&call.call_id);
                self.made.insert(&call.call_id, call);
            }
            LlmData::ToolResult(result) => {
                self.open_user();
                let content = result_content(&result.output);
                if !self.results.contains_key(result.call_id.as_str())
                    && self.calls.contains(&result.call_id.as_str())
                {
                    self.results.insert(
                        &result.call_id,
                        Block {
                            content: Content::Result {
                                id: &result.call_id,
                                content,
                                error: false,
                            },
                            cache: None,
                        },
                    );
                } else {
                    let description = self
                        .made
                        .get(result.call_id.as_str())
                        .map_or_else(|| result.call_id.clone(), |call| describe_call(call));
                    self.user.push(Block {
                        content: Content::Text(Cow::Owned(format!(
                            "Result of tool call {description}:"
                        ))),
                        cache: None,
                    });
                    self.user.extend(content);
                }
            }
        }
        Ok(())
    }
    fn add_assistant(&mut self, block: Option<Block<'a>>) {
        let Some(block) = block else { return };
        self.started = true;
        if self.role == "user" {
            self.close_user();
        }
        self.role = "assistant";
        self.assistant.push(block);
    }
    fn open_user(&mut self) {
        self.started = true;
        if self.role == "assistant" {
            self.close_assistant();
        }
        self.role = "user";
    }
    fn add_user(&mut self, block: Option<Block<'a>>) {
        if let Some(block) = block {
            self.open_user();
            self.user.push(block);
        }
    }
    fn close_assistant(&mut self) {
        if !self.assistant.is_empty() {
            self.messages.push(Message {
                role: "assistant",
                content: std::mem::take(&mut self.assistant),
            });
        }
        self.role = "";
    }
    fn close_user(&mut self) {
        let mut content = Vec::with_capacity(self.calls.len() + self.user.len());
        for id in self.calls.drain(..) {
            content.push(self.results.remove(id).unwrap_or_else(|| Block {
                content: Content::Result {
                    id,
                    content: vec![text(UNRECORDED).unwrap()],
                    error: true,
                },
                cache: None,
            }));
        }
        content.append(&mut self.user);
        if !content.is_empty() {
            self.messages.push(Message {
                role: "user",
                content,
            });
        }
        self.results.clear();
        self.role = "";
    }
    fn close(&mut self) {
        match self.role {
            "assistant" => {
                self.close_assistant();
                if !self.calls.is_empty() {
                    self.close_user();
                }
            }
            "user" => self.close_user(),
            _ => {}
        }
    }
}
fn result_content(output: &[ToolResultOutput]) -> Vec<Block<'_>> {
    output
        .iter()
        .filter_map(|part| match part.kind.as_str() {
            "text" => text(&part.value),
            "image" => image(&part.value).or_else(|| text(IMAGE_OMITTED)),
            _ => None,
        })
        .collect()
}
fn describe_call(call: &ToolCall) -> String {
    let arguments = call
        .arguments
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let arguments = if arguments.chars().count() > 300 {
        format!("{}…", arguments.chars().take(299).collect::<String>())
    } else {
        arguments
    };
    format!("{} ({} {arguments})", call.call_id, call.name)
}
