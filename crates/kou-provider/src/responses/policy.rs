use super::ApiError;
use chrono::DateTime;
use kou_domain::{Json, validate_json};
use regex::Regex;
use reqwest::{StatusCode, header::HeaderMap};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{
    fmt,
    sync::LazyLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_GO_DURATION: Duration = Duration::from_nanos(i64::MAX as u64);
static RETRY_MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)try again in[\t\n\f\r ]*([0-9]+(?:\.[0-9]+)?)[\t\n\f\r ]*(ms|milliseconds?|s|seconds?)(?-u:\b)").unwrap()
});
static CONTEXT_LENGTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)maximum context length is ([0-9]+) tokens.*?(?:resulted in|requested about|requested) ([0-9]+) tokens").unwrap()
});

#[derive(Debug)]
pub enum RespondError {
    Cancelled,
    DeadlineExceeded,
    IdleTimeout,
    Transport(reqwest::Error),
    Protocol(crate::Error),
    CredentialsRejected {
        source: Box<RespondError>,
    },
    Api {
        error: Box<ApiError>,
        create_response: bool,
    },
    RateLimited {
        retry_after: Duration,
        source: Box<RespondError>,
    },
    ContextOverflow {
        tokens: i64,
        limit: i64,
        source: Box<RespondError>,
    },
}
impl RespondError {
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api { error, .. } => Some(error),
            Self::RateLimited { source, .. }
            | Self::ContextOverflow { source, .. }
            | Self::CredentialsRejected { source } => source.api_error(),
            _ => None,
        }
    }
    pub(crate) fn api(error: ApiError, create_response: bool) -> Self {
        Self::Api {
            error: Box::new(error),
            create_response,
        }
    }
    pub(crate) fn transport(error: reqwest::Error) -> Self {
        // URLs can contain signed query parameters or user-supplied credentials.
        Self::Transport(error.without_url())
    }
}
impl From<crate::Error> for RespondError {
    fn from(error: crate::Error) -> Self {
        Self::Protocol(error)
    }
}
impl fmt::Display for RespondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("context canceled"),
            Self::DeadlineExceeded => f.write_str("context deadline exceeded"),
            Self::IdleTimeout => f.write_str("remote response idle timeout"),
            Self::Transport(error) => error.fmt(f),
            Self::Protocol(error) => error.fmt(f),
            Self::CredentialsRejected { source } => write!(
                f,
                "codex credentials rejected; renew them externally and recreate the client: {source}"
            ),
            Self::Api {
                error,
                create_response,
            } => {
                if *create_response {
                    f.write_str("create response: ")?;
                }
                error.fmt(f)
            }
            Self::RateLimited { source, .. } | Self::ContextOverflow { source, .. } => {
                source.fmt(f)
            }
        }
    }
}
impl std::error::Error for RespondError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Api { error, .. } => Some(error),
            Self::RateLimited { source, .. }
            | Self::ContextOverflow { source, .. }
            | Self::CredentialsRejected { source } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(30),
        }
    }
}
impl RetryPolicy {
    pub(crate) fn validate(self) -> crate::Result<()> {
        if self.max_attempts == 0 {
            return Err(crate::Error("max attempts must be positive".into()));
        }
        if self.max_backoff < self.initial_backoff
            || self.max_backoff > MAX_GO_DURATION
            || (self.initial_backoff.is_zero() && !self.max_backoff.is_zero())
        {
            return Err(crate::Error("invalid response retry backoff".into()));
        }
        Ok(())
    }
    pub fn delay(
        self,
        attempt: usize,
        error: Option<&ApiError>,
        headers: &HeaderMap,
        now: SystemTime,
        jitter: f64,
    ) -> Duration {
        let mut hint = retry_after_header(headers, now);
        if let Some(error) = error.filter(|error| error.code == "rate_limit_exceeded") {
            hint = hint.max(retry_after_message(&error.message));
        }
        if !hint.is_zero() {
            return hint.min(self.max_backoff);
        }
        let (mut delay, cap) = if error.is_some_and(|error| {
            matches!(error.code.as_str(), "server_is_overloaded" | "slow_down")
        }) {
            (Duration::from_secs(10), Duration::from_secs(60))
        } else {
            (self.initial_backoff, self.max_backoff)
        };
        // Saturation must not loop through an attacker-controlled attempt count.
        for _ in 1..attempt.min(65) {
            delay = delay.saturating_mul(2).min(cap);
        }
        delay = delay.min(cap);
        let fraction = if jitter.is_finite() {
            jitter.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let subtract = Duration::from_nanos(((delay.as_nanos() / 5) as f64 * fraction) as u64);
        delay.saturating_sub(subtract)
    }
}

pub fn images_rejected(error: &ApiError) -> bool {
    let message = error.message.to_lowercase();
    message.contains("content.type is invalid")
        || message.contains("image")
            && [
                "not support",
                "unsupported",
                "only supported",
                "invalid",
                "not allowed",
                "not a valid",
            ]
            .iter()
            .any(|phrase| message.contains(phrase))
}
pub fn retryable_error(error: &ApiError) -> bool {
    if matches!(
        error.code.as_str(),
        "context_length_exceeded"
            | "insufficient_quota"
            | "usage_not_included"
            | "usage_limit_reached"
            | "credit_balance_exhausted"
            | "billing_hard_limit_reached"
            | "cyber_policy"
            | "misalignment_policy_violation"
            | "invalid_prompt"
            | "bio_policy"
            | "invalid_api_key"
            | "invalid_token"
    ) || matches!(
        error.kind.as_str(),
        "authentication_error" | "permission_error" | "insufficient_quota" | "usage_limit_reached"
    ) || images_rejected(error)
    {
        return false;
    }
    (200..300).contains(&error.status_code)
        || matches!(
            error.status_code,
            408 | 425 | 429 | 500 | 502 | 503 | 504 | 520..=524 | 529
        )
}

pub fn retry_after_message(message: &str) -> Duration {
    let Some(captures) = RETRY_MESSAGE.captures(message) else {
        return Duration::ZERO;
    };
    let Ok(value) = captures[1].parse::<f64>() else {
        return Duration::ZERO;
    };
    positive_nanos(
        value
            * if captures[2].to_ascii_lowercase().starts_with('m') {
                1_000_000.0
            } else {
                1_000_000_000.0
            },
    )
    .unwrap_or_default()
}
pub fn retry_after_header(headers: &HeaderMap, now: SystemTime) -> Duration {
    let Some(value) = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
    else {
        return Duration::ZERO;
    };
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds <= i64::MAX as u64 / 1_000_000_000)
            .map(Duration::from_secs)
            .unwrap_or_default();
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(now).ok())
        .unwrap_or_default()
}
fn positive_nanos(nanos: f64) -> Option<Duration> {
    if !nanos.is_finite() || nanos <= 0.0 || nanos > i64::MAX as f64 {
        return None;
    }
    // Go converts a float duration by truncation, not rounding to a nanosecond.
    Some(Duration::from_nanos(nanos as u64)).filter(|duration| *duration <= MAX_GO_DURATION)
}

pub fn reset_time(at: &Json, after: &Json, now: SystemTime) -> Option<SystemTime> {
    if let Ok(seconds) = at.decode::<f64>()
        && seconds.is_finite()
        && seconds > 0.0
        && seconds < i64::MAX as f64
    {
        return UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64));
    }
    if let Ok(text) = at.decode::<String>()
        && let Ok(date) = DateTime::parse_from_rfc3339(text.trim())
    {
        return Some(date.into());
    }
    after
        .decode::<f64>()
        .ok()
        .and_then(|seconds| positive_nanos(seconds * 1_000_000_000.0))
        .and_then(|duration| now.checked_add(duration))
}

pub(crate) fn provider_error(status: u16, body: &[u8], now: SystemTime) -> ApiError {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Envelope<'a> {
        #[serde(borrow, deserialize_with = "crate::optional_object")]
        error: Option<super::stream::WireError<'a>>,
    }
    if validate_json(body).is_ok()
        && let Ok(envelope) = crate::optional_object::<_, Envelope<'_>>(
            &mut serde_json::Deserializer::from_slice(body),
        )
        && let Some(wire) = envelope.and_then(|envelope| envelope.error)
        && (!wire.code.is_empty() || !wire.message.is_empty())
    {
        let at = raw_json(wire.resets_at);
        let after = raw_json(wire.resets_in_seconds);
        return ApiError {
            status_code: status,
            code: wire.code.into_owned(),
            message: wire.message.into_owned(),
            param: wire.param.into_owned(),
            kind: wire.kind.into_owned(),
            reset_at: reset_time(&at, &after, now),
            resets_at: at,
            resets_in_seconds: after,
        };
    }
    let message = String::from_utf8_lossy(body).trim().to_owned();
    ApiError {
        status_code: status,
        message: if message.is_empty() {
            StatusCode::from_u16(status)
                .ok()
                .and_then(|status| status.canonical_reason())
                .unwrap_or_default()
                .into()
        } else {
            message
        },
        ..ApiError::default()
    }
}
pub(crate) fn raw_json(value: Option<&RawValue>) -> Json {
    value
        .and_then(|value| Json::parse(value.get()).ok())
        .unwrap_or_default()
}

pub(crate) fn usage_limit(
    error: &ApiError,
    headers: &HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    if matches!(
        error.code.as_str(),
        "insufficient_quota"
            | "usage_not_included"
            | "credit_balance_exhausted"
            | "billing_hard_limit_reached"
    ) || error.kind == "insufficient_quota"
        || error.status_code != 429
            && error.code != "usage_limit_reached"
            && error.kind != "usage_limit_reached"
    {
        return None;
    }
    Some(
        retry_after_header(headers, now)
            .max(retry_after_message(&error.message))
            .max(
                error
                    .reset_at
                    .and_then(|at| at.duration_since(now).ok())
                    .unwrap_or_default(),
            )
            .min(MAX_GO_DURATION),
    )
}
pub(crate) fn context_overflow(error: RespondError) -> RespondError {
    let Some(api) = error.api_error() else {
        return error;
    };
    let captures = CONTEXT_LENGTH.captures(&api.message);
    if api.code != "context_length_exceeded" && captures.is_none() {
        return error;
    }
    let (limit, tokens) = captures.map_or((0, 0), |captures| {
        (
            captures[1].parse().unwrap_or(0),
            captures[2].parse().unwrap_or(0),
        )
    });
    RespondError::ContextOverflow {
        tokens,
        limit,
        source: Box::new(error),
    }
}
