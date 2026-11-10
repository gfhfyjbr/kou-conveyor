use super::*;
use crate::operation::FileResult;

fn tool(kind: &str) -> File {
    File::new(
        kind,
        "Alias",
        FileConfig {
            directory: "/inert/work/".into(),
            transcript: "/inert/transcript".into(),
        },
    )
}
fn translate(tool: &File, arguments: &str) -> (CallStatus, Vec<Spec>) {
    let mut specs = Vec::new();
    let status = tool.translate(arguments, &mut |spec| {
        specs.push(spec);
        "submitted".into()
    });
    (status, specs)
}
fn operation(path: &str, text: &str) -> Arc<Operation> {
    let mut operation = file_spec(FileState {
        action: "read".into(),
        path: path.into(),
        result: Some(FileResult {
            text: text.into(),
            ..FileResult::default()
        }),
        ..FileState::default()
    })
    .unwrap()
    .operation("owned".into());
    operation.status = "completed".into();
    Arc::new(operation)
}
#[test]
fn file_calls_preserve_path_spelling_and_borrow_unused_unescaped_arguments() {
    let parsed =
        Parsed::parse(r#"{"path":" a/../b ","patch":"unused patch","content":"escaped\ntext"}"#)
            .unwrap();
    assert!(matches!(parsed.path, Cow::Borrowed(_)));
    assert!(matches!(parsed.patch, Cow::Borrowed(_)));
    assert!(matches!(parsed.content, Cow::Owned(_)));
    let (status, specs) = translate(
        &tool("read"),
        r#"{"path":" a/../b ","file_path":"ignored","offset":null,"max_output_length":false,"unknown":1e999}"#,
    );
    assert!(status.error.is_empty());
    let state = decode_file(&specs.into_iter().next().unwrap().operation("owned".into())).unwrap();
    assert_eq!(state.path, "/inert/work//a/../b");
    assert_eq!((state.offset, state.limit), (0, 0));
    for (kind, arguments, action) in [
        ("write", r#"{"path":"x"}"#, "write"),
        (
            "edit",
            r#"{"path":"/absolute","old_string":"","new_string":"","replace_all":null}"#,
            "edit",
        ),
        (
            "apply_patch",
            r#"{"input":"   ","patch":" raw patch \n"}"#,
            "patch",
        ),
        (
            "transcript_search",
            r#"{"query":" needle ","limit":"2"}"#,
            "transcript_search",
        ),
    ] {
        let (status, specs) = translate(&tool(kind), arguments);
        assert!(status.error.is_empty(), "{kind}: {}", status.error);
        let state =
            decode_file(&specs.into_iter().next().unwrap().operation("owned".into())).unwrap();
        assert_eq!(state.action, action);
        if kind == "edit" {
            assert_eq!(state.path, "/absolute");
        }
        if kind == "apply_patch" {
            assert_eq!(state.patch, " raw patch \n");
            assert_eq!(state.root, "/inert/work/");
        }
        if kind == "transcript_search" {
            assert_eq!(state.query, " needle ");
            assert_eq!(state.path, "/inert/transcript");
            assert_eq!(state.limit, 2);
        }
    }
}
#[test]
fn file_validation_preserves_typed_field_order_and_does_not_submit_invalid_work() {
    let read = tool("read");
    for raw in ["null", "[]", "{", r#"{"path":"a","path":"b"}"#] {
        let (status, specs) = translate(&read, raw);
        assert_eq!(status.error, "Alias arguments must be a JSON object");
        assert!(specs.is_empty());
    }
    for (raw, pointer) in [
        (r#"{"path":42,"offset":"soon"}"#, "/path"),
        (r#"{"offset":"soon","path":42}"#, "/offset"),
        (r#"{"offset":1.5,"path":42}"#, "/path"),
        (r#"{"path":"a","replace_all":"true"}"#, "/replace_all"),
    ] {
        let (status, specs) = translate(&read, raw);
        assert!(
            status.error.contains(&format!("within {pointer:?}")),
            "{}",
            status.error
        );
        assert!(specs.is_empty());
    }
    let (status, _) = translate(&read, r#"{"path":"a","offset":1e100}"#);
    assert_eq!(
        status.error,
        "Alias argument \"offset\" must be a whole number, not 1e+100"
    );
    let (status, _) = translate(&tool("write"), r#"{"path":"a","offset":-1}"#);
    assert_eq!(
        status.error,
        "Alias argument \"offset\" must not be negative"
    );
    let (status, _) = translate(
        &tool("apply_patch"),
        r#"{"path":"a\u0000b","input":"patch"}"#,
    );
    assert_eq!(status.error, "Alias argument \"path\" contains a NUL byte");
}
#[test]
fn file_counts_follow_go_exact_integer_range_before_rejecting_negatives() {
    let read = tool("read");
    for value in ["9007199254740992", "9007199254740993", "\"0x1p53\""] {
        let (status, specs) = translate(&read, &format!(r#"{{"path":"x","offset":{value}}}"#));
        assert!(status.error.is_empty(), "{}", status.error);
        let state =
            decode_file(&specs.into_iter().next().unwrap().operation("owned".into())).unwrap();
        assert_eq!(state.offset, 1i64 << 53);
    }
    for (value, diagnostic) in [
        (
            "9007199254740994",
            "must be a whole number, not 9.007199254740994e+15",
        ),
        ("-9007199254740992", "must not be negative"),
        (
            "-9007199254740994",
            "must be a whole number, not -9.007199254740994e+15",
        ),
        (
            "-9223372036854775808",
            "must be a whole number, not -9.223372036854776e+18",
        ),
    ] {
        let (status, specs) = translate(&read, &format!(r#"{{"path":"x","offset":{value}}}"#));
        assert_eq!(
            status.error,
            format!("Alias argument \"offset\" {diagnostic}")
        );
        assert!(specs.is_empty());
    }
}
#[test]
fn file_results_keep_text_and_reject_ambiguous_or_incomplete_terminal_states() {
    let read = tool("read");
    let operation = operation("/inert/read", "unchanged\r\n");
    let result = read
        .result(
            "call",
            &CallStatus::default(),
            std::slice::from_ref(&operation),
        )
        .unwrap();
    assert_eq!(result.call_id, "call");
    assert_eq!(result.output[0].value, "unchanged\r\n");
    assert_eq!(
        format_file_result("Alias", "call", &operation).unwrap(),
        "unchanged\r\n"
    );
    assert!(read.result("call", &CallStatus::default(), &[]).is_err());
    assert!(
        read.result(
            "call",
            &CallStatus {
                error: "validation".into(),
                ..CallStatus::default()
            },
            std::slice::from_ref(&operation)
        )
        .is_err()
    );
    for status in ["failed", "canceled", "completed"] {
        let mut current = file_spec(FileState {
            action: "read".into(),
            path: "/inert/read".into(),
            ..FileState::default()
        })
        .unwrap()
        .operation("owned".into());
        current.status = status.into();
        assert!(
            read.result("call", &CallStatus::default(), &[Arc::new(current)])
                .is_err()
        );
    }
    assert!(read.running(&[operation]).is_empty());
}
#[test]
fn repeat_read_notes_are_idempotent_reset_on_change_and_keep_raw_part_identity() {
    let read = tool("read");
    let first = operation("/inert/read", "same");
    let render = |id: &str, operation| {
        read.result(id, &CallStatus::default(), &[operation])
            .unwrap()
            .output
            .remove(0)
            .value
    };
    assert_eq!(render("one", first.clone()), "same");
    assert_eq!(render("two", first.clone()), "same");
    let third = render("three", first.clone());
    assert!(third.contains("read 3 times, unchanged since the first"));
    assert_eq!(render("three", first.clone()), third);
    assert_eq!(
        render("changed", operation("/inert/read", "changed")),
        "changed"
    );
    assert_eq!(render("back", first.clone()), "same");
    assert_eq!(
        render("raw-alias", operation("/inert/./read", "same")),
        "same"
    );
    let cached_different_path = render("three", operation("/other/current-path", "other text"));
    assert!(cached_different_path.starts_with("other text"));
    assert!(
        cached_different_path
            .contains("These lines of /other/current-path have now been read 3 times")
    );
}
