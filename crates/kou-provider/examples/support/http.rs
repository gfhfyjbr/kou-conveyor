use kou_provider::responses::{
    Adapter, CancellationToken, Config, Exchange, Reporter, RequestOptions, RespondError, Retry,
    StreamEvent,
};
use reqwest::{
    Client,
    header::{HeaderName, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    task::JoinSet,
    time::Instant,
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(super) struct Settings {
    replies: Vec<Reply>,
    max_attempts: usize,
    repeat: usize,
    family: String,
    #[serde(rename = "CacheTTL")]
    cache_ttl: String,
}
#[derive(Default)]
struct Captured {
    requests: Vec<Value>,
    cache_headers: Vec<String>,
    request_headers: Vec<BTreeMap<String, String>>,
}
enum Provider {
    Responses(Adapter),
    Client(kou_provider::clients::Client),
    Codex(kou_provider::codex::Client),
}
impl Provider {
    async fn respond(
        &self,
        request: &kou_provider::Request,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut impl Reporter,
    ) -> Result<kou_domain::Response, RespondError> {
        match self {
            Self::Responses(adapter) => {
                adapter
                    .respond(request, options, cancellation, reporter)
                    .await
            }
            Self::Client(client) => {
                client
                    .respond(request, options, cancellation, reporter)
                    .await
            }
            Self::Codex(client) => {
                client
                    .respond(request, options, cancellation, reporter)
                    .await
            }
        }
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Reply {
    status: u16,
    body: String,
    retry_after: String,
}
#[derive(Default)]
struct Report {
    stream: Vec<StreamEvent>,
    warnings: Vec<String>,
    retries: Vec<Value>,
    traces: Vec<Value>,
}
impl Reporter for Report {
    fn stream(&mut self, event: StreamEvent) {
        self.stream.push(event);
    }
    fn warning(&mut self, message: &str) {
        self.warnings.push(message.into());
    }
    fn retry(&mut self, retry: Retry<'_>) {
        self.retries.push(json!({"Attempt":retry.attempt,"MaxAttempts":retry.max_attempts,"Delay":retry.delay.as_nanos() as i64,"Error":retry.error.to_string()}));
    }
    fn trace(&mut self, trace: Exchange<'_>) {
        let response = if kou_domain::validate_json(trace.response_body).is_ok() {
            serde_json::from_slice::<Value>(trace.response_body).unwrap()
        } else {
            Value::String(String::from_utf8_lossy(trace.response_body).into_owned())
        };
        self.traces.push(json!({"Request":serde_json::from_slice::<Value>(trace.request_body).unwrap(),"Response":response,"Status":trace.status_code}));
    }
}
fn unix_nanos(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_nanos()).ok(),
        Err(error) => i64::try_from(error.duration().as_nanos())
            .ok()
            .and_then(i64::checked_neg),
    }
}
fn rate_limit(error: &RespondError) -> Option<Value> {
    match error {
        RespondError::RateLimited { retry_after, .. } => {
            Some(json!({"RetryAfter":retry_after.as_nanos() as i64}))
        }
        RespondError::ContextOverflow { source, .. } => rate_limit(source),
        _ => None,
    }
}
fn overflow(error: &RespondError) -> Option<Value> {
    match error {
        RespondError::ContextOverflow { tokens, limit, .. } => {
            Some(json!({"Tokens":tokens,"Limit":limit}))
        }
        RespondError::RateLimited { source, .. } => overflow(source),
        _ => None,
    }
}
pub(super) fn run(input: &super::Input) -> kou_provider::Result<Value> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| kou_provider::Error(error.to_string()))?;
    runtime
        .block_on(run_async(input))
        .map_err(|error| kou_provider::Error(error.to_string()))
}
async fn run_async(input: &super::Input) -> Result<Value, Box<dyn std::error::Error>> {
    let attempts = input.http.max_attempts.max(1);
    let repeat = input.http.repeat.max(1);
    if attempts > 3 || repeat > 3 || input.http.replies.is_empty() || input.http.replies.len() > 9 {
        return Err("invalid local HTTP oracle limits".into());
    }
    for reply in &input.http.replies {
        HeaderValue::from_str(&reply.retry_after)?;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}/responses", listener.local_addr()?);
    let captured = Arc::new(Mutex::new(Captured::default()));
    let server = {
        let captured = captured.clone();
        let replies = input.http.replies.clone();
        tokio::spawn(async move {
            let mut index = 0;
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let Ok((stream,_))=accepted else{break;};
                        let reply=replies[index.min(replies.len()-1)].clone();index+=1;
                        let captured=captured.clone();
                        children.spawn(async move{
                            let mut stream=BufReader::new(stream);
                            let mut line=String::new();
                            if stream.read_line(&mut line).await.unwrap_or(0)==0{return;}
                            if !line.starts_with("POST /responses HTTP/1.1"){return;}
                            let mut headers=BTreeMap::new();
                            loop {
                                line.clear();
                                if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                                    return;
                                }
                                if line == "\r\n" {
                                    break;
                                }
                                let Some((name, value)) = line.split_once(':') else { return; };
                                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
                            }
                            let length=headers.get("content-length").and_then(|value|value.parse::<usize>().ok()).unwrap_or(0);
                            if length>8<<20{return;}
                            let mut body=vec![0;length];if stream.read_exact(&mut body).await.is_err(){return;}
                            let Ok(body)=serde_json::from_slice(&body)else{return;};
                            {
                                let mut captured = captured.lock().unwrap();
                                captured.requests.push(body);
                                captured.cache_headers.push(headers.get("x-cache-key").cloned().unwrap_or_default());
                                let mut selected: BTreeMap<_, _> = ["authorization", "content-type", "accept", "x-cache-key", "x-session-id", "x-session-affinity", "chatgpt-account-id", "session-id", "originator"]
                                    .into_iter().map(|name| (name.to_owned(), headers.get(name).cloned().unwrap_or_default())).collect();
                                if headers.contains_key("originator") { selected.insert("user-agent".into(), headers.get("user-agent").cloned().unwrap_or_default()); }
                                captured.request_headers.push(selected);
                            }
                            let status=if reply.status==0{200}else{reply.status};
                            let content_type=if(200..300).contains(&status){"text/event-stream"}else{"application/json"};
                            let retry=if reply.retry_after.is_empty(){String::new()}else{format!("Retry-After: {}\r\n",reply.retry_after)};
                            let headers=format!("HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nConnection: close\r\nContent-Length: {}\r\n{retry}\r\n",reply.body.len());
                            let mut stream=stream.into_inner();if stream.write_all(headers.as_bytes()).await.is_ok(){let _=stream.write_all(reply.body.as_bytes()).await;}
                        });
                    }
                    _=children.join_next(),if !children.is_empty()=>{}
                }
            }
        })
    };
    let mut config = Config::new(&endpoint);
    config.extensions = input.extensions.clone();
    config.retry.max_attempts = attempts;
    config.cache_key_header = Some(HeaderName::from_static("x-cache-key"));
    config.prompt_cache_key_field = true;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let adapter = if input.http.family.is_empty() {
        Provider::Responses(Adapter::with_client(config, client)?)
    } else if input.http.family == "codex" {
        Provider::Codex(kou_provider::codex::Client::new(
            kou_provider::codex::Config {
                access_token: "fixture-subscription-token".into(),
                account_id: "fixture-account".into(),
                base_url: endpoint.strip_suffix("/responses").unwrap().into(),
                max_attempts: Some(attempts),
                ..kou_provider::codex::Config::default()
            },
        )?)
    } else {
        use kou_provider::clients::{Config, Service};
        let service = match input.http.family.as_str() {
            "openai" => Service::OpenAi,
            "fireworks" => Service::Fireworks,
            "openrouter" => Service::OpenRouter,
            "ollama" => Service::Ollama,
            _ => return Err("unknown local HTTP oracle family".into()),
        };
        let mut config = Config::new(service);
        config.api_key = "fixture-key".into();
        config.base_url = endpoint.strip_suffix("/responses").unwrap().into();
        config.max_attempts = Some(attempts);
        config.cache_ttl.clone_from(&input.http.cache_ttl);
        Provider::Client(kou_provider::clients::Client::with_client(config, client)?)
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut calls = Vec::new();
    let mut traces = Vec::new();
    for _ in 0..repeat {
        let mut report = Report::default();
        let result = adapter
            .respond(
                &input.request,
                RequestOptions {
                    cache_key: &input.cache_key,
                    deadline: Some(deadline),
                },
                &CancellationToken::new(),
                &mut report,
            )
            .await;
        let mut call = json!({"Response":Value::Null,"Error":"","API":Value::Null,"RateLimit":Value::Null,"Overflow":Value::Null,
            "Stream":report.stream,"Warnings":report.warnings,"Retries":report.retries});
        match result {
            Ok(response) => call["Response"] = serde_json::to_value(response)?,
            Err(error) => {
                call["Error"] = error.to_string().into();
                if let Some(api) = error.api_error() {
                    call["API"] = json!({"Status":api.status_code,"Code":api.code,"Message":api.message,"Param":api.param,"Type":api.kind,"ResetUnixNanos":api.reset_at.and_then(unix_nanos)});
                }
                call["RateLimit"] = rate_limit(&error).unwrap_or(Value::Null);
                call["Overflow"] = overflow(&error).unwrap_or(Value::Null);
            }
        }
        calls.push(call);
        traces.extend(report.traces);
    }
    server.abort();
    let _ = server.await;
    let captured = captured.lock().unwrap();
    Ok(
        json!({"Calls":calls,"Requests":captured.requests,"CacheHeaders":captured.cache_headers,"RequestHeaders":captured.request_headers,"Traces":traces}),
    )
}
