use kou_domain::Json;
use kou_provider::responses::{
    ApiError, ResponseState, RetryPolicy, StreamLimits,
    policy::{
        images_rejected, reset_time, retry_after_header, retry_after_message, retryable_error,
    },
};
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn retry_classification_matches_go_final_codes_types_and_statuses() {
    for code in [
        "context_length_exceeded",
        "insufficient_quota",
        "usage_not_included",
        "usage_limit_reached",
        "credit_balance_exhausted",
        "billing_hard_limit_reached",
        "cyber_policy",
        "misalignment_policy_violation",
        "invalid_prompt",
        "bio_policy",
        "invalid_api_key",
        "invalid_token",
    ] {
        for status_code in [200, 429, 503] {
            assert!(
                !retryable_error(&ApiError {
                    status_code,
                    code: code.into(),
                    ..ApiError::default()
                }),
                "{status_code} {code}"
            );
        }
    }
    for kind in [
        "authentication_error",
        "permission_error",
        "insufficient_quota",
        "usage_limit_reached",
    ] {
        assert!(!retryable_error(&ApiError {
            status_code: 503,
            kind: kind.into(),
            ..ApiError::default()
        }));
    }
    for status_code in [
        200, 299, 408, 425, 429, 500, 502, 503, 504, 520, 521, 522, 523, 524, 529,
    ] {
        assert!(retryable_error(&ApiError {
            status_code,
            code: "unknown".into(),
            kind: "invalid_request_error".into(),
            ..ApiError::default()
        }));
    }
    for status_code in [0, 199, 300, 307, 400, 401, 403, 404, 501, 505, 525, 530] {
        assert!(!retryable_error(&ApiError {
            status_code,
            ..ApiError::default()
        }));
    }
}
#[test]
fn retry_hints_backoff_overload_and_jitter_match_go() {
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for (message, nanos) in [
        (
            "Please try again in 11.054s. Visit the docs.",
            11_054_000_000,
        ),
        ("try again in 28ms.", 28_000_000),
        ("try again in 1.5 milliseconds.", 1_500_000),
        ("TRY AGAIN IN 35 SECONDS.", 35_000_000_000),
        ("try again in 1 second", 1_000_000_000),
    ] {
        assert_eq!(
            retry_after_message(message),
            Duration::from_nanos(nanos),
            "{message}"
        );
    }
    for message in [
        "try again in 0s",
        "try again in -1s",
        "try again in NaNs",
        "try again in 999999999999999999999s",
        "try again in 1 minute",
        "try again in 12snowflakes",
        "try again in 2",
        "quota exhausted",
    ] {
        assert_eq!(retry_after_message(message), Duration::ZERO, "{message}");
    }
    for (value, seconds) in [
        (" 120 ", 120),
        ("0", 0),
        ("-1", 0),
        ("1.5", 0),
        ("9223372036", 9223372036),
        ("9223372037", 0),
        ("18446744073709551615", 0),
        ("999999999999999999999", 0),
        ("garbage", 0),
        ("", 0),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_str(value).unwrap());
        assert_eq!(
            retry_after_header(&headers, now),
            Duration::from_secs(seconds),
            "{value}"
        );
    }
    for seconds in [60, 0] {
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            HeaderValue::from_str(&httpdate::fmt_http_date(now + Duration::from_secs(seconds)))
                .unwrap(),
        );
        assert_eq!(
            retry_after_header(&headers, now),
            Duration::from_secs(seconds)
        );
    }
    let policy = RetryPolicy::default();
    for (attempt, code, message, header, millis) in [
        (1, "", "", "", 2000),
        (3, "", "", "", 8000),
        (usize::MAX, "", "", "", 30000),
        (1, "server_is_overloaded", "", "", 10000),
        (2, "slow_down", "", "", 20000),
        (usize::MAX, "slow_down", "", "", 60000),
        (1, "rate_limit_exceeded", "try again in 3600s", "", 30000),
        (1, "", "", "86400", 30000),
        (1, "rate_limit_exceeded", "try again in 28ms", "", 28),
        (1, "rate_limit_exceeded", "try again in 20s", "10", 20000),
        (1, "rate_limit_exceeded", "try again in 10s", "20", 20000),
        (1, "rate_limit_exceeded", "try again in 5s", "invalid", 5000),
        (1, "server_error", "try again in 120s", "", 2000),
        (1, "server_is_overloaded", "", "90", 30000),
    ] {
        let error = ApiError {
            code: code.into(),
            message: message.into(),
            ..ApiError::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_str(header).unwrap());
        let base = Duration::from_millis(millis);
        assert_eq!(
            policy.delay(attempt, Some(&error), &headers, now, 0.0),
            base,
            "{code} {message} {header}"
        );
        for jitter in [0.25, 0.5, 0.99, 1.0] {
            let delay = policy.delay(attempt, Some(&error), &headers, now, jitter);
            assert!((base * 4 / 5..=base).contains(&delay));
            if code == "rate_limit_exceeded" || !header.is_empty() {
                assert_eq!(delay, base);
            }
        }
    }
}
#[test]
fn reset_times_are_anchored_once_and_invalid_top_level_does_not_hide_nested_reset() {
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for (at, after, expected) in [
        ("1800000012.9", "999", Some(now + Duration::from_secs(12))),
        (
            r#""2027-01-15T08:00:12Z""#,
            "999",
            Some(now + Duration::from_secs(12)),
        ),
        ("null", "1.25", Some(now + Duration::from_millis(1250))),
        ("-1", "2", Some(now + Duration::from_secs(2))),
        ("0", "null", None),
        (r#""bad""#, "-2", None),
        ("1e999", "1e999", None),
    ] {
        assert_eq!(
            reset_time(&Json::parse(at).unwrap(), &Json::parse(after).unwrap(), now),
            expected,
            "{at} {after}"
        );
    }
    for top in ["null", "0", r#""bad""#] {
        let mut state = ResponseState::new(StreamLimits::default()).unwrap();
        state.observe(format!(r#"{{"type":"error","resets_at":{top},"error":{{"code":"usage_limit_reached","resets_at":1800000012}}}}"#).as_bytes(), |_| {}).unwrap();
        assert_eq!(
            state.api_error().unwrap().reset_at,
            Some(now + Duration::from_secs(12))
        );
    }
}
#[test]
fn image_rejections_are_final_for_unchanged_request() {
    for message in [
        "messages.content.type is invalid, allowed values: ['text']",
        "Images are not supported",
        "unsupported image",
        "image only supported for another model",
        "invalid image",
        "image is not allowed",
        "not a valid image",
    ] {
        let error = ApiError {
            status_code: 503,
            message: message.into(),
            ..ApiError::default()
        };
        assert!(images_rejected(&error));
        assert!(!retryable_error(&error));
    }
    for message in [
        "invalid parameter",
        "image generation failed temporarily",
        "the vision endpoint is unavailable",
    ] {
        assert!(!images_rejected(&ApiError {
            message: message.into(),
            ..ApiError::default()
        }));
    }
}
