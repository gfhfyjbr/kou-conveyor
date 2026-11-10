use crate::{Error, Result, null_default, object_or_null};
use kou_domain::{
    Failure, Json, LlmData, LlmItem, Message, Reasoning, Response, ToolCall, Usage, validate_json,
};
use serde::{
    Deserialize,
    de::{self, Visitor},
};
use serde_json::value::RawValue;
use std::{borrow::Cow, fmt};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Envelope<'a> {
    #[serde(borrow, deserialize_with = "text")]
    id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    status: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "null_default")]
    output: Vec<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    usage: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    error: Option<&'a RawValue>,
    #[serde(borrow, deserialize_with = "optional_raw")]
    incomplete_details: Option<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Header<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "text")]
    kind: Cow<'a, str>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct OutputMessage<'a> {
    #[serde(borrow, deserialize_with = "text")]
    id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    phase: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "null_default")]
    content: Vec<&'a RawValue>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct OutputCall<'a> {
    #[serde(borrow, deserialize_with = "text")]
    id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    status: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    call_id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    name: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "text")]
    arguments: Cow<'a, str>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct OutputReasoning<'a> {
    #[serde(borrow, deserialize_with = "text")]
    id: Cow<'a, str>,
    #[serde(borrow, deserialize_with = "null_default")]
    summary: Vec<Object<Text<'a>>>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Text<'a> {
    #[serde(borrow, deserialize_with = "text")]
    text: Cow<'a, str>,
}
#[derive(Default)]
struct Object<T>(T);
impl<'de, T: Deserialize<'de> + Default> Deserialize<'de> for Object<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        object_or_null(deserializer).map(Self)
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Refusal<'a> {
    #[serde(borrow, deserialize_with = "text")]
    refusal: Cow<'a, str>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Tokens {
    #[serde(deserialize_with = "null_default")]
    input_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    output_tokens: i64,
    #[serde(deserialize_with = "object_or_null")]
    input_tokens_details: InputTokens,
    #[serde(deserialize_with = "object_or_null")]
    output_tokens_details: OutputTokens,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct InputTokens {
    #[serde(deserialize_with = "null_default")]
    cached_tokens: i64,
    #[serde(deserialize_with = "null_default")]
    cache_write_tokens: i64,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct OutputTokens {
    #[serde(deserialize_with = "null_default")]
    reasoning_tokens: i64,
}

pub(crate) fn optional_raw<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(de).map(Some)
}
// Borrow ordinary text; only JSON escapes allocate. Null has Go's zero value.
pub(crate) fn text<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> std::result::Result<Cow<'de, str>, D::Error> {
    struct Text;
    impl<'de> Visitor<'de> for Text {
        type Value = Cow<'de, str>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or null")
        }
        fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Borrowed(""))
        }
        fn visit_some<D: serde::Deserializer<'de>>(
            self,
            de: D,
        ) -> std::result::Result<Self::Value, D::Error> {
            de.deserialize_str(self)
        }
        fn visit_borrowed_str<E: de::Error>(
            self,
            value: &'de str,
        ) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Borrowed(value))
        }
        fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Owned(value.to_owned()))
        }
        fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Owned(value))
        }
    }
    de.deserialize_option(Text)
}
fn decode<'de, T: Deserialize<'de> + Default>(
    raw: &'de str,
) -> std::result::Result<T, serde_json::Error> {
    object_or_null(&mut serde_json::Deserializer::from_str(raw))
}

pub fn decode_response(body: &[u8]) -> Result<Response> {
    validate_json(body).map_err(|e| Error(format!("decode response: {e}")))?;
    let envelope: Envelope<'_> = object_or_null(&mut serde_json::Deserializer::from_slice(body))
        .map_err(|e| Error(format!("decode response: {e}")))?;
    let (stop, failure) = terminal_status(&envelope)?;
    let raw_usage = envelope.usage.map_or("null", RawValue::get);
    let usage: Tokens = decode(raw_usage).map_err(|e| Error(format!("decode usage: {e}")))?;
    let mut output = Vec::with_capacity(envelope.output.len());
    for (index, raw) in envelope.output.into_iter().enumerate() {
        if let Some(item) =
            output_item(raw).map_err(|e| Error(format!("output item {index}: {e}")))?
        {
            output.push(item);
        }
    }
    Ok(Response {
        id: envelope.id.into_owned(),
        stop,
        failure,
        output: Some(output),
        usage: Usage {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.input_tokens_details.cached_tokens,
            cache_write_input_tokens: usage.input_tokens_details.cache_write_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.output_tokens_details.reasoning_tokens,
            raw: if envelope.usage.is_some() {
                Json::parse(raw_usage).map_err(|e| Error(e.to_string()))?
            } else {
                Json::default()
            },
        },
    })
}

fn output_item(raw: &RawValue) -> Result<Option<LlmItem>> {
    let header: Header<'_> =
        decode(raw.get()).map_err(|e| Error(format!("decode output item type: {e}")))?;
    let (id, data) = match header.kind.as_ref() {
        "web_search_call" => return Ok(None),
        "message" => {
            let message: OutputMessage<'_> = decode(raw.get()).map_err(|e| Error(e.to_string()))?;
            let mut text = String::new();
            for raw in message.content {
                let header: Header<'_> = decode(raw.get())
                    .map_err(|e| Error(format!("decode message content type: {e}")))?;
                match header.kind.as_ref() {
                    "output_text" => {
                        let part: Text<'_> = decode(raw.get()).map_err(|e| Error(e.to_string()))?;
                        text.push_str(&part.text);
                    }
                    "refusal" => {
                        let part: Refusal<'_> =
                            decode(raw.get()).map_err(|e| Error(e.to_string()))?;
                        text.push_str(&part.refusal);
                    }
                    kind => {
                        return Err(Error(format!("unsupported message content type {kind:?}")));
                    }
                }
            }
            (
                message.id,
                LlmData::Message(Message {
                    role: "assistant".into(),
                    text,
                    phase: message.phase.into_owned(),
                    ..Default::default()
                }),
            )
        }
        "function_call" => {
            let call: OutputCall<'_> = decode(raw.get()).map_err(|e| Error(e.to_string()))?;
            if matches!(call.status.as_ref(), "in_progress" | "incomplete") {
                return Ok(None);
            }
            (
                call.id,
                LlmData::ToolCall(ToolCall {
                    call_id: call.call_id.into_owned(),
                    name: call.name.into_owned(),
                    arguments: call.arguments.into_owned(),
                }),
            )
        }
        "reasoning" => {
            let reasoning: OutputReasoning<'_> =
                decode(raw.get()).map_err(|e| Error(e.to_string()))?;
            let summary = reasoning
                .summary
                .into_iter()
                .map(|part| part.0.text.into_owned())
                .collect();
            (
                reasoning.id,
                LlmData::Reasoning(Reasoning {
                    summary: Some(summary),
                    raw: Json::parse(raw.get()).map_err(|e| Error(e.to_string()))?,
                }),
            )
        }
        kind => return Err(Error(format!("unsupported output item type {kind:?}"))),
    };
    Ok(Some(LlmItem {
        provider_id: id.into_owned(),
        model: String::new(),
        data,
    }))
}

fn terminal_status(envelope: &Envelope<'_>) -> Result<(String, Option<Failure>)> {
    match envelope.status.as_ref() {
        "completed" => Ok(("complete".into(), None)),
        "incomplete" => {
            #[derive(Default, Deserialize)]
            #[serde(default)]
            struct Details {
                #[serde(deserialize_with = "null_default")]
                reason: String,
            }
            let details: Details =
                decode(envelope.incomplete_details.map_or("null", RawValue::get))
                    .map_err(|e| Error(format!("decode incomplete details: {e}")))?;
            let stop = match details.reason.as_str() {
                "max_output_tokens" => "max_output_tokens",
                "content_filter" => "refused",
                reason => return Err(Error(format!("unsupported incomplete reason {reason:?}"))),
            };
            Ok((stop.into(), None))
        }
        "failed" => {
            #[derive(Default, Deserialize)]
            #[serde(default)]
            struct ApiFailure {
                #[serde(deserialize_with = "null_default")]
                code: String,
                #[serde(deserialize_with = "null_default")]
                message: String,
            }
            let failure: Option<ApiFailure> =
                crate::optional_object(&mut serde_json::Deserializer::from_str(
                    envelope.error.map_or("null", RawValue::get),
                ))
                .map_err(|e| Error(format!("decode response error: {e}")))?;
            let failure = failure.map_or_else(
                || Failure {
                    code: String::new(),
                    message: "response failed".into(),
                },
                |value| Failure {
                    code: value.code,
                    message: value.message,
                },
            );
            Ok((String::new(), Some(failure)))
        }
        status => Err(Error(format!("unsupported response status {status:?}"))),
    }
}
