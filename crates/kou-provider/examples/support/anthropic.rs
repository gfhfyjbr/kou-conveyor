use kou_provider::{
    Error, Result,
    anthropic::{
        self, AttemptOptions, CancellationToken, Client, Config, PrefixReport, Reporter,
        RequestOptions, RespondError,
    },
    responses::{Retry, StreamEvent},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, Instant},
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Reply {
    status: u16,
    body: String,
    headers: BTreeMap<String, String>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Settings {
    official: bool,
    #[serde(rename = "BaseURL")]
    base_url: String,
    #[serde(rename = "APIKey")]
    api_key: Option<String>,
    max_attempts: Option<i64>,
    max_tokens: i64,
    #[serde(rename = "CacheTTL")]
    cache_ttl: String,
    thinking_turns: Option<i64>,
    server_fallbacks: Option<bool>,
    repeat: usize,
    replies: Vec<Reply>,
}
#[derive(Default)]
struct Reports {
    stream: Vec<StreamEvent>,
    prefixes: Vec<PrefixReport>,
    retries: Vec<Value>,
}
impl Reporter for Reports {
    fn stream(&mut self, event: StreamEvent) {
        self.stream.push(event);
    }
    fn prefix(&mut self, make: impl FnOnce() -> Result<PrefixReport>) -> Result<()> {
        self.prefixes.push(make()?);
        Ok(())
    }
    fn retry(&mut self, retry: Retry<'_>) {
        self.retries.push(json!({"Attempt":retry.attempt,"MaxAttempts":retry.max_attempts,"Delay":retry.delay.as_nanos() as u64,"Error":retry.error.to_string()}));
    }
}
const BODY: &str = r#"{"id":"msg","type":"message","role":"assistant","model":"fixture","content":[],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}"#;
fn stream(body: &str) -> String {
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{body}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}
fn selected_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    [
        "Content-Type",
        "Accept",
        "X-Api-Key",
        "Authorization",
        "Anthropic-Version",
        "Anthropic-Beta",
    ]
    .into_iter()
    .filter_map(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| (name.into(), value.into()))
    })
    .collect()
}
pub(super) fn run(input: &super::Input) -> Result<Value> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error(error.to_string()))?
        .block_on(run_async(input))
}
async fn run_async(input: &super::Input) -> Result<Value> {
    let mut settings: Settings = serde_json::from_str(input.anthropic.as_str().unwrap_or("{}"))
        .map_err(|error| Error(error.to_string()))?;
    let network = matches!(
        input.action.as_str(),
        "anthropic-http" | "anthropic-stream" | "anthropic-models"
    );
    if network && (settings.official || !settings.base_url.is_empty()) {
        return Err(Error(
            "oracle network actions use only their owned loopback server".into(),
        ));
    }
    if settings.replies.is_empty() {
        settings.replies.push(Reply {
            status: 200,
            body: if input.action == "anthropic-stream" {
                input.stream.clone()
            } else {
                stream(input.body.as_str().unwrap_or(BODY))
            },
            headers: BTreeMap::new(),
        });
    }
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let stop = CancellationToken::new();
    let (base, server) = if network {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| Error(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| Error(error.to_string()))?;
        let replies = std::mem::take(&mut settings.replies);
        let capture = captured.clone();
        let stop = stop.clone();
        let chunk_size = input.chunk_size;
        let model_listing = input.action == "anthropic-models";
        let server = tokio::spawn(async move {
            let work = async {
                loop {
                    let (mut socket, _) = listener.accept().await?;
                    let mut bytes = Vec::new();
                    let header_end = loop {
                        let mut chunk = [0; 8192];
                        let count = socket.read(&mut chunk).await?;
                        if count == 0 {
                            return Err(std::io::Error::other("oracle request EOF"));
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            break end + 4;
                        }
                        if bytes.len() > 64 << 10 {
                            return Err(std::io::Error::other("oracle header limit"));
                        }
                    };
                    let header = String::from_utf8_lossy(&bytes[..header_end]);
                    let path = header
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    let headers: BTreeMap<String, String> = header
                        .lines()
                        .skip(1)
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
                        .collect();
                    let length = headers
                        .get("content-length")
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or_default();
                    if length > 8 << 20 {
                        return Err(std::io::Error::other("oracle body limit"));
                    }
                    while bytes.len() < header_end + length {
                        let mut chunk = [0; 8192];
                        let count = socket.read(&mut chunk).await?;
                        if count == 0 {
                            return Err(std::io::Error::other("oracle body EOF"));
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                    }
                    let headers: BTreeMap<String, String> = [
                        "Content-Type",
                        "Accept",
                        "X-Api-Key",
                        "Authorization",
                        "Anthropic-Version",
                        "Anthropic-Beta",
                    ]
                    .into_iter()
                    .filter_map(|name| {
                        headers
                            .get(&name.to_lowercase())
                            .map(|value| (name.into(), value.clone()))
                    })
                    .collect();
                    let body: Value = if length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes[header_end..header_end + length])
                            .map_err(std::io::Error::other)?
                    };
                    let index = {
                        let mut capture = capture.lock().unwrap();
                        let index = capture.len();
                        capture.push(json!({"Body":body,"Headers":headers,"Path":path}));
                        index
                    };
                    let reply = &replies[index.min(replies.len() - 1)];
                    let status = if reply.status == 0 { 200 } else { reply.status };
                    let content_type = reply
                        .headers
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                        .map_or(
                            if model_listing {
                                "application/json"
                            } else {
                                "text/event-stream"
                            },
                            |(_, value)| value.as_str(),
                        );
                    let mut head = format!(
                        "HTTP/1.1 {status} fixture\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\n",
                        reply.body.len()
                    );
                    for (key, value) in &reply.headers {
                        if key.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                            return Err(std::io::Error::other("oracle header newline"));
                        }
                        if !key.eq_ignore_ascii_case("content-type") {
                            head.push_str(&format!("{key}: {value}\r\n"));
                        }
                    }
                    head.push_str("\r\n");
                    socket.write_all(head.as_bytes()).await?;
                    for chunk in reply.body.as_bytes().chunks(if chunk_size == 0 {
                        reply.body.len().max(1)
                    } else {
                        chunk_size
                    }) {
                        socket.write_all(chunk).await?;
                        tokio::task::yield_now().await;
                    }
                    socket.shutdown().await?;
                }
            };
            tokio::select! {_ = stop.cancelled()=>Ok(()),result=work=>result}
        });
        (format!("http://{address}/prefix/v1/"), Some(server))
    } else {
        (
            if !settings.base_url.is_empty() {
                settings.base_url.clone()
            } else if settings.official {
                anthropic::BASE_URL.into()
            } else {
                "http://messages.fixture.invalid/prefix/v1/".into()
            },
            None,
        )
    };
    let config = Config {
        api_key: settings.api_key.unwrap_or_else(|| "synthetic-key".into()),
        base_url: base,
        max_attempts: Some(settings.max_attempts.unwrap_or(1).max(0) as usize),
        max_tokens: settings.max_tokens,
        cache_ttl: settings.cache_ttl,
        thinking_turns: settings.thinking_turns,
        server_fallbacks: settings.server_fallbacks,
        ..Default::default()
    };
    let http = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| Error(error.to_string()))?;
    let client = Client::with_client(config, http);
    let mut reports = Reports::default();
    let (mut responses, mut errors, mut overflows, mut waits) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut listing = None;
    let result = if let Ok(client) = client {
        if input.action == "anthropic-models" {
            let result = client
                .list_models(
                    RequestOptions {
                        deadline: Some(Instant::now() + Duration::from_secs(3)),
                        ..Default::default()
                    },
                    &CancellationToken::new(),
                )
                .await;
            let (models, message, unavailable) = match result {
                Ok(models) => (json!(models), String::new(), false),
                Err(error) => (
                    json!([]),
                    error.to_string(),
                    matches!(error, RespondError::ModelsUnavailable),
                ),
            };
            listing = Some(json!({"Models":models,"Error":message,"Unavailable":unavailable}));
        } else {
            for _ in 0..settings.repeat.max(1) {
                let result = if network {
                    client
                        .respond(
                            &input.request,
                            RequestOptions {
                                deadline: Some(Instant::now() + Duration::from_secs(3)),
                                ..Default::default()
                            },
                            &CancellationToken::new(),
                            &mut reports,
                        )
                        .await
                } else {
                    (|| {
                        let wire = client
                            .prepare(&input.request, AttemptOptions::default())
                            .map_err(RespondError::Build)?;
                        let mut headers = selected_headers(client.headers());
                        if !wire.betas().is_empty() {
                            headers.insert("Anthropic-Beta".into(), wire.betas().join(","));
                        }
                        let body = wire.encode(32 << 20).map_err(RespondError::Build)?;
                        captured.lock().unwrap().push(json!({"Body":serde_json::from_slice::<Value>(&body).unwrap(),"Headers":headers,"Path":format!("{}?{}",client.endpoint().path(),client.endpoint().query().unwrap_or_default())}));
                        let response = anthropic::decode_response(
                            input.body.as_str().unwrap_or(BODY).as_bytes(),
                            &BTreeMap::new(),
                        )
                        .map_err(RespondError::Request)?;
                        reports
                            .prefix(|| {
                                Ok(PrefixReport {
                                    model: wire.model.into(),
                                    response: response.id.clone(),
                                    usage: response.usage.clone(),
                                    prefix: wire.prefix()?,
                                })
                            })
                            .map_err(RespondError::Build)?;
                        Ok(response)
                    })()
                };
                let (mut overflow, mut wait) = (Value::Null, Value::Null);
                if let Err(RespondError::ContextOverflow { tokens, limit, .. }) = &result {
                    overflow = json!({"Tokens":tokens,"Limit":limit});
                }
                if let Err(RespondError::RateLimited { retry_after, .. }) = &result {
                    wait = json!(retry_after.as_nanos() as u64);
                }
                overflows.push(overflow);
                waits.push(wait);
                match result {
                    Ok(response) => {
                        responses.push(response);
                        errors.push(String::new());
                    }
                    Err(error) => {
                        responses.push(kou_domain::Response::default());
                        errors.push(error.to_string());
                    }
                }
            }
        }
        Ok(())
    } else {
        Err(client.err().unwrap())
    };
    stop.cancel();
    if let Some(server) = server {
        server
            .await
            .map_err(|error| Error(error.to_string()))?
            .map_err(|error| Error(error.to_string()))?;
    }
    result?;
    let captured = captured.lock().unwrap();
    if let Some(mut listing) = listing {
        listing["Requests"] = json!(*captured);
        return Ok(listing);
    }
    Ok(
        json!({"Requests":*captured,"Responses":responses,"Errors":errors,"Stream":reports.stream,"Prefixes":reports.prefixes,"Retries":reports.retries,"Overflows":overflows,"Waits":waits}),
    )
}
