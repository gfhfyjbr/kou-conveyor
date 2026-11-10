#[path = "support/anthropic.rs"]
mod anthropic;
#[path = "support/codex.rs"]
mod codex;
#[path = "support/http.rs"]
mod http;

use kou_domain::Json;
use kou_provider::{
    Request,
    responses::{StreamDecoder, StreamLimits, decode_response, prompt_cache_key, request_body},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Input {
    action: String,
    request: Request,
    cache_key: String,
    extensions: BTreeMap<String, Json>,
    body: Json,
    codex: Json,
    anthropic: Json,
    stream: String,
    chunk_size: usize,
    #[serde(rename = "HTTP")]
    http: http::Settings,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin().take(8 << 20).read_to_end(&mut bytes)?;
    let input: Input = serde_json::from_slice(&bytes)?;
    let result = match input.action.as_str() {
        "request" => request_body(
            &input.request,
            &prompt_cache_key(&input.cache_key),
            &input.extensions,
        )
        .map(|body| serde_json::from_slice::<Value>(&body).unwrap()),
        "decode" => decode_response(input.body.as_str().unwrap_or("{}").as_bytes())
            .map(|response| serde_json::to_value(response).unwrap()),
        "stream" => stream_result(&input),
        "http" => http::run(&input),
        "codex-config" => codex::run(&input.codex),
        action if action.starts_with("anthropic-") => anthropic::run(&input),
        _ => return Err("unsupported oracle action".into()),
    };
    let output = match result {
        Ok(result) => json!({"Result":result,"Error":""}),
        Err(error) => json!({"Result":Value::Null,"Error":error.to_string()}),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn stream_result(input: &Input) -> kou_provider::Result<Value> {
    let mut decoder = StreamDecoder::new(StreamLimits::default())?;
    let mut events = Vec::new();
    let chunk = if input.chunk_size == 0 {
        input.stream.len().max(1)
    } else {
        input.chunk_size
    };
    for bytes in input.stream.as_bytes().chunks(chunk) {
        if decoder.feed(bytes, |event| events.push(event))? {
            break;
        }
    }
    let body = decoder.into_body()?;
    let response = decode_response(body.get().as_bytes())?;
    Ok(json!({"Response":response,"Stream":events}))
}
