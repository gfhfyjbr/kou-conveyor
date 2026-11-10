use super::{
    ApiError, ResponseState, StreamDecoder, StreamEvent, StreamLimits, decode_response,
    policy::{
        RespondError, RetryPolicy, context_overflow, images_rejected, provider_error,
        retryable_error, usage_limit,
    },
    prompt_cache_key,
    request::request_body_limited,
};
use crate::{Error, History, RequestData, Result, sse};
use bytes::Bytes;
use kou_domain::{Json, LlmData, Response};
use reqwest::{
    Client, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::value::RawValue;
use std::{
    collections::{BTreeMap, VecDeque},
    future::{Future, pending},
    sync::{Arc, Mutex},
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
    pub text_only_models: usize,
    pub text_only_model_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            request_bytes: 32 << 20,
            error_bytes: 1 << 20,
            stream: StreamLimits::default(),
            concurrent_requests: 8,
            text_only_models: 256,
            text_only_model_bytes: 64 << 10,
        }
    }
}

pub struct Config {
    pub endpoint: String,
    pub headers: HeaderMap,
    pub cache_key_header: Option<HeaderName>,
    pub prompt_cache_key_field: bool,
    pub extensions: BTreeMap<String, Json>,
    pub retry: RetryPolicy,
    pub response_idle_timeout: Duration,
    pub limits: Limits,
}
impl Config {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            headers: HeaderMap::new(),
            cache_key_header: None,
            prompt_cache_key_field: false,
            extensions: BTreeMap::new(),
            retry: RetryPolicy::default(),
            response_idle_timeout: Duration::from_secs(30 * 60),
            limits: Limits::default(),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct RequestOptions<'a> {
    pub cache_key: &'a str,
    pub deadline: Option<Instant>,
}
pub struct Retry<'a> {
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay: Duration,
    pub error: &'a (dyn std::error::Error + Send + Sync),
}
pub struct Exchange<'a> {
    pub request_body: &'a [u8],
    pub status_code: u16,
    pub response_body: &'a [u8],
}

/// Callbacks borrow the exchange and run synchronously, outside network idle
/// deadlines. Implementations must not retain an unbounded event history.
pub trait Reporter {
    fn stream(&mut self, _event: StreamEvent) {}
    fn warning(&mut self, _message: &str) {}
    fn retry(&mut self, _retry: Retry<'_>) {}
    fn trace(&mut self, _exchange: Exchange<'_>) {}
}
impl Reporter for () {}

#[derive(Clone)]
pub struct Adapter {
    inner: Arc<Inner>,
}
struct Inner {
    client: Client,
    endpoint: Url,
    config: Config,
    admission: Semaphore,
    text_only: Mutex<TextOnly>,
}
#[derive(Default)]
struct TextOnly {
    models: VecDeque<String>,
    bytes: usize,
}
impl TextOnly {
    fn insert(&mut self, model: &str, limits: Limits) {
        if limits.text_only_models == 0
            || model.len() > limits.text_only_model_bytes
            || self.models.iter().any(|id| id == model)
        {
            return;
        }
        while self.models.len() >= limits.text_only_models
            || model.len() > limits.text_only_model_bytes - self.bytes
        {
            let Some(old) = self.models.pop_front() else {
                break;
            };
            self.bytes -= old.len();
        }
        self.models.push_back(model.into());
        self.bytes += model.len();
    }
}
pub(crate) fn http_client(follow_redirects: bool, bypass_proxy: bool) -> Result<Client> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(30))
        .retry(reqwest::retry::never());
    if !follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    if bypass_proxy {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|error| {
        Error(format!(
            "create responses HTTP client: {}",
            error.without_url()
        ))
    })
}
impl Adapter {
    pub fn new(config: Config) -> Result<Self> {
        Self::with_client(config, http_client(true, false)?)
    }
    /// The supplied client owns TLS/proxy/redirect settings. To keep the adapter's
    /// attempt count authoritative, disable its automatic retry policy. Do not
    /// configure a whole-body timeout that also counts reporter processing time.
    pub fn with_client(config: Config, client: Client) -> Result<Self> {
        config.retry.validate()?;
        if config.endpoint.trim().is_empty() {
            return Err(Error("responses API endpoint must be set".into()));
        }
        let endpoint = Url::parse(&config.endpoint)
            .map_err(|_| Error("invalid responses API endpoint".into()))?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
            return Err(Error(
                "responses API endpoint must be an HTTP(S) URL".into(),
            ));
        }
        if config.response_idle_timeout.is_zero()
            || Instant::now()
                .checked_add(config.response_idle_timeout)
                .is_none()
        {
            return Err(Error(
                "response idle timeout must be positive and representable".into(),
            ));
        }
        let limits = config.limits;
        if limits.request_bytes == 0
            || limits.error_bytes == 0
            || !(1..=1024).contains(&limits.concurrent_requests)
        {
            return Err(Error("invalid responses transport limits".into()));
        }
        ResponseState::new(limits.stream)?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                endpoint,
                admission: Semaphore::new(limits.concurrent_requests),
                config,
                text_only: Mutex::new(TextOnly::default()),
            }),
        })
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
        .map_err(|_| Error("responses transport is closed".into()))?;
        check_control(cancellation, options.deadline)?;
        let without_images = self
            .inner
            .text_only
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .models
            .iter()
            .any(|id| id == &request.model.id);
        let mut result = self
            .respond_once(request, options, cancellation, reporter, without_images)
            .await;
        let rejection = match &result {
            Err(error) => error.api_error().map(|error| {
                (
                    error.code.as_str(),
                    error.message.as_str(),
                    images_rejected(error),
                )
            }),
            Ok(response) => response.failure.as_ref().map(|failure| {
                let error = ApiError {
                    code: failure.code.clone(),
                    message: failure.message.clone(),
                    ..ApiError::default()
                };
                (
                    failure.code.as_str(),
                    failure.message.as_str(),
                    images_rejected(&error),
                )
            }),
        };
        if !without_images
            && has_images(request)
            && let Some((_, message, true)) = rejection
        {
            self.inner
                .text_only
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(&request.model.id, self.inner.config.limits);
            reporter.warning(&format!("the model {} does not accept images; they are left out of its requests from now on ({message})", request.model.id));
            reporter.stream(reset());
            // Drop the rejected body/error before constructing another request.
            drop(result);
            result = self
                .respond_once(request, options, cancellation, reporter, true)
                .await;
        }
        match result {
            Err(error) => Err(context_overflow(error)),
            Ok(response) => {
                if let Some(failure) = &response.failure
                    && failure.code == "context_length_exceeded"
                {
                    return Err(context_overflow(RespondError::api(
                        ApiError {
                            status_code: 200,
                            code: failure.code.clone(),
                            message: failure.message.clone(),
                            ..ApiError::default()
                        },
                        false,
                    )));
                }
                Ok(response)
            }
        }
    }
    async fn respond_once(
        &self,
        request: &RequestData<impl History>,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
        without_images: bool,
    ) -> std::result::Result<Response, RespondError> {
        check_control(cancellation, options.deadline)?;
        let config = &self.inner.config;
        let key = prompt_cache_key(options.cache_key);
        let body = Bytes::from(request_body_limited(
            request,
            if config.prompt_cache_key_field {
                &key
            } else {
                ""
            },
            &config.extensions,
            without_images,
            config.limits.request_bytes,
        )?);
        let mut headers = config.headers.clone();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(header) = &config.cache_key_header
            && !key.is_empty()
        {
            headers.insert(
                header,
                HeaderValue::from_str(&key).map_err(|error| Error(error.to_string()))?,
            );
        }
        for attempt in 1..=config.retry.max_attempts {
            check_control(cancellation, options.deadline)?;
            if attempt > 1 {
                reporter.stream(reset());
            }
            let result = self
                .exchange_attempt(
                    body.clone(),
                    headers.clone(),
                    options,
                    cancellation,
                    reporter,
                )
                .await;
            if !result.retryable() || attempt == config.retry.max_attempts {
                let now = SystemTime::now();
                if !matches!(result.outcome, Outcome::Terminal { .. })
                    && let Some(delay) = result
                        .api_error()
                        .and_then(|error| usage_limit(error, &result.headers, now))
                {
                    return Err(RespondError::RateLimited {
                        retry_after: delay,
                        source: Box::new(result.into_error()),
                    });
                }
                match result.outcome {
                    Outcome::Terminal { body: response, .. } => {
                        reporter.trace(Exchange {
                            request_body: &body,
                            status_code: result.status,
                            response_body: response.get().as_bytes(),
                        });
                        return decode_response(response.get().as_bytes()).map_err(Into::into);
                    }
                    Outcome::Http {
                        body: response,
                        error,
                    } => {
                        reporter.trace(Exchange {
                            request_body: &body,
                            status_code: result.status,
                            response_body: &response,
                        });
                        return Err(RespondError::api(error, true));
                    }
                    Outcome::Failure { error, .. } => return Err(error),
                }
            }
            check_control(cancellation, options.deadline)?;
            let delay = config.retry.delay(
                attempt,
                result.api_error(),
                &result.headers,
                SystemTime::now(),
                rand::random::<f64>(),
            );
            if let Some(error) = result.reason() {
                reporter.retry(Retry {
                    attempt,
                    max_attempts: config.retry.max_attempts,
                    delay,
                    error,
                });
            }
            drop(result);
            guarded(
                tokio::time::sleep(delay),
                cancellation,
                options.deadline,
                None,
            )
            .await?;
        }
        unreachable!("validated nonzero attempt count")
    }
    async fn exchange_attempt(
        &self,
        body: Bytes,
        headers: HeaderMap,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
    ) -> Attempt {
        let mut attempt = Attempt {
            status: 0,
            headers: HeaderMap::new(),
            outcome: Outcome::Failure {
                error: Error("responses request did not start".into()).into(),
                retry: false,
            },
        };
        let idle = Some(self.inner.config.response_idle_timeout);
        let send = self
            .inner
            .client
            .post(self.inner.endpoint.clone())
            .headers(headers)
            .body(body)
            .send();
        let mut response = match guarded(send, cancellation, options.deadline, idle).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return attempt.failed(RespondError::transport(error), true),
            Err(error) => return attempt.control_failed(error),
        };
        attempt.status = response.status().as_u16();
        attempt.headers = response.headers().clone();
        if !response.status().is_success() {
            let limit = self.inner.config.limits.error_bytes;
            let mut body = Vec::new();
            loop {
                match guarded(response.chunk(), cancellation, options.deadline, idle).await {
                    Ok(Ok(Some(bytes))) => {
                        if bytes.len() > limit - body.len() {
                            let message = if limit == 1 << 20 {
                                "responses API error response exceeds 1 MiB"
                            } else {
                                "responses API error response exceeds the error-byte limit"
                            };
                            return attempt.failed(Error(message.into()).into(), false);
                        }
                        body.extend_from_slice(&bytes);
                    }
                    Ok(Ok(None)) | Ok(Err(_)) | Err(RespondError::IdleTimeout) => break,
                    Err(error) => return attempt.control_failed(error),
                }
            }
            let error = provider_error(attempt.status, &body, SystemTime::now());
            attempt.outcome = Outcome::Http { body, error };
            return attempt;
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let framed = content_type.is_empty()
            || content_type
                .parse::<mime::Mime>()
                .ok()
                .is_some_and(|mime| mime.essence_str() == "text/event-stream");
        let mut decoder = if framed {
            match StreamDecoder::new(self.inner.config.limits.stream) {
                Ok(decoder) => Decoder::Framed(decoder),
                Err(error) => return attempt.failed(error.into(), false),
            }
        } else {
            match ResponseState::new(self.inner.config.limits.stream) {
                Ok(state) => Decoder::Raw(state),
                Err(error) => return attempt.failed(error.into(), false),
            }
        };
        loop {
            match guarded(response.chunk(), cancellation, options.deadline, idle).await {
                Ok(Ok(Some(bytes))) => {
                    for bytes in bytes.chunks(32 << 10) {
                        if let Err(error) = check_control(cancellation, options.deadline) {
                            return attempt.control_failed(error);
                        }
                        match decoder.feed(bytes, |event| {
                            if check_control(cancellation, options.deadline).is_ok() {
                                reporter.stream(event);
                            }
                        }) {
                            Ok(true) => return attempt.decoded(decoder),
                            Ok(false) => {}
                            Err(error) => return attempt.failed(error.into(), false),
                        }
                    }
                }
                Ok(Ok(None)) => return attempt.decoded(decoder),
                Ok(Err(error)) => return attempt.failed(RespondError::transport(error), true),
                Err(error) => return attempt.control_failed(error),
            }
        }
    }
}
fn reset() -> StreamEvent {
    StreamEvent {
        index: 0,
        kind: String::new(),
        text: String::new(),
        reset: true,
    }
}
fn has_images(request: &RequestData<impl History>) -> bool {
    request.input.iter().any(|item| match &item.data {
        LlmData::Message(message) => message
            .images
            .as_ref()
            .is_some_and(|images| !images.is_empty()),
        LlmData::ToolResult(result) => result.output.iter().any(|part| part.kind == "image"),
        _ => false,
    })
}

enum Decoder {
    Framed(StreamDecoder),
    Raw(ResponseState),
}
impl Decoder {
    fn state(&self) -> &ResponseState {
        match self {
            Self::Framed(decoder) => decoder.state(),
            Self::Raw(state) => state,
        }
    }
    fn feed(&mut self, bytes: &[u8], report: impl FnMut(StreamEvent)) -> Result<bool> {
        match self {
            Self::Framed(decoder) => decoder.feed(bytes, report),
            Self::Raw(state) => {
                if let Some(data) = sse::data(bytes) {
                    let data = std::str::from_utf8(&data)
                        .map_err(|error| {
                            Error(format!("invalid Responses stream event JSON: {error}"))
                        })?
                        .trim();
                    if !data.is_empty() && data != "[DONE]" {
                        state.observe(data.as_bytes(), report)?;
                    }
                }
                Ok(state.is_finished())
            }
        }
    }
    fn into_body(self) -> Result<Box<RawValue>> {
        match self {
            Self::Framed(decoder) => decoder.into_body(),
            Self::Raw(state) => state.into_body(),
        }
    }
}
struct Attempt {
    status: u16,
    headers: HeaderMap,
    outcome: Outcome,
}
enum Outcome {
    Terminal {
        body: Box<RawValue>,
        failure: Option<ApiError>,
    },
    Http {
        body: Vec<u8>,
        error: ApiError,
    },
    Failure {
        error: RespondError,
        retry: bool,
    },
}
impl Attempt {
    fn failed(mut self, error: RespondError, retry: bool) -> Self {
        self.outcome = Outcome::Failure { error, retry };
        self
    }
    fn control_failed(self, error: RespondError) -> Self {
        let retry = matches!(error, RespondError::IdleTimeout);
        self.failed(error, retry)
    }
    fn decoded(mut self, decoder: Decoder) -> Self {
        if let Some(error) = decoder.state().api_error() {
            let error = error.clone();
            let retry = retryable_error(&error);
            return self.failed(RespondError::api(error, false), retry);
        }
        let failed = decoder.state().failed_terminal();
        match decoder.into_body() {
            Ok(body) => {
                let failure =
                    failed.then(|| provider_error(200, body.get().as_bytes(), SystemTime::now()));
                self.outcome = Outcome::Terminal { body, failure };
                self
            }
            Err(error) => {
                let retry =
                    error.0 == "responses stream ended without a terminal response: unexpected EOF";
                self.failed(error.into(), retry)
            }
        }
    }
    fn api_error(&self) -> Option<&ApiError> {
        match &self.outcome {
            Outcome::Terminal { failure, .. } => failure.as_ref(),
            Outcome::Http { error, .. } => Some(error),
            Outcome::Failure { error, .. } => error.api_error(),
        }
    }
    fn retryable(&self) -> bool {
        match &self.outcome {
            Outcome::Terminal { failure, .. } => failure.as_ref().is_some_and(retryable_error),
            Outcome::Http { error, .. } => retryable_error(error),
            Outcome::Failure { retry, .. } => *retry,
        }
    }
    fn reason(&self) -> Option<&(dyn std::error::Error + Send + Sync)> {
        match &self.outcome {
            Outcome::Terminal { failure, .. } => failure.as_ref().map(|error| error as _),
            Outcome::Http { error, .. } => Some(error),
            Outcome::Failure { error, .. } => Some(error),
        }
    }
    fn into_error(self) -> RespondError {
        match self.outcome {
            Outcome::Http { error, .. } => RespondError::api(error, true),
            Outcome::Failure { error, .. } => error,
            Outcome::Terminal { .. } => {
                unreachable!("terminal responses are not usage-limit errors")
            }
        }
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
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(RespondError::Cancelled),
        _ = at(deadline) => Err(RespondError::DeadlineExceeded),
        _ = at(idle) => Err(RespondError::IdleTimeout),
        output = future => Ok(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_only_cache_is_count_and_byte_bounded() {
        let mut cache = TextOnly::default();
        let limits = Limits {
            text_only_models: 2,
            text_only_model_bytes: 5,
            ..Limits::default()
        };
        for model in ["aa", "bbb", "c", "dd", "too-long-to-cache", "dd"] {
            cache.insert(model, limits);
            assert!(cache.models.len() <= 2 && cache.bytes <= 5);
        }
        assert_eq!(
            cache.models.iter().map(String::as_str).collect::<Vec<_>>(),
            ["c", "dd"]
        );
        assert_eq!(cache.bytes, 3);
        let mut cache = TextOnly::default();
        cache.insert(
            "m",
            Limits {
                text_only_models: 0,
                ..limits
            },
        );
        assert!(cache.models.is_empty());
    }
}
