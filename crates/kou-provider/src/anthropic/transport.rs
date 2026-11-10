use super::{
    AttemptOptions, BASE_URL, PrefixReport, RequestConfig, StreamDecoder, StreamLimits,
    WireRequest,
    policy::{self, RespondError},
    prepare,
};
pub use crate::responses::RequestOptions;
use crate::{
    Error, History, RequestData, Result,
    responses::{Retry, StreamEvent},
};
use bytes::Bytes;
use kou_domain::{Json, LlmData, LlmItem, Reasoning, Response};
use reqwest::{
    Client as HttpClient, Url,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use std::{
    future::{Future, pending},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{sync::Semaphore, time::Instant};
pub use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub request_bytes: usize,
    pub error_bytes: usize,
    pub stream: StreamLimits,
    pub concurrent_requests: usize,
    pub model_pages: usize,
    pub listed_models: usize,
    pub model_name_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            request_bytes: 32 << 20,
            error_bytes: 1 << 20,
            stream: StreamLimits::default(),
            concurrent_requests: 8,
            model_pages: 100,
            listed_models: 100_000,
            model_name_bytes: 8 << 20,
        }
    }
}
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub max_attempts: Option<usize>,
    pub max_tokens: i64,
    pub cache_ttl: String,
    pub thinking_turns: Option<i64>,
    pub server_fallbacks: Option<bool>,
    pub response_idle_timeout: Duration,
    pub limits: Limits,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            max_attempts: None,
            max_tokens: 0,
            cache_ttl: String::new(),
            thinking_turns: None,
            server_fallbacks: None,
            response_idle_timeout: Duration::from_secs(10 * 60),
            limits: Limits::default(),
        }
    }
}
pub trait Reporter {
    fn stream(&mut self, _event: StreamEvent) {}
    fn retry(&mut self, _retry: Retry<'_>) {}
    fn prefix(&mut self, _make: impl FnOnce() -> Result<PrefixReport>) -> Result<()> {
        Ok(())
    }
}
impl Reporter for () {}
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}
struct Inner {
    http: HttpClient,
    endpoint: Url,
    headers: HeaderMap,
    request: RequestConfig,
    attempts: usize,
    idle: Duration,
    limits: Limits,
    admission: Semaphore,
    no_context: AtomicBool,
    no_fallbacks: AtomicBool,
}
impl Client {
    pub fn new(config: Config) -> Result<Self> {
        Self::with_client(config, crate::responses::http_client(false, false)?)
    }
    /// The caller owns TLS/proxy/redirect settings. Disable hidden retries and
    /// whole-body timeouts; this adapter owns attempts and per-read idle timers.
    pub fn with_client(config: Config, http: HttpClient) -> Result<Self> {
        let key = config.api_key.trim();
        if key.is_empty() {
            return Err(Error("Anthropic API key must be set".into()));
        }
        let base = config.base_url.trim();
        let base = if base.is_empty() { BASE_URL } else { base };
        let base = base.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let base = Url::parse(&format!("{base}/"))
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
            .ok_or_else(|| {
                Error(format!(
                    "invalid Messages API base URL {:?}",
                    config.base_url
                ))
            })?;
        let official = base
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"));
        let attempts = config.max_attempts.unwrap_or(5);
        if attempts == 0 {
            return Err(Error("max attempts must be positive".into()));
        }
        if config.max_tokens < 0 {
            return Err(Error("max tokens must not be negative".into()));
        }
        let ttl = config.cache_ttl.trim().to_lowercase();
        if !matches!(ttl.as_str(), "" | "5m" | "1h") {
            return Err(Error("cache TTL must be 5m or 1h".into()));
        }
        let turns = config.thinking_turns.unwrap_or(2);
        if turns < 0 {
            return Err(Error("thinking turns must not be negative".into()));
        }
        if config.limits.request_bytes == 0
            || config.limits.error_bytes == 0
            || config.limits.model_pages == 0
            || config.limits.listed_models == 0
            || config.limits.model_name_bytes == 0
            || !(1..=1024).contains(&config.limits.concurrent_requests)
            || config.response_idle_timeout.is_zero()
            || Instant::now()
                .checked_add(config.response_idle_timeout)
                .is_none()
        {
            return Err(Error("invalid Messages API transport limits".into()));
        }
        StreamDecoder::new(config.limits.stream)?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let mut secret = HeaderValue::from_str(key)
            .map_err(|_| Error("invalid Anthropic API key header".into()))?;
        secret.set_sensitive(true);
        headers.insert("x-api-key", secret);
        if !official {
            let mut secret = HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| Error("invalid Anthropic API key header".into()))?;
            secret.set_sensitive(true);
            headers.insert(AUTHORIZATION, secret);
        }
        let mut endpoint = base
            .join("v1/messages")
            .map_err(|_| Error("invalid Messages API endpoint".into()))?;
        endpoint.query_pairs_mut().append_pair("beta", "true");
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                endpoint,
                headers,
                request: RequestConfig {
                    official,
                    max_tokens: config.max_tokens,
                    cache_ttl: ttl,
                    thinking_turns: turns,
                    server_fallbacks: config.server_fallbacks.unwrap_or(official),
                },
                attempts,
                idle: config.response_idle_timeout,
                limits: config.limits,
                admission: Semaphore::new(config.limits.concurrent_requests),
                no_context: AtomicBool::new(false),
                no_fallbacks: AtomicBool::new(false),
            }),
        })
    }
    pub fn prepare<'a>(
        &'a self,
        request: &'a RequestData<impl History>,
        mut attempt: AttemptOptions,
    ) -> Result<WireRequest<'a>> {
        attempt.no_context_management |= self.inner.no_context.load(Ordering::Relaxed);
        attempt.no_fallbacks |= self.inner.no_fallbacks.load(Ordering::Relaxed);
        prepare(request, &self.inner.request, attempt)
    }
    pub async fn list_models(
        &self,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
    ) -> std::result::Result<Vec<String>, RespondError> {
        use serde::Deserialize;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Model {
            #[serde(deserialize_with = "crate::null_default")]
            id: String,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Page<'a> {
            #[serde(borrow, deserialize_with = "crate::null_default")]
            data: Vec<&'a serde_json::value::RawValue>,
            has_more: Option<bool>,
            #[serde(deserialize_with = "crate::null_default")]
            last_id: String,
        }
        let _permit = guarded(
            self.inner.admission.acquire(),
            cancellation,
            options.deadline,
            None,
        )
        .await?
        .map_err(|_| RespondError::Request(Error("Messages API transport is closed".into())))?;
        let mut models = Vec::new();
        let mut name_bytes = 0;
        let mut cursor = String::new();
        for _ in 0..self.inner.limits.model_pages {
            check_control(cancellation, options.deadline)?;
            let mut endpoint = self
                .inner
                .endpoint
                .join("models")
                .map_err(|error| RespondError::Build(Error(error.to_string())))?;
            if !cursor.is_empty() {
                endpoint.query_pairs_mut().append_pair("after_id", &cursor);
            }
            endpoint.query_pairs_mut().append_pair("limit", "1000");
            let mut headers = self.inner.headers.clone();
            headers.remove(CONTENT_TYPE);
            let mut response = guarded(
                self.inner.http.get(endpoint).headers(headers).send(),
                cancellation,
                options.deadline,
                Some(self.inner.idle),
            )
            .await?
            .map_err(RespondError::transport)?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let mut body = Vec::new();
            while let Some(chunk) = guarded(
                response.chunk(),
                cancellation,
                options.deadline,
                Some(self.inner.idle),
            )
            .await?
            .map_err(RespondError::transport)?
            {
                if chunk.len() > self.inner.limits.error_bytes - body.len() {
                    return Err(RespondError::Request(Error(
                        "Messages API model page exceeds the page-byte limit".into(),
                    )));
                }
                body.extend_from_slice(&chunk);
            }
            if matches!(status, 404 | 405) {
                return Err(RespondError::ModelsUnavailable);
            }
            if !(200..300).contains(&status) {
                return Err(RespondError::Api(policy::api_error(
                    status, &headers, &body,
                )));
            }
            let content_type = policy::header(&headers, "content-type");
            // The Go SDK keeps the base type even when MIME parameters are invalid.
            let is_json = content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .parse::<mime::Mime>()
                .ok()
                .is_some_and(|media| {
                    media.essence_str().contains("application/json")
                        || media.essence_str().ends_with("+json")
                });
            if !is_json {
                return Err(RespondError::Request(Error(format!(
                    "expected destination type of 'string' or '[]byte' for responses with content-type '{content_type}' that is not 'application/json'"
                ))));
            }
            let page: Page<'_> =
                crate::object_or_null(&mut serde_json::Deserializer::from_slice(&body))
                    .map_err(|error| RespondError::Request(Error(error.to_string())))?;
            let empty = page.data.is_empty();
            for raw in page.data {
                let model: Model =
                    crate::object_or_null(&mut serde_json::Deserializer::from_str(raw.get()))
                        .map_err(|error| RespondError::Request(Error(error.to_string())))?;
                if models.len() >= self.inner.limits.listed_models
                    || model.id.len() > self.inner.limits.model_name_bytes - name_bytes
                {
                    return Err(RespondError::Request(Error(
                        "Messages API model listing exceeds the model limit".into(),
                    )));
                }
                name_bytes += model.id.len();
                models.push(model.id);
            }
            if empty || page.has_more == Some(false) || page.last_id.is_empty() {
                return Ok(models);
            }
            cursor = page.last_id;
        }
        Err(RespondError::Request(Error(
            "Messages API model listing exceeds the page-count limit".into(),
        )))
    }
    pub fn endpoint(&self) -> &Url {
        &self.inner.endpoint
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.inner.headers
    }
    pub async fn respond(
        &self,
        request: &RequestData<impl History>,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
    ) -> std::result::Result<Response, RespondError> {
        let _permit = guarded(
            self.inner.admission.acquire(),
            cancellation,
            options.deadline,
            None,
        )
        .await?
        .map_err(|_| RespondError::Request(Error("Messages API transport is closed".into())))?;
        let mut attempt = 1;
        let mut settings = AttemptOptions::default();
        let mut reset = false;
        loop {
            check_control(cancellation, options.deadline)?;
            let wire = self
                .prepare(request, settings)
                .map_err(RespondError::Build)?;
            if reset {
                reporter.stream(StreamEvent {
                    index: 0,
                    kind: String::new(),
                    text: String::new(),
                    reset: true,
                });
                check_control(cancellation, options.deadline)?;
            }
            let body = Bytes::from(
                wire.encode(self.inner.limits.request_bytes)
                    .map_err(RespondError::Build)?,
            );
            let mut headers = self.inner.headers.clone();
            if !wire.betas().is_empty() {
                headers.insert(
                    "anthropic-beta",
                    HeaderValue::from_str(&wire.betas().join(",")).unwrap(),
                );
            }
            let (outcome, headers) = self
                .exchange(body, headers, options, cancellation, reporter)
                .await;
            let outcome: std::result::Result<(), RespondError> = match outcome {
                Ok((mut response, true)) => {
                    check_control(cancellation, options.deadline)?;
                    reporter
                        .prefix(|| {
                            Ok(PrefixReport {
                                model: wire.model.into(),
                                response: response.id.clone(),
                                usage: response.usage.clone(),
                                prefix: wire.prefix()?,
                            })
                        })
                        .map_err(RespondError::Build)?;
                    check_control(cancellation, options.deadline)?;
                    if !settings.keep_thinking {
                        response.output.get_or_insert_with(Vec::new).insert(
                            0,
                            LlmItem {
                                provider_id: String::new(),
                                model: String::new(),
                                data: LlmData::Reasoning(Reasoning {
                                    summary: None,
                                    raw: Json::parse(r#"{"type":"thinking_reset"}"#).unwrap(),
                                }),
                            },
                        );
                    }
                    return Ok(response);
                }
                Ok((_, false)) => Err(RespondError::Request(Error(
                    super::stream::UNEXPECTED_EOF.into(),
                ))),
                Err(error) => Err(error),
            };
            let error = outcome.unwrap_err();
            check_control(cancellation, options.deadline)?;
            if let Some(api) = error.api_error() {
                if settings.keep_thinking && policy::thinking_rejected(api) {
                    settings.keep_thinking = false;
                    reset = true;
                    continue;
                }
                if wire.has_context_management() && policy::context_rejected(api) {
                    self.inner.no_context.store(true, Ordering::Relaxed);
                    reset = true;
                    continue;
                }
                if wire.has_fallbacks() && policy::fallbacks_rejected(api) {
                    self.inner.no_fallbacks.store(true, Ordering::Relaxed);
                    reset = true;
                    continue;
                }
                if api.status_code == 400 {
                    if let Some(room) = policy::response_room(&api.message, wire.max_tokens) {
                        settings.room = room;
                        reset = true;
                        continue;
                    }
                    if let Some((tokens, limit)) = policy::overflow(api) {
                        return Err(RespondError::ContextOverflow {
                            tokens,
                            limit,
                            source: Box::new(error),
                        });
                    }
                }
                if let Some(wait) = policy::rate_wait(api, &headers, SystemTime::now())
                    && (wait > Duration::from_secs(60) || attempt == self.inner.attempts)
                {
                    return Err(RespondError::RateLimited {
                        retry_after: wait,
                        source: Box::new(error),
                    });
                }
                if api.status_code == 413 {
                    return Err(RespondError::ContextOverflow {
                        tokens: 0,
                        limit: 0,
                        source: Box::new(error),
                    });
                }
            }
            if attempt == self.inner.attempts || !error.retryable() {
                return Err(error);
            }
            let delay = policy::retry_delay(
                attempt,
                &error,
                &headers,
                SystemTime::now(),
                rand::random::<f64>(),
            );
            reporter.retry(Retry {
                attempt,
                max_attempts: self.inner.attempts,
                delay,
                error: &error,
            });
            check_control(cancellation, options.deadline)?;
            drop(error);
            guarded(
                tokio::time::sleep(delay),
                cancellation,
                options.deadline,
                None,
            )
            .await?;
            attempt += 1;
            reset = true;
        }
    }
    async fn exchange(
        &self,
        body: Bytes,
        headers: HeaderMap,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
    ) -> (
        std::result::Result<(Response, bool), RespondError>,
        HeaderMap,
    ) {
        let mut received_headers = HeaderMap::new();
        let result = async {
            let idle = Some(self.inner.idle);
            let mut response = guarded(
                self.inner
                    .http
                    .post(self.inner.endpoint.clone())
                    .headers(headers)
                    .body(body)
                    .send(),
                cancellation,
                options.deadline,
                idle,
            )
            .await?
            .map_err(RespondError::transport)?;
            received_headers = response.headers().clone();
            let status = response.status().as_u16();
            if !response.status().is_success() {
                let mut bytes = Vec::new();
                while let Some(chunk) =
                    guarded(response.chunk(), cancellation, options.deadline, idle)
                        .await?
                        .map_err(RespondError::transport)?
                {
                    if chunk.len() > self.inner.limits.error_bytes - bytes.len() {
                        return Err(RespondError::Request(Error(
                            "Messages API error response exceeds the error-byte limit".into(),
                        )));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                return Err(RespondError::Api(policy::api_error(
                    status,
                    &received_headers,
                    &bytes,
                )));
            }
            let mut decoder =
                StreamDecoder::new(self.inner.limits.stream).map_err(RespondError::Request)?;
            while let Some(chunk) = guarded(response.chunk(), cancellation, options.deadline, idle)
                .await?
                .map_err(RespondError::transport)?
            {
                for bytes in chunk.chunks(32 << 10) {
                    check_control(cancellation, options.deadline)?;
                    let stop = decoder
                        .feed(bytes, |event| {
                            if check_control(cancellation, options.deadline).is_ok() {
                                reporter.stream(event);
                            }
                        })
                        .map_err(RespondError::Request)?;
                    check_control(cancellation, options.deadline)?;
                    if stop && let Some(raw) = decoder.api_error() {
                        return Err(RespondError::Api(policy::api_error(
                            status,
                            &received_headers,
                            raw,
                        )));
                    }
                }
            }
            let complete = decoder.is_complete();
            let response = decoder.finish_partial().map_err(RespondError::Request)?;
            Ok((response, complete))
        }
        .await;
        (result, received_headers)
    }
}
fn check_control(
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> std::result::Result<(), RespondError> {
    if cancellation.is_cancelled() {
        return Err(RespondError::Cancelled);
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(RespondError::DeadlineExceeded);
    }
    Ok(())
}
async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => pending().await,
    }
}
async fn guarded<T>(
    future: impl Future<Output = T>,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
    idle: Option<Duration>,
) -> std::result::Result<T, RespondError> {
    check_control(cancellation, deadline)?;
    let idle = idle.and_then(|duration| Instant::now().checked_add(duration));
    tokio::select! {biased;
        _=cancellation.cancelled()=>Err(RespondError::Cancelled),
        _=at(deadline)=>Err(RespondError::DeadlineExceeded),
        _=at(idle)=>Err(RespondError::IdleTimeout),
        output=future=>Ok(output),
    }
}
