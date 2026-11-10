use crate::{
    Error, Result, null_default, object_or_null,
    responses::{optional_raw, text},
};
use kou_domain::{
    Failure, Json, LlmData, LlmItem, Message, Reasoning, Response, ToolCall, Usage, validate_json,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{borrow::Cow, collections::BTreeMap};

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Envelope<'a> {
    #[serde(borrow, deserialize_with = "text")]
    pub id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub stop_reason: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "null_default")]
    pub content: Vec<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    pub usage: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Block<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    pub kind: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub text: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub thinking: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub signature: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub data: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    pub name: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    pub input: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Tokens {
    #[serde(deserialize_with = "null_default")]
    input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    cache_read_input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    cache_creation_input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    output_tokens: i64,
    #[serde(deserialize_with = "object_or_null")]
    output_tokens_details: Details,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Details {
    #[serde(deserialize_with = "null_default")]
    thinking_tokens: i64,
}

pub fn decode_response(body: &[u8], inputs: &BTreeMap<i64, String>) -> Result<Response> {
    validate_json(body).map_err(|error| Error(format!("decode Messages API response: {error}")))?;
    let message: Envelope<'_> = decode(body)?;
    let blocks = message
        .content
        .iter()
        .map(|raw| decode::<Block<'_>>(raw.get().as_bytes()))
        .collect::<Result<Vec<_>>>()?;
    let usage = usage_from_raw(message.usage.map(RawValue::get))?;
    from_blocks(
        message.id.into_owned(),
        &message.stop_reason,
        blocks,
        inputs,
        usage,
    )
}
pub(super) fn usage_from_raw(raw_usage: Option<&str>) -> Result<Usage> {
    let usage: Tokens = decode(raw_usage.unwrap_or("null").as_bytes())?;
    let usage = Usage {
        input_tokens: usage
            .input_tokens
            .wrapping_add(usage.cache_read_input_tokens)
            .wrapping_add(usage.cache_creation_input_tokens),
        cached_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_creation_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.output_tokens_details.thinking_tokens,
        raw: match raw_usage.filter(|raw| raw.trim_start().starts_with('{')) {
            Some(raw) => Json::parse(raw).map_err(|error| Error(error.to_string()))?,
            None => Json::default(),
        },
    };
    Ok(usage)
}
pub(super) fn decode<'a, T: Deserialize<'a> + Default>(body: &'a [u8]) -> Result<T> {
    object_or_null(&mut serde_json::Deserializer::from_slice(body))
        .map_err(|error| Error(format!("decode Messages API response: {error}")))
}
pub(super) fn from_blocks(
    id: String,
    stop_reason: &str,
    blocks: Vec<Block<'_>>,
    inputs: &BTreeMap<i64, String>,
    usage: Usage,
) -> Result<Response> {
    let mut response = Response {
        id,
        usage,
        ..Default::default()
    };
    match stop_reason {
        "refusal" => {
            response.stop = "refused".into();
            return Ok(response);
        }
        "max_tokens" => response.stop = "max_output_tokens".into(),
        "model_context_window_exceeded" => {
            response.failure = Some(Failure {
                code: "context_window_exceeded".into(),
                message: "the conversation no longer fits the model's context window".into(),
            })
        }
        _ => response.stop = "complete".into(),
    }
    let boundary = blocks.iter().rposition(|block| block.kind == "fallback");
    let last = blocks.len().saturating_sub(1);
    let mut output = Vec::new();
    for (index, block) in blocks.into_iter().enumerate() {
        if boundary.is_some_and(|boundary| index < boundary) && block.kind != "text" {
            continue;
        }
        let data = match block.kind.as_ref() {
            "text" if !block.text.trim().is_empty() => LlmData::Message(Message {
                role: "assistant".into(),
                text: block.text.into_owned(),
                ..Default::default()
            }),
            "thinking" if !block.signature.is_empty() => {
                #[derive(serde::Serialize)]
                struct Raw<'a> {
                    #[serde(rename = "type")]
                    kind: &'static str,
                    thinking: &'a str,
                    signature: &'a str,
                }
                let raw = serde_json::to_string(&Raw {
                    kind: "thinking",
                    thinking: &block.thinking,
                    signature: &block.signature,
                })
                .map_err(|error| Error(error.to_string()))?;
                LlmData::Reasoning(Reasoning {
                    summary: (!block.thinking.trim().is_empty())
                        .then(|| vec![block.thinking.into_owned()]),
                    raw: Json::parse(&raw).map_err(|error| Error(error.to_string()))?,
                })
            }
            "redacted_thinking" if !block.data.is_empty() => {
                #[derive(serde::Serialize)]
                struct Raw<'a> {
                    #[serde(rename = "type")]
                    kind: &'static str,
                    data: &'a str,
                }
                let raw = serde_json::to_string(&Raw {
                    kind: "redacted_thinking",
                    data: &block.data,
                })
                .map_err(|error| Error(error.to_string()))?;
                LlmData::Reasoning(Reasoning {
                    summary: None,
                    raw: Json::parse(&raw).map_err(|error| Error(error.to_string()))?,
                })
            }
            "tool_use" => {
                let input = inputs
                    .get(&(index as i64))
                    .map(String::as_str)
                    .unwrap_or_else(|| block.input.map_or("{}", RawValue::get));
                let input = if input.is_empty()
                    || (input == "null" && !inputs.contains_key(&(index as i64)))
                {
                    "{}"
                } else {
                    input
                };
                if response.stop == "max_output_tokens"
                    && (index == last || validate_json(input.as_bytes()).is_err())
                {
                    continue;
                }
                LlmData::ToolCall(ToolCall {
                    call_id: block.id.into_owned(),
                    name: block.name.into_owned(),
                    arguments: input.into(),
                })
            }
            _ => continue,
        };
        output.push(LlmItem {
            provider_id: String::new(),
            model: String::new(),
            data,
        });
    }
    response.output = (!output.is_empty()).then_some(output);
    Ok(response)
}
