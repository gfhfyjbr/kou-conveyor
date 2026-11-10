use super::*;
use crate::operation::shell::{ShellInput, ShellResult, ShellState, decode_shell, shell_spec};
use kou_domain::{Json, Timestamp};
use serde_json::value::RawValue;

fn number(text: &str) -> Option<f64> {
    let raw: &RawValue = serde_json::from_str(text).unwrap();
    number::number(raw).unwrap()
}
fn state(operation: &mut Operation, state: &ShellState) {
    operation.state = Json::parse(&serde_json::to_string(state).unwrap()).unwrap();
}
fn bash() -> Bash {
    Bash::new(BashConfig {
        shell: "/bin/sh".into(),
        base_directory: "/inert".into(),
        default_timeout: 600.0,
        ..BashConfig::default()
    })
}
#[test]
fn numeric_strings_match_go_syntax_and_single_ieee_rounding() {
    for (encoded, expected) in [
        ("null", None),
        ("\"1_000\"", Some(1000.0)),
        ("\"0X_1.fp+2\"", Some(7.75)),
        ("\"0x1p-1074\"", Some(f64::from_bits(1))),
        ("\"0x1p-1075\"", Some(0.0)),
        ("\"0x1.0000000000001p-1075\"", Some(f64::from_bits(1))),
        ("\"0x1.fffffffffffffp-1023\"", Some(f64::MIN_POSITIVE)),
        ("\"0x1.00000000000008p0\"", Some(1.0)),
        (
            "\"0x1.00000000000008001p0\"",
            Some(f64::from_bits(1.0f64.to_bits() + 1)),
        ),
        ("\"0x0p9999999999999999999999\"", Some(0.0)),
        ("\"+1.\"", Some(1.0)),
    ] {
        assert_eq!(
            number(encoded).map(f64::to_bits),
            expected.map(f64::to_bits),
            "{encoded}"
        );
    }
    assert_eq!(number("\"-0x0p0\"").unwrap().to_bits(), (-0.0f64).to_bits());
    for invalid in [
        r#"" \t ""#,
        "\"\"",
        "\"0x1\"",
        "\"0x1_p1\"",
        "\"0x1p_2\"",
        "\"0x_1p1__0\"",
        "\"1__2\"",
        "\"1e_2\"",
        "\"NaN\"",
        "1e999",
        "\"0x1p1024\"",
    ] {
        let raw: &RawValue = serde_json::from_str(invalid).unwrap();
        assert!(number::number(raw).is_err(), "{invalid}");
    }
}
#[test]
fn bash_translation_preserves_command_and_bounds_errors_before_submission() {
    let bash = bash();
    let mut specs = Vec::new();
    let status = bash.translate(
        "{\"command\":\"  true\\n\",\"timeout\":0.1250000009}",
        &mut |spec| {
            specs.push(spec);
            "owned".into()
        },
    );
    assert_eq!(status.waiting_for, Some(vec!["owned".into()]));
    let shell = decode_shell(&specs.pop().unwrap().operation("owned".into())).unwrap();
    assert_eq!(shell.input.command, "  true\n");
    assert_eq!(shell.input.timeout, 0.1250000009);
    let status = bash.translate("{\"command\":42,\"max_output_length\":3}", &mut |_| {
        panic!("invalid arguments submitted work")
    });
    assert!(status.error_truncated);
    assert!(status.error.starts_with('d') && status.error.ends_with("ng"));
    assert!(status.error.contains("bytes truncated"));
    for (value, expected) in [
        (
            "9007199254740992",
            "max_output_length must not exceed 1000000",
        ),
        (
            "9007199254740993",
            "max_output_length must not exceed 1000000",
        ),
        (
            "9007199254740994",
            "max_output_length must be a positive integer",
        ),
        ("1e18", "max_output_length must be a positive integer"),
    ] {
        let arguments = format!(r#"{{"command":"true","max_output_length":{value}}}"#);
        let status = bash.translate(&arguments, &mut |_| {
            panic!("invalid arguments submitted work")
        });
        assert_eq!(status.error, format!("bash argument: {expected}"));
    }
    for args in [
        "[]",
        "{}",
        "{\"command\":\"x\",\"timeout\":-1}",
        "{\"command\":\"x\",\"timeout\":14401}",
    ] {
        assert!(
            !bash
                .translate(args, &mut |_| panic!("invalid arguments submitted work"))
                .error
                .is_empty()
        );
    }
}
#[test]
fn bash_results_use_explicit_clocks_and_capture_paths_without_altering_output() {
    let bash = bash();
    let mut operation = shell_spec(
        ShellInput {
            command: "true".into(),
            shell: "/bin/sh".into(),
            timeout: 600.0,
            ..ShellInput::default()
        },
        "/inert".into(),
        40_000,
    )
    .unwrap()
    .operation("owned".into());
    operation.status = "awaiting".into();
    let mut shell = decode_shell(&operation).unwrap();
    shell.started_at = Timestamp::parse("2026-11-10T00:00:00Z").unwrap();
    shell.out_path = "/inert/out".into();
    shell.err_path = "/inert/err".into();
    shell.process_group_id = 12345;
    state(&mut operation, &shell);
    let now = Timestamp::parse("2026-11-10T00:01:30Z").unwrap();
    let operations = [Arc::new(operation.clone())];
    let detail = "Its output so far is in /inert/out (stdout) and /inert/err (stderr): read them with tail. To stop it: kill -TERM -- -12345";
    assert_eq!(bash.running(&operations), detail);
    let result = bash
        .result_at("call", &CallStatus::default(), &operations, now)
        .unwrap();
    assert_eq!(
        result.output[0].value,
        format!("Command is still running (1m30s so far). {detail}")
    );
    operation.status = "completed".into();
    shell.result = Some(ShellResult {
        out: "stdout\r\n".into(),
        err: "stderr\n\n".into(),
        timed_out: true,
        exit_code: 143,
        duration: 82.1,
        ..ShellResult::default()
    });
    state(&mut operation, &shell);
    let result = bash
        .result_at("call", &CallStatus::default(), &[Arc::new(operation)], now)
        .unwrap();
    assert_eq!(
        result.output[0].value,
        "stdout\r\n\nStderr:\nstderr\n\n\nExit code: 143\nThe command was stopped at its timeout of 10m0s.\nTook 1m22s."
    );
}
