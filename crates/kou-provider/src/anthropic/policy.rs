use crate::{Error, null_default, object_or_null};
use regex::Regex;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::{
    fmt,
    sync::LazyLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default)]
pub struct ApiError {
    pub status_code: u16,
    pub kind: String,
    pub message: String,
    pub request_id: String,
    pub raw: String,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.kind.is_empty() {
            status_text(self.status_code)
        } else {
            &self.kind
        };
        write!(
            f,
            "Messages API error {} {kind}: {}",
            self.status_code, self.message
        )?;
        if !self.request_id.is_empty() {
            write!(f, " (request {})", self.request_id)?;
        }
        Ok(())
    }
}
impl std::error::Error for ApiError {}
fn status_text(status: u16) -> &'static str {
    match status {
        413 => return "Request Entity Too Large",
        414 => return "Request URI Too Long",
        _ => {}
    }
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("")
}
#[derive(Debug)]
pub enum RespondError {
    Cancelled,
    DeadlineExceeded,
    IdleTimeout,
    ModelsUnavailable,
    Build(Error),
    Request(Error),
    Transport(reqwest::Error),
    Api(ApiError),
    RateLimited {
        retry_after: Duration,
        source: Box<Self>,
    },
    ContextOverflow {
        tokens: i64,
        limit: i64,
        source: Box<Self>,
    },
}
impl fmt::Display for RespondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("context canceled"),
            Self::DeadlineExceeded => f.write_str("context deadline exceeded"),
            Self::IdleTimeout => {
                f.write_str("Messages API request failed: Messages API stream went idle")
            }
            Self::ModelsUnavailable => f.write_str("the endpoint does not list models"),
            Self::Build(error) => error.fmt(f),
            Self::Request(error) => write!(f, "Messages API request failed: {error}"),
            Self::Transport(error) => write!(f, "Messages API request failed: {error}"),
            Self::Api(error) => error.fmt(f),
            Self::RateLimited { source, .. } | Self::ContextOverflow { source, .. } => {
                source.fmt(f)
            }
        }
    }
}
impl std::error::Error for RespondError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(error) | Self::Request(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Api(error) => Some(error),
            Self::RateLimited { source, .. } | Self::ContextOverflow { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl RespondError {
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            Self::RateLimited { source, .. } | Self::ContextOverflow { source, .. } => {
                source.api_error()
            }
            _ => None,
        }
    }
    pub(super) fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url())
    }
    pub(super) fn retryable(&self) -> bool {
        match self {
            Self::Api(error) => {
                matches!(error.status_code, 408 | 409 | 429)
                    || error.status_code >= 500
                    || matches!(
                        error.kind.as_str(),
                        "api_error" | "rate_limit_error" | "timeout_error" | "overloaded_error"
                    )
            }
            Self::Transport(error) => error.is_timeout() || error.is_connect() || error.is_body(),
            Self::IdleTimeout => true,
            Self::Request(error) => error.0 == super::stream::UNEXPECTED_EOF,
            _ => false,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Fields {
    #[serde(deserialize_with = "null_default")]
    message: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Envelope {
    #[serde(deserialize_with = "object_or_null")]
    error: Fields,
    #[serde(deserialize_with = "null_default")]
    message: String,
}
pub(super) fn api_error(status: u16, headers: &HeaderMap, raw: &[u8]) -> ApiError {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Kind {
        #[serde(rename = "type", deserialize_with = "null_default")]
        value: String,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct KindEnvelope {
        #[serde(deserialize_with = "object_or_null")]
        error: Kind,
    }
    let kind: KindEnvelope =
        object_or_null(&mut serde_json::Deserializer::from_slice(raw)).unwrap_or_default();
    let fields: Envelope =
        object_or_null(&mut serde_json::Deserializer::from_slice(raw)).unwrap_or_default();
    let raw = String::from_utf8_lossy(raw).into_owned();
    let message = if !fields.error.message.is_empty() {
        fields.error.message
    } else if !fields.message.is_empty() {
        fields.message
    } else if !raw.trim().is_empty() {
        raw.trim().into()
    } else {
        status_text(status).into()
    };
    ApiError {
        status_code: status,
        kind: kind.error.value,
        message,
        request_id: header(headers, "request-id").into(),
        raw,
    }
}
pub(super) fn header<'a>(headers: &'a HeaderMap, key: &str) -> &'a str {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}
fn duration(value: f64, scale: f64) -> Duration {
    let nanos = value * scale;
    if !nanos.is_finite() || nanos >= i64::MAX as f64 {
        return Duration::ZERO;
    }
    Duration::from_nanos(nanos.max(0.0) as u64)
}
pub(super) fn header_delay(headers: &HeaderMap, now: SystemTime) -> Duration {
    if let Ok(value) = header(headers, "retry-after-ms").parse::<f64>()
        && value > 0.0
    {
        return duration(value, 1_000_000.0);
    }
    let value = header(headers, "retry-after").trim();
    if let Ok(value) = value.parse::<f64>()
        && value > 0.0
    {
        return duration(value, 1_000_000_000.0);
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|time| time.duration_since(now).ok())
        .unwrap_or_default()
}
pub(super) fn rate_wait(
    error: &ApiError,
    headers: &HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    if error.status_code != 429 && error.kind != "rate_limit_error" {
        return None;
    }
    let mut reset = Duration::ZERO;
    if header(headers, "anthropic-ratelimit-unified-status")
        .trim()
        .eq_ignore_ascii_case("rejected")
    {
        let value = header(headers, "anthropic-ratelimit-unified-reset").trim();
        let deadline = value
            .parse::<i64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64)))
            .or_else(|| {
                chrono::DateTime::parse_from_rfc3339(value)
                    .ok()
                    .map(SystemTime::from)
            });
        reset = deadline
            .and_then(|time| time.duration_since(now).ok())
            .unwrap_or_default();
    }
    Some(header_delay(headers, now).max(reset))
}
pub(super) fn retry_delay(
    attempt: usize,
    error: &RespondError,
    headers: &HeaderMap,
    now: SystemTime,
    jitter: f64,
) -> Duration {
    let overloaded = error
        .api_error()
        .is_some_and(|error| error.kind == "overloaded_error" || error.status_code == 529);
    if !overloaded {
        let hint = header_delay(headers, now);
        if !hint.is_zero() {
            return hint.min(Duration::from_secs(60));
        }
    }
    let seconds = (2u64 << attempt.saturating_sub(1).min(4)).min(30);
    let mut base = Duration::from_secs(seconds);
    if overloaded {
        base = base.max(Duration::from_secs(10u64.saturating_mul(attempt as u64)));
    }
    base.saturating_sub(Duration::from_nanos(
        ((base.as_nanos() / 5) as f64 * jitter.clamp(0.0, 1.0)) as u64,
    ))
}
static CONTEXT_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([0-9]+)[\t\n\f\r ]*\+[\t\n\f\r ]*([0-9]+)[\t\n\f\r ]*>[\t\n\f\r ]*([0-9]+)")
        .unwrap()
});
static PROMPT_TOO_LONG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:prompt|input) is too long(?:[^0-9]*?([0-9]+) tokens? > ([0-9]+))?").unwrap()
});
fn number(captures: &regex::Captures<'_>, index: usize) -> Option<i64> {
    captures.get(index)?.as_str().parse::<i64>().ok()
}
pub(super) fn response_room(message: &str, requested: i64) -> Option<i64> {
    if !message.contains("max_tokens") {
        return None;
    }
    let captures = CONTEXT_LIMIT.captures(message)?;
    let input = number(&captures, 1)?;
    let window = number(&captures, 3)?;
    let room = window.checked_sub(input)?;
    (room >= 4096 && room < requested).then_some(room)
}
pub(super) fn overflow(error: &ApiError) -> Option<(i64, i64)> {
    if error.status_code == 413 {
        return Some((0, 0));
    }
    if error.status_code != 400 {
        return None;
    }
    if let Some(captures) = PROMPT_TOO_LONG.captures(&error.message) {
        return Some((
            number(&captures, 1).unwrap_or(0),
            number(&captures, 2).unwrap_or(0),
        ));
    }
    if error.message.contains("max_tokens")
        && let Some(captures) = CONTEXT_LIMIT.captures(&error.message)
        && let (Some(input), Some(window)) = (number(&captures, 1), number(&captures, 3))
        && window.wrapping_sub(input) < 4096
    {
        return Some((input.wrapping_add(4096), window));
    }
    None
}
pub(super) fn thinking_rejected(error: &ApiError) -> bool {
    let message = error.message.to_lowercase();
    error.status_code == 400 && message.contains("thinking") && message.contains("signature")
}
pub(super) fn context_rejected(error: &ApiError) -> bool {
    let message = error.message.to_lowercase();
    error.status_code == 400
        && ["context_management", "context-management", "clear_thinking"]
            .iter()
            .any(|part| message.contains(part))
}
pub(super) fn fallbacks_rejected(error: &ApiError) -> bool {
    error.status_code == 400 && error.message.to_lowercase().contains("fallback")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overload_backoff_is_linear_and_ignores_retry_after() {
        let headers = HeaderMap::from_iter([(
            "retry-after"
                .parse::<reqwest::header::HeaderName>()
                .unwrap(),
            "0.001".parse().unwrap(),
        )]);
        for status in [400, 529] {
            let error = RespondError::Api(ApiError {
                status_code: status,
                kind: if status == 400 {
                    "overloaded_error"
                } else {
                    "api_error"
                }
                .into(),
                ..Default::default()
            });
            assert_eq!(
                retry_delay(10, &error, &headers, UNIX_EPOCH, 0.0),
                Duration::from_secs(100)
            );
            assert_eq!(
                retry_delay(10, &error, &headers, UNIX_EPOCH, 1.0),
                Duration::from_secs(80)
            );
        }
        let normal = RespondError::Api(ApiError {
            status_code: 503,
            ..Default::default()
        });
        assert_eq!(
            retry_delay(10, &normal, &headers, UNIX_EPOCH, 0.5),
            Duration::from_millis(1)
        );
        assert_eq!(
            retry_delay(10, &normal, &HeaderMap::new(), UNIX_EPOCH, 1.0),
            Duration::from_secs(24)
        );
    }
    #[test]
    fn retry_hint_truncation_and_unified_reset_maximum() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "0.0012349".parse().unwrap());
        assert_eq!(
            header_delay(&headers, UNIX_EPOCH),
            Duration::from_nanos(1234)
        );
        headers.insert("retry-after-ms", "0.0000000001".parse().unwrap());
        headers.insert("retry-after", "3600".parse().unwrap());
        assert_eq!(header_delay(&headers, UNIX_EPOCH), Duration::ZERO);
        headers.remove("retry-after-ms");
        headers.insert("retry-after", "2".parse().unwrap());
        headers.insert(
            "anthropic-ratelimit-unified-status",
            " ReJeCtEd ".parse().unwrap(),
        );
        headers.insert("anthropic-ratelimit-unified-reset", "1100".parse().unwrap());
        let error = ApiError {
            status_code: 429,
            ..Default::default()
        };
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        assert_eq!(
            rate_wait(&error, &headers, now),
            Some(Duration::from_secs(100))
        );
        headers.insert(
            "anthropic-ratelimit-unified-reset",
            "1970-01-01T00:18:20Z".parse().unwrap(),
        );
        assert_eq!(
            rate_wait(&error, &headers, now),
            Some(Duration::from_secs(100))
        );
    }
    #[test]
    fn context_room_and_overflow_match_the_go_threshold() {
        let text = "input and max_tokens exceed context: 195904 + 64000 > 200000";
        assert_eq!(response_room(text, 64000), Some(4096));
        let error = ApiError {
            status_code: 400,
            message: text.into(),
            ..Default::default()
        };
        assert_eq!(overflow(&error), None);
        let error = ApiError {
            message: text.replace("195904", "195905"),
            ..error
        };
        assert_eq!(overflow(&error), Some((200001, 200000)));
        assert_eq!(
            response_room("max_tokens: 195904\u{a0}+ 64000 > 200000", 64000),
            None
        );
        let error = ApiError {
            status_code: 413,
            ..Default::default()
        };
        assert_eq!(overflow(&error), Some((0, 0)));
    }
    #[test]
    fn api_type_and_error_message_have_independent_decoding() {
        let error = api_error(
            400,
            &HeaderMap::new(),
            br#"{"error":{"type":"invalid_request_error","message":[]}}"#,
        );
        assert_eq!(error.kind, "invalid_request_error");
        assert!(
            error
                .to_string()
                .contains(r#"{"error":{"type":"invalid_request_error","message":[]}}"#)
        );
    }
}
