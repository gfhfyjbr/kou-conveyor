use kou_domain::{Json, Operation};
use kou_runtime::operation::*;

fn file(action: &str, path: &str) -> Operation {
    file_spec(FileState {
        action: action.into(),
        path: path.into(),
        patch: "*** Begin Patch\n*** End Patch".into(),
        ..FileState::default()
    })
    .unwrap()
    .operation("file".into())
}
#[test]
fn values_keep_opaque_json_and_have_no_effects() {
    for raw in ["null", "1e999", r#"{"items":[1,true,"é"]}"#] {
        let current = value_spec(Json::parse(raw).unwrap())
            .unwrap()
            .operation("value".into());
        assert_eq!(decode_value(&current).unwrap().as_str(), Some(raw));
        let step = advance_value(current, None).unwrap();
        assert_eq!(step.operation.status, "completed");
        assert!(step.dispatches.is_empty());
    }
    assert_eq!(
        value_spec(Json::default()).unwrap_err().to_string(),
        "value operation result must be valid JSON"
    );
    let mut current = value_spec(Json::parse("null").unwrap())
        .unwrap()
        .operation("value".into());
    current.state = Json::parse("[]").unwrap();
    assert!(
        decode_value(&current)
            .unwrap_err()
            .to_string()
            .contains("cannot unmarshal JSON array")
    );
}
#[test]
fn interrupted_mutating_operations_are_not_automatically_repeated() {
    for action in ["edit", "patch", "read", "write", "transcript_search"] {
        let mut current = if action == "transcript_search" {
            file_spec(FileState {
                action: action.into(),
                path: "/inert-fixture".into(),
                query: "query".into(),
                ..FileState::default()
            })
            .unwrap()
            .operation("file".into())
        } else {
            file(action, "/inert-fixture")
        };
        current.status = "awaiting".into();
        let step = advance_file(current, None).unwrap();
        if matches!(action, "edit" | "patch") {
            assert!(step.dispatches.is_empty());
            assert_eq!(step.operation.status, "failed");
            assert!(
                decode_file(&step.operation)
                    .unwrap()
                    .terminal_error
                    .contains("interrupted by a restart")
            );
        } else {
            assert_eq!(step.dispatches.len(), 1);
            assert_eq!(step.operation.status, "awaiting");
            assert_eq!(step.dispatches[0].source, "file");
            assert_eq!(step.dispatches[0].correlation_id, "file");
        }
    }
}
#[test]
fn cancellation_and_wrong_source_discard_the_result() {
    let mut current = file("read", "/inert-fixture");
    current.status = "awaiting".into();
    for (source, kind, status, error) in [
        (
            "file",
            "primitive.canceled",
            "canceled",
            "file operation canceled",
        ),
        (
            "other",
            "compute.completed",
            "failed",
            "file primitive event source is \"other\", want \"file\"",
        ),
        (
            "file",
            "primitive.failed",
            "failed",
            "file operation failed with an invalid result",
        ),
    ] {
        let event = Event {
            source: source.into(),
            kind: kind.into(),
            result: EventResult::File(FileResult {
                text: "must be discarded".into(),
                ..FileResult::default()
            }),
        };
        let step = advance_file(current.clone(), Some(event)).unwrap();
        let state = decode_file(&step.operation).unwrap();
        assert_eq!(step.operation.status, status);
        assert!(state.result.is_none());
        assert_eq!(state.terminal_error, error);
    }
}
#[test]
fn file_effects_require_explicit_dispatch_and_complete_through_a_transition() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("owned.txt")
        .to_str()
        .unwrap()
        .to_owned();
    let states = [
        FileState {
            action: "write".into(),
            content: "one\ntwo\n".into(),
            path: path.clone(),
            ..FileState::default()
        },
        FileState {
            action: "edit".into(),
            old_string: "two".into(),
            new_string: "2".into(),
            path: path.clone(),
            ..FileState::default()
        },
        FileState {
            action: "read".into(),
            path: path.clone(),
            ..FileState::default()
        },
    ];
    for (index, state) in states.into_iter().enumerate() {
        let current = file_spec(state).unwrap().operation(format!("file-{index}"));
        let step = advance_file(current, None).unwrap();
        if index == 0 {
            assert!(!std::path::Path::new(&path).exists());
        }
        let dispatch = &step.dispatches[0];
        let result = run_file(&dispatch.file, || false).unwrap();
        if index == 1 {
            assert!(
                result
                    .text
                    .contains(". The file now reads around the first change:\n")
            );
        }
        if index == 2 {
            assert_eq!(result.text, "     1\tone\n     2\t2");
        }
        let event = Event {
            source: dispatch.source.clone(),
            kind: "compute.completed".into(),
            result: EventResult::File(result),
        };
        let done = advance_file(step.operation, Some(event)).unwrap();
        assert_eq!(done.operation.status, "completed");
        assert!(done.dispatches.is_empty());
        assert!(decode_file(&done.operation).unwrap().result.is_some());
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), "one\n2\n");
}
#[test]
fn nested_file_results_reject_positional_arrays_and_scalar_shapes() {
    for (raw, shape) in [
        ("[]", "array"),
        (r#"["positional",[],1,2,3,false]"#, "array"),
        (r#""text""#, "string"),
        ("42", "number"),
        ("true", "boolean"),
    ] {
        let mut current = file("read", "/inert-fixture");
        current.state = Json::parse(&format!(
            r#"{{"Action":"read","Path":"/inert-fixture","Result":{raw}}}"#
        ))
        .unwrap();
        assert_eq!(
            decode_file(&current).unwrap_err().to_string(),
            format!(
                "decode file operation \"file\" state: json: cannot unmarshal JSON {shape} into Go operation.FileResult within \"/Result\""
            )
        );
        assert!(advance_file(current, None).is_err());
    }
    for (raw, present) in [("null", false), (r#"{"Text":"unchanged\r\n"}"#, true)] {
        let mut current = file("read", "/inert-fixture");
        current.state = Json::parse(&format!(
            r#"{{"Action":"read","Path":"/inert-fixture","Result":{raw}}}"#
        ))
        .unwrap();
        let result = decode_file(&current).unwrap().result;
        assert_eq!(result.is_some(), present);
        if let Some(result) = result {
            assert_eq!(result.text, "unchanged\r\n");
        }
    }
}
#[test]
fn cancellation_before_execution_does_not_create_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.txt");
    let state = FileState {
        action: "write".into(),
        path: path.to_str().unwrap().into(),
        content: "must not run".into(),
        ..FileState::default()
    };
    assert_eq!(
        run_file(&state, || true).unwrap_err().to_string(),
        "context canceled"
    );
    assert!(!path.exists());
}
