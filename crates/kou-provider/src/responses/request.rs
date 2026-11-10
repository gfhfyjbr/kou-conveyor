use crate::{Error, History, RequestData, Result, Tool};
use kou_domain::{Json, LlmData, LlmItem, Message, ToolCall, ToolResult, validate_json};
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
};

const UNRECORDED: &str = "No result was recorded for this tool call.";
const IMAGE_LEFT_OUT: &str = "[An image was left out here: this model cannot read images.]";

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    store: bool,
    stream: bool,
    include: [&'static str; 1],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input: Vec<Item<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Function<'a>>,
    #[serde(flatten)]
    extensions: &'a BTreeMap<String, Json>,
}
#[derive(Serialize)]
struct Reasoning<'a> {
    effort: &'a str,
    summary: &'static str,
}

enum Item<'a> {
    Message {
        role: &'a str,
        content: Content<'a>,
        id: Option<&'a str>,
        phase: Option<&'a str>,
        identified: bool,
    },
    Call {
        call: &'a ToolCall,
        id: Option<&'a str>,
        arguments: Cow<'a, str>,
    },
    Output {
        id: &'a str,
        content: Vec<Part<'a>>,
    },
    Raw(&'a Json),
}
#[derive(Serialize)]
#[serde(untagged)]
enum Content<'a> {
    Text(Cow<'a, str>),
    Parts(Vec<Part<'a>>),
}
enum Part<'a> {
    Text(Cow<'a, str>, bool),
    Image(&'a str, bool),
}
impl Serialize for Part<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Text(text, output) => {
                map.serialize_entry("type", if *output { "output_text" } else { "input_text" })?;
                map.serialize_entry("text", text)?;
                if *output {
                    map.serialize_entry("annotations", &[] as &[()])?;
                    map.serialize_entry("logprobs", &[] as &[()])?;
                }
            }
            Self::Image(url, detail) => {
                map.serialize_entry("type", "input_image")?;
                map.serialize_entry("image_url", url)?;
                if *detail {
                    map.serialize_entry("detail", "auto")?;
                }
            }
        }
        map.end()
    }
}
impl Serialize for Item<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Self::Raw(value) = self {
            return value.serialize(serializer);
        }
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Raw(_) => unreachable!(),
            Self::Message {
                role,
                content,
                id,
                phase,
                identified,
            } => {
                map.serialize_entry("role", role)?;
                map.serialize_entry("content", content)?;
                if *identified {
                    map.serialize_entry("type", "message")?;
                }
                if let Some(id) = id {
                    map.serialize_entry("id", id)?;
                    map.serialize_entry("status", "completed")?;
                }
                if let Some(phase) = phase {
                    map.serialize_entry("phase", phase)?;
                }
            }
            Self::Call {
                call,
                id,
                arguments,
            } => {
                map.serialize_entry("type", "function_call")?;
                map.serialize_entry("call_id", &call.call_id)?;
                map.serialize_entry("name", &call.name)?;
                map.serialize_entry("arguments", arguments)?;
                if let Some(id) = id {
                    map.serialize_entry("id", id)?;
                }
            }
            Self::Output { id, content } => {
                map.serialize_entry("type", "function_call_output")?;
                map.serialize_entry("call_id", id)?;
                map.serialize_entry("output", content)?;
            }
        }
        map.end()
    }
}
struct Function<'a>(&'a Tool);
impl Serialize for Function<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let tool = self.0;
        let mut map = serializer.serialize_map(None)?;
        if tool.kind == "hosted" {
            map.serialize_entry("type", &tool.name)?;
        } else {
            map.serialize_entry("type", "function")?;
            map.serialize_entry("name", &tool.name)?;
            let empty = BTreeMap::<String, Json>::new();
            map.serialize_entry("parameters", tool.parameters.as_ref().unwrap_or(&empty))?;
            map.serialize_entry("strict", &false)?;
            if !tool.description.is_empty() {
                map.serialize_entry("description", &tool.description)?;
            }
        }
        map.end()
    }
}

/// The wire view borrows history text, argument strings, schemas and reasoning.
/// Only synthetic missing/late results need new content; no serde::Value tree or
/// cloned conversation is retained beside the serialized request body.
pub fn request_body(
    request: &RequestData<impl History>,
    cache_key: &str,
    extensions: &BTreeMap<String, Json>,
) -> Result<Vec<u8>> {
    let body = wire_body(request, cache_key, extensions, false)?;
    serde_json::to_vec(&body).map_err(|e| Error(format!("encode response request: {e}")))
}

pub(super) fn request_body_limited(
    request: &RequestData<impl History>,
    cache_key: &str,
    extensions: &BTreeMap<String, Json>,
    without_images: bool,
    limit: usize,
) -> Result<Vec<u8>> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit - self.bytes.len() {
                return Err(std::io::Error::other(
                    "responses request exceeds the request-byte limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let body = wire_body(request, cache_key, extensions, without_images)?;
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, &body)
        .map_err(|e| Error(format!("encode response request: {e}")))?;
    Ok(output.bytes)
}
fn wire_body<'a>(
    request: &'a RequestData<impl History>,
    cache_key: &'a str,
    extensions: &'a BTreeMap<String, Json>,
    without_images: bool,
) -> Result<Body<'a>> {
    let input = input(request, without_images)?;
    for (index, tool) in request.tools.iter().enumerate() {
        let error = match tool.kind.as_str() {
            "function" => None,
            "hosted" if tool.name == "web_search" => None,
            "hosted" => Some(format!("unsupported hosted tool name {:?}", tool.name)),
            _ => Some(format!("unsupported tool type {:?}", tool.kind)),
        };
        if let Some(error) = error {
            return Err(Error(format!("tool {index}: {error}")));
        }
    }
    let effort = &request.model.reasoning_effort;
    if !effort.is_empty() && !matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max")
    {
        return Err(Error(format!("unsupported reasoning effort {effort:?}")));
    }
    let optional = [
        ("input", !input.is_empty()),
        ("prompt_cache_key", !cache_key.is_empty()),
        (
            "max_output_tokens",
            request.model.max_output_tokens.is_some(),
        ),
        ("reasoning", !effort.is_empty()),
        ("tools", !request.tools.is_empty()),
    ];
    for (name, value) in extensions {
        if matches!(name.as_str(), "model" | "store" | "stream" | "include")
            || optional
                .iter()
                .any(|(field, present)| field == name && *present)
        {
            return Err(Error(format!(
                "request extension {name:?} overrides a Responses API field"
            )));
        }
        if value.is_missing() {
            return Err(Error(format!(
                "request extension {name:?} is not valid JSON"
            )));
        }
    }
    let body = Body {
        model: &request.model.id,
        store: false,
        stream: true,
        include: ["reasoning.encrypted_content"],
        input,
        prompt_cache_key: (!cache_key.is_empty()).then_some(cache_key),
        max_output_tokens: request.model.max_output_tokens,
        reasoning: (!effort.is_empty()).then_some(Reasoning {
            effort,
            summary: "auto",
        }),
        tools: request.tools.iter().map(Function).collect(),
        extensions,
    };
    Ok(body)
}

fn own<'a>(item: &'a LlmItem, model: &str) -> Option<(&'a LlmItem, &'a str)> {
    if !model.is_empty() && !item.model.is_empty() && model != item.model {
        if matches!(item.data, LlmData::Reasoning(_)) {
            return None;
        }
        return Some((item, ""));
    }
    Some((item, &item.provider_id))
}
fn foreign(item: &LlmItem) -> bool {
    if let LlmData::Reasoning(reasoning) = &item.data {
        if reasoning.raw.is_missing() {
            return false;
        }
        #[derive(Default, serde::Deserialize)]
        struct Kind {
            #[serde(default, rename = "type", deserialize_with = "crate::null_default")]
            kind: String,
        }
        return crate::optional_object::<_, Kind>(&mut serde_json::Deserializer::from_str(
            reasoning.raw.as_str().unwrap_or("null"),
        ))
        .is_ok_and(|value| value.is_none_or(|value| value.kind != "reasoning"));
    }
    false
}
fn input(request: &RequestData<impl History>, without_images: bool) -> Result<Vec<Item<'_>>> {
    let recorded: HashSet<&str> = request
        .input
        .iter()
        .filter_map(|item| {
            if let LlmData::ToolResult(result) = &item.data {
                Some(result.call_id.as_str())
            } else {
                None
            }
        })
        .collect();
    let mut calls = HashMap::new();
    let mut answered = HashSet::new();
    let mut unrecorded = Vec::new();
    let mut answering = false;
    let mut converted = Vec::with_capacity(request.input.len());
    for (index, source) in request.input.iter().enumerate() {
        let Some((item, id)) = own(source, &request.model.id) else {
            continue;
        };
        if foreign(item) {
            continue;
        }
        let call = if let LlmData::ToolCall(call) = &item.data {
            Some(call)
        } else {
            None
        };
        let result = if let LlmData::ToolResult(result) = &item.data {
            Some(result)
        } else {
            None
        };
        let output = result.is_some_and(|result| !answered.contains(result.call_id.as_str()));
        if !output && (call.is_none() || answering) {
            answer_unrecorded(&mut converted, &mut unrecorded, &mut answered);
        }
        answering = output;
        if let Some(call) = call {
            calls.insert(call.call_id.as_str(), call);
            if !recorded.contains(call.call_id.as_str()) {
                unrecorded.push(call.call_id.as_str());
            }
        }
        let converted_item = if let Some(result) = result {
            let late = answered.contains(result.call_id.as_str());
            answered.insert(result.call_id.as_str());
            if late {
                late_result(&calls, result, without_images)
            } else {
                item_wire(item, id, without_images)
            }
        } else {
            item_wire(item, id, without_images)
        };
        converted.push(converted_item.map_err(|e| Error(format!("input item {index}: {e}")))?);
    }
    answer_unrecorded(&mut converted, &mut unrecorded, &mut answered);
    Ok(converted)
}
fn answer_unrecorded<'a>(
    converted: &mut Vec<Item<'a>>,
    unrecorded: &mut Vec<&'a str>,
    answered: &mut HashSet<&'a str>,
) {
    for id in unrecorded.drain(..) {
        if answered.insert(id) {
            converted.push(Item::Output {
                id,
                content: vec![Part::Text(Cow::Borrowed(UNRECORDED), false)],
            });
        }
    }
}
fn message<'a>(source: &'a Message, id: &'a str, without_images: bool) -> Item<'a> {
    let images = source.images.as_deref().unwrap_or_default();
    let (text, images) = if without_images && !images.is_empty() {
        let labels: Vec<&str> = images
            .iter()
            .map(|image| image.label.trim())
            .filter(|label| !label.is_empty())
            .collect();
        let note = if labels.is_empty() {
            "[The images of this message were left out: this model cannot read images.]".into()
        } else {
            format!(
                "[The images of this message were left out: this model cannot read images. They were {}.]",
                labels.join(", ")
            )
        };
        (
            Cow::Owned(format!("{}\n\n{note}", source.text).trim().to_owned()),
            &[][..],
        )
    } else {
        (Cow::Borrowed(source.text.as_str()), images)
    };
    let content = if id.is_empty() && images.is_empty() {
        Content::Text(text)
    } else if !id.is_empty() && source.role == "assistant" {
        Content::Parts(if text.is_empty() {
            vec![]
        } else {
            vec![Part::Text(text, true)]
        })
    } else {
        let mut parts = Vec::new();
        for image in images {
            if !image.label.is_empty() {
                parts.push(Part::Text(Cow::Borrowed(&image.label), false));
            }
            parts.push(Part::Image(&image.url, true));
        }
        if !text.is_empty() || images.is_empty() {
            parts.push(Part::Text(text, false));
        }
        Content::Parts(parts)
    };
    let assistant = !id.is_empty() && source.role == "assistant";
    Item::Message {
        role: &source.role,
        content,
        id: assistant.then_some(id),
        phase: (id.is_empty() || assistant)
            .then_some(source.phase.as_str())
            .filter(|v| !v.is_empty()),
        identified: !id.is_empty(),
    }
}
fn item_wire<'a>(source: &'a LlmItem, id: &'a str, without_images: bool) -> Result<Item<'a>> {
    match &source.data {
        LlmData::Message(value) => Ok(message(value, id, without_images)),
        LlmData::Reasoning(value) => {
            if value.raw.is_missing() {
                return Err(Error(
                    "reasoning item must carry the provider item in Raw".into(),
                ));
            }
            Ok(Item::Raw(&value.raw))
        }
        LlmData::ToolCall(call) => {
            let arguments = if call.arguments.trim_start().starts_with('{')
                && validate_json(call.arguments.as_bytes()).is_ok()
            {
                Cow::Borrowed(call.arguments.as_str())
            } else {
                Cow::Owned(
                    serde_json::to_string(&BTreeMap::from([(
                        "invalid_arguments",
                        &call.arguments,
                    )]))
                    .map_err(|e| {
                        Error(format!(
                            "encode tool call {:?} arguments: {e}",
                            call.call_id
                        ))
                    })?,
                )
            };
            Ok(Item::Call {
                call,
                id: (!id.is_empty()).then_some(id),
                arguments,
            })
        }
        LlmData::ToolResult(result) => {
            let mut parts = Vec::new();
            for part in &result.output {
                parts.push(match part.kind.as_str() {
                    "text" => Part::Text(Cow::Borrowed(&part.value), false),
                    "image" if without_images => Part::Text(Cow::Borrowed(IMAGE_LEFT_OUT), false),
                    "image" => Part::Image(&part.value, false),
                    _ => {
                        return Err(Error(format!(
                            "unsupported tool result kind {:?}",
                            part.kind
                        )));
                    }
                });
            }
            Ok(Item::Output {
                id: &result.call_id,
                content: parts,
            })
        }
    }
}
fn describe(call: &ToolCall) -> String {
    let mut text = String::new();
    let mut chars = 0;
    for word in call.arguments.split_whitespace() {
        if chars > 0 {
            text.push(' ');
            chars += 1;
        }
        for ch in word.chars() {
            text.push(ch);
            chars += 1;
            if chars > 300 {
                let cut = text.char_indices().nth(299).unwrap().0;
                text.truncate(cut);
                text.push('…');
                return format!("{} ({} {text})", call.call_id, call.name);
            }
        }
    }
    format!("{} ({} {text})", call.call_id, call.name)
}
fn late_result<'a>(
    calls: &HashMap<&str, &ToolCall>,
    result: &'a ToolResult,
    without_images: bool,
) -> Result<Item<'a>> {
    let description = calls
        .get(result.call_id.as_str())
        .map_or_else(|| result.call_id.clone(), |call| describe(call));
    let mut text = format!("Result of tool call {description}:");
    let mut has_text_part = true;
    let mut parts = Vec::new();
    let mut images = false;
    for part in &result.output {
        let (kind, value) = if without_images && part.kind == "image" {
            ("text", IMAGE_LEFT_OUT)
        } else {
            (part.kind.as_str(), part.value.as_str())
        };
        match kind {
            "text" => {
                if has_text_part {
                    text.push('\n');
                }
                text.push_str(value);
                has_text_part = true;
            }
            "image" => {
                if !text.is_empty() {
                    parts.push(Part::Text(Cow::Owned(std::mem::take(&mut text)), false));
                }
                has_text_part = false;
                parts.push(Part::Image(&part.value, true));
                images = true;
            }
            _ => {
                return Err(Error(format!(
                    "unsupported tool result kind {:?}",
                    part.kind
                )));
            }
        }
    }
    let content = if !images {
        Content::Text(Cow::Owned(text))
    } else {
        if !text.is_empty() {
            parts.push(Part::Text(Cow::Owned(text), false));
        }
        Content::Parts(parts)
    };
    Ok(Item::Message {
        role: "user",
        content,
        id: None,
        phase: None,
        identified: false,
    })
}
