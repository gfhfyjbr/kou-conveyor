use super::*;
use crate::operation::shell::{ShellResult, decode_shell};

fn config() -> CommandConfig {
    CommandConfig {
        name: "Echo".into(),
        command: vec!["/bin/echo".into(), "it's".into(), String::new()],
        directory: "/inert/work//".into(),
        base_directory: "/inert/captures".into(),
        ..CommandConfig::default()
    }
}
fn operation(command: &dyn Translator, call_id: &str, arguments: &str) -> Operation {
    let mut specs = Vec::new();
    let status = command.translate_call(
        &ToolCall {
            call_id: call_id.into(),
            name: "aliased_Echo".into(),
            arguments: arguments.into(),
        },
        &mut |spec| {
            specs.push(spec);
            "owned".into()
        },
    );
    assert!(status.error.is_empty(), "{}", status.error);
    assert_eq!(status.waiting_for, Some(vec!["owned".into()]));
    assert_eq!(specs.len(), 1);
    specs.pop().unwrap().operation("owned".into())
}

#[test]
fn commands_validate_argv_and_posix_environment_names_before_translation() {
    for argv in [vec![], vec![String::new()], vec![String::new(), "x".into()]] {
        assert_eq!(
            Command::new(CommandConfig {
                command: argv,
                ..config()
            })
            .err()
            .unwrap()
            .to_string(),
            "tool \"Echo\" has no command"
        );
    }
    for name in ["", "9X", "A B", "Ü", "A=", "X\n"] {
        assert_eq!(
            Command::new(CommandConfig {
                environment: BTreeMap::from([(name.into(), "value".into())]),
                ..config()
            })
            .err()
            .unwrap()
            .to_string(),
            format!("tool \"Echo\": {name:?} is not a variable name")
        );
    }
    let command = Command::new(CommandConfig {
        environment: BTreeMap::from([
            ("_".into(), "one".into()),
            ("A_Z9".into(), "two".into()),
            ("x0".into(), "three".into()),
        ]),
        ..config()
    })
    .unwrap();
    let current = operation(&command, "call", "{}");
    assert_eq!(current.max_output_length, 40_000);
    assert!(!command.waits());
}

#[test]
fn command_specs_keep_call_identity_shell_quoting_and_raw_json_tokens() {
    let command = Command::new(CommandConfig {
        waits: true,
        environment: BTreeMap::from([
            ("ZZ".into(), "it's $HOME `quoted`\nline".into()),
            ("AAA".into(), "one".into()),
            (CALL_ID.into(), "injected ID".into()),
            (TOOL_NAME.into(), "injected name".into()),
        ]),
        ..config()
    })
    .unwrap();
    let translator: &dyn Translator = &command;
    let current = operation(
        translator,
        "call'7",
        r#" { "z":1e999, "text":"line\nKOU_CONVEYOR_TOOL_ARGUMENTS\n'quote'", "unicode":"\u00e9", "n":1.00 } "#,
    );
    let state = decode_shell(&current).unwrap();
    assert_eq!(state.input.shell, "/bin/sh");
    assert_eq!(state.input.directory, "/inert/work//");
    assert_eq!(state.base_directory, "/inert/captures");
    assert_eq!(state.input.timeout, 0.0);
    assert!(translator.waits());
    assert_eq!(translator.running(&[Arc::new(current)]), "");
    assert_eq!(
        state.input.command,
        concat!(
            "export AAA='one' KOU_CONVEYOR_TOOL_CALL_ID='call'\\''7' KOU_CONVEYOR_TOOL_NAME='Echo' ZZ='it'\\''s $HOME `quoted`\nline'\n",
            "exec '/bin/echo' 'it'\\''s' '' <<'KOU_CONVEYOR_TOOL_ARGUMENTS'\n",
            r#"{"z":1e999,"text":"line\nKOU_CONVEYOR_TOOL_ARGUMENTS\n'quote'","unicode":"\u00e9","n":1.00}"#,
            "\nKOU_CONVEYOR_TOOL_ARGUMENTS\n"
        )
    );
}

#[test]
fn command_validation_uses_configured_output_limits_and_submits_no_work() {
    for limit in [0, -1, 1, 3, 30] {
        let command = Command::new(CommandConfig {
            max_output_length: limit,
            ..config()
        })
        .unwrap();
        for raw in [
            "[]",
            "null",
            "{",
            r#"{"x":1,"x":2}"#,
            r#"{"x":{"y":1,"y":2}}"#,
        ] {
            let status =
                command.translate(raw, &mut |_| panic!("invalid arguments submitted work"));
            assert!(!status.error.is_empty());
            assert_eq!(status.error_truncated, limit > 0 && limit < 36);
            assert_eq!(
                command.result("call", &status, &[]).unwrap().output[0].value,
                format!("Error: {}", status.error)
            );
        }
    }
    let command = Command::new(config()).unwrap();
    for raw in ["", " \n\t", "{}", r#"{"max_output_length":false}"#] {
        let current = operation(&command, "call", raw);
        assert_eq!(current.max_output_length, 40_000);
    }
}

#[test]
fn command_results_use_plugin_validation_context_and_shared_bash_formatting() {
    let command = Command::new(config()).unwrap();
    let mut current = operation(&command, "call", "{}");
    assert_eq!(
        command
            .result("call", &CallStatus::default(), &[])
            .unwrap_err()
            .to_string(),
        "Echo call \"call\" has 0 operations, want 1"
    );
    let status = CallStatus {
        error: "validation".into(),
        ..CallStatus::default()
    };
    assert_eq!(
        command
            .result("call", &status, &[Arc::new(current.clone())])
            .unwrap_err()
            .to_string(),
        "Echo call \"call\" has both a validation error and operations"
    );
    let mut state = decode_shell(&current).unwrap();
    state.result = Some(ShellResult {
        out: "exact\r\n".into(),
        err: "stderr\n".into(),
        exit_code: 7,
        ..ShellResult::default()
    });
    current.state = kou_domain::Json::parse(&serde_json::to_string(&state).unwrap()).unwrap();
    current.status = "completed".into();
    let result = command
        .result("call", &CallStatus::default(), &[Arc::new(current)])
        .unwrap();
    assert_eq!(
        result.output[0].value,
        "exact\r\n\nStderr:\nstderr\n\nExit code: 7"
    );
}
