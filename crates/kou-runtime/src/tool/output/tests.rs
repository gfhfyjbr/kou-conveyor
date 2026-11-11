use super::*;
use serde::Deserialize;
use std::collections::BTreeMap;

fn parse(encoded: &str) -> Result<usize> {
    let raw: &RawValue = serde_json::from_str(encoded).unwrap();
    parse_max_output_length(Some(raw))
}
fn bash() -> Bash {
    Bash::new(BashConfig {
        shell: "/bin/sh".into(),
        base_directory: "/operations".into(),
        ..BashConfig::default()
    })
}
fn no_submission(_: Spec) -> String {
    panic!("invalid arguments submitted work")
}
/// The error without the marker that says how much of it was cut.
fn retained(error: &str) -> String {
    let Some((head, rest)) = error.split_once("...") else {
        return error.into();
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    match rest[digits..].strip_prefix(" bytes truncated...") {
        Some(tail) if digits > 0 => format!("{head}{tail}"),
        _ => error.into(),
    }
}

#[test]
fn output_length_is_read_as_models_write_it() {
    for value in ["20000", "\"20000\"", "20000.0"] {
        assert_eq!(parse(value).unwrap(), 20000, "{value}");
    }
    assert_eq!(
        parse_max_output_length(None).unwrap(),
        DEFAULT_MAX_OUTPUT_LENGTH
    );
    for (value, error) in [
        ("0", "max_output_length must be a positive integer"),
        ("null", "max_output_length must be a positive integer"),
        ("1.5", "max_output_length must be a positive integer"),
        ("1000001", "max_output_length must not exceed 1000000"),
        (
            "true",
            "decode max_output_length: json: cannot unmarshal JSON boolean into Go toolarg.Number: want a number, such as 100, not true",
        ),
    ] {
        assert_eq!(parse(value).unwrap_err().to_string(), error, "{value}");
    }
}

#[test]
fn output_length_maximum_matches_tool_schemas() {
    let maximum = MAX_OUTPUT_LENGTH.to_string();
    assert_eq!(parse(&maximum).unwrap() as i64, MAX_OUTPUT_LENGTH);
    #[derive(Deserialize)]
    struct Schema {
        maximum: i64,
        default: usize,
    }
    let registry = Registry::new(StaticTranslators::default(), [BASH_NAME]);
    let mut checked = 0;
    for definition in registry.static_definitions() {
        if definition.tool.name != BASH_NAME {
            continue;
        }
        let parameters = definition.tool.parameters.as_ref().unwrap();
        let properties: BTreeMap<String, &RawValue> =
            serde_json::from_str(parameters["properties"].as_str().unwrap()).unwrap();
        let schema: Schema = serde_json::from_str(properties["max_output_length"].get()).unwrap();
        assert_eq!(schema.maximum, MAX_OUTPUT_LENGTH);
        assert_eq!(schema.default, DEFAULT_MAX_OUTPUT_LENGTH);
        checked += 1;
    }
    assert_eq!(checked, 1);
}

#[test]
fn validation_errors_are_bounded_before_result_translation() {
    let bash = bash();
    let long_name = "é".repeat(DEFAULT_MAX_OUTPUT_LENGTH + 1);
    let quoted = serde_json::to_string(&long_name).unwrap();
    let malformed = format!("{{{quoted}:0,{quoted}:0}}");
    let invalid_limit = format!("{{\"max_output_length\":{quoted}}}");
    for (name, arguments, limit, truncated) in [
        (
            "Bash requested",
            r#"{"command":null,"max_output_length":10}"#,
            10,
            true,
        ),
        (
            "Bash untruncated",
            r#"{"command":null}"#,
            DEFAULT_MAX_OUTPUT_LENGTH,
            false,
        ),
        (
            "Bash malformed",
            malformed.as_str(),
            DEFAULT_MAX_OUTPUT_LENGTH,
            true,
        ),
        (
            "Bash invalid limit",
            invalid_limit.as_str(),
            DEFAULT_MAX_OUTPUT_LENGTH,
            false,
        ),
    ] {
        let status = bash.translate(arguments, &mut no_submission);
        assert!(
            status.waiting().is_empty() && !status.error.is_empty(),
            "{name}"
        );
        assert!(retained(&status.error).chars().count() <= limit, "{name}");
        assert_eq!(status.error_truncated, truncated, "{name}");
        let restored: CallStatus =
            serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
        assert_eq!(restored, status, "{name}");
        let result = bash.result("invalid-call", &restored, &[]).unwrap();
        let output = result.output[0].value.strip_prefix("Error: ");
        assert_eq!(output, Some(status.error.as_str()), "{name}");
    }
}

#[test]
fn tools_reject_invalid_output_lengths_before_submission() {
    let bash = bash();
    let over = (MAX_OUTPUT_LENGTH + 1).to_string();
    for value in [
        "0",
        "-1",
        over.as_str(),
        "1000000000",
        "1.5",
        "\"1.5\"",
        "\"ten\"",
        "true",
        "null",
        "[]",
        "{}",
        "999999999999999999999999",
    ] {
        let arguments = format!(r#"{{"command":"pwd","max_output_length":{value}}}"#);
        let status = bash.translate(&arguments, &mut no_submission);
        assert!(status.error.contains("max_output_length"), "{value}");
        assert!(status.waiting().is_empty(), "{value}");
        let result = bash.result("invalid-call", &status, &[]).unwrap();
        assert!(
            result.output[0].value.contains("max_output_length"),
            "{value}"
        );
    }
}

#[test]
fn error_status_bounds_characters_and_zero_is_the_default_limit() {
    let short = error_status("tool \"Grep\" is not available".into(), 0);
    assert_eq!(short.error, "tool \"Grep\" is not available");
    assert!(!short.error_truncated && short.waiting_for.is_none());
    let exact = error_status("é".repeat(DEFAULT_MAX_OUTPUT_LENGTH), 0);
    assert!(!exact.error_truncated);
    let long = error_status("é".repeat(DEFAULT_MAX_OUTPUT_LENGTH + 1), 0);
    assert!(long.error_truncated);
    assert_eq!(
        retained(&long.error).chars().count(),
        DEFAULT_MAX_OUTPUT_LENGTH
    );
    assert!(long.error.contains("...2 bytes truncated..."));
    let cut = error_status("ab界yz".into(), 3);
    assert_eq!(cut.error, "a...4 bytes truncated...yz");
    assert!(cut.error_truncated);
}

// Go quotes the value back cut at 40 bytes and then makes the text valid: each
// byte of a character cut in two reads as U+FFFD.
#[test]
fn a_rejected_value_cut_inside_a_character_reads_as_go_reports_it() {
    for (digits, scalar, kept) in [
        (38, "界", "\u{fffd}"),
        (37, "界", "\u{fffd}\u{fffd}"),
        (36, "🙂", "\u{fffd}\u{fffd}\u{fffd}"),
        (37, "é", "é"),
        (39, "é", ""),
    ] {
        let digits = "9".repeat(digits);
        let value = format!("\"{digits}{scalar}x\"");
        assert_eq!(
            parse(&value).unwrap_err().to_string(),
            format!(
                "decode max_output_length: json: cannot unmarshal JSON string into Go toolarg.Number: want a number, such as 100, not \"{digits}{kept}…"
            ),
            "{value}"
        );
    }
}
