use super::*;

fn operation(status: &str, content: &[u8]) -> Operation {
    let mut current = skill_spec("/inert/raw/../SKILL.md".into())
        .unwrap()
        .operation("owned".into());
    current.status = status.into();
    current.max_output_length = 1;
    current.idempotency = Json::parse(r#"{"opaque":"owned fixture"}"#).unwrap();
    current.state = encode(&SkillState {
        path: "/inert/raw/../SKILL.md".into(),
        content: content.into(),
        ..SkillState::default()
    })
    .unwrap();
    current
}
fn event(kind: &str, result: SkillEventResult) -> SkillEvent {
    SkillEvent {
        kind: kind.into(),
        source: "owned".into(),
        correlation_id: CORRELATION.into(),
        result,
    }
}
fn complete(current: Operation, event: SkillEvent) -> Operation {
    let step = advance_skill(current, Some(event)).unwrap();
    assert!(step.dispatches.is_empty());
    step.operation.unwrap()
}
fn state(current: &Operation) -> SkillState {
    decode_skill(current).unwrap()
}

#[test]
fn skill_specs_and_headers_keep_raw_paths_and_go_byte_string_shape() {
    assert_eq!(
        skill_spec(String::new()).unwrap_err().0,
        "skill path must be set"
    );
    for path in [" \n\t", " raw/../SKILL.md ", "\0", "/inert//SKILL.md"] {
        let spec = skill_spec(path.into()).unwrap();
        assert_eq!(spec.max_output_length, 0);
        assert_eq!(
            spec.state.as_str().unwrap(),
            format!(
                r#"{{"Path":{},"Content":"","TerminalError":""}}"#,
                serde_json::to_string(path).unwrap()
            )
        );
    }
    let mut current = operation("ready", b"");
    current.kind = "file".into();
    assert_eq!(
        decode_skill(&current).unwrap_err().0,
        "decode skill-use operation \"owned\": type \"file\": unsupported operation"
    );
    current.kind = "skill_use".into();
    current.version = 2;
    assert_eq!(
        decode_skill(&current).unwrap_err().0,
        "decode skill-use operation \"owned\": version 2: unsupported operation"
    );
}

#[test]
fn skill_ready_requires_initial_state_and_awaiting_recovery_resumes_saved_bytes() {
    let initial = operation("ready", b"");
    let step = advance_skill(initial.clone(), None).unwrap();
    let updated = step.operation.unwrap();
    assert_eq!(updated.idempotency, initial.idempotency);
    assert_eq!(updated.max_output_length, 1);
    assert_eq!(updated.status, "awaiting");
    assert!(state(&updated).content.is_empty());
    let request = &step.dispatches[0];
    assert_eq!(request.source, "owned");
    assert_eq!(request.correlation_id, CORRELATION);
    assert_eq!(request.path, "/inert/raw/../SKILL.md");
    assert_eq!(request.offset, 0);
    assert_eq!(request.count, i64::MAX);
    assert_eq!(
        advance_skill(operation("ready", b"stale"), None)
            .unwrap_err()
            .0,
        "advance ready skill-use operation \"owned\": state is not initial"
    );
    let mut invalid = operation("awaiting", b"");
    let mut fields = state(&invalid);
    fields.terminal_error = "stale".into();
    invalid.state = encode(&fields).unwrap();
    assert_eq!(
        advance_skill(invalid, None).unwrap_err().0,
        "advance awaiting skill-use operation \"owned\": state is terminal"
    );
    let step = advance_skill(operation("awaiting", b"saved \xff"), None).unwrap();
    assert_eq!(step.dispatches[0].offset, 7);
    assert_eq!(state(&step.operation.unwrap()).content, b"saved \xff");
}

#[test]
fn skill_chunks_checkpoint_exact_bytes_and_complete_without_text_conversion() {
    let mut current = operation("awaiting", b"");
    let chunks: [&[u8]; 4] = [b"prefix", b"", b"\xff\x00\xe4", b"\xb8\xad"];
    let mut expected = Vec::new();
    for chunk in chunks {
        current = complete(
            current,
            event(
                "io.read_output",
                SkillEventResult::Output {
                    offset: expected.len() as i64,
                    data: chunk.into(),
                },
            ),
        );
        expected.extend_from_slice(chunk);
        assert_eq!(current.status, "awaiting");
        assert_eq!(state(&current).content, expected);
        let recovered = advance_skill(current.clone(), None).unwrap();
        assert_eq!(recovered.dispatches[0].offset, expected.len() as i64);
    }
    current = complete(
        current,
        event(
            "io.read_completed",
            SkillEventResult::Completed {
                size: expected.len() as i64,
            },
        ),
    );
    assert_eq!(current.status, "completed");
    assert_eq!(state(&current).content, expected);
    assert_eq!(
        advance_skill(current, None).unwrap_err().0,
        "advance skill-use operation \"owned\": terminal status \"completed\""
    );
}

#[test]
fn skill_event_failures_preserve_partial_content_and_do_not_clip_errors() {
    let current = operation("awaiting", b"partial");
    let mut foreign = event("io.read_output", SkillEventResult::Empty);
    foreign.source = "other".into();
    let failed = complete(current.clone(), foreign);
    assert_eq!(
        state(&failed).terminal_error,
        "skill-use primitive event source is \"other\", want \"owned\""
    );
    let mut wrong = event("io.read_output", SkillEventResult::Empty);
    wrong.correlation_id = "wrong".into();
    let failed = complete(current.clone(), wrong);
    assert_eq!(
        state(&failed).terminal_error,
        "skill-use primitive event correlation is \"wrong\", want \"skill-use-read\""
    );
    for failure in [
        String::new(),
        "owned failure".into(),
        "long界".repeat(40001),
    ] {
        let failed = complete(
            current.clone(),
            event(
                "primitive.failed",
                SkillEventResult::Failure(failure.clone()),
            ),
        );
        assert_eq!(failed.status, "failed");
        assert_eq!(state(&failed).content, b"partial");
        assert_eq!(state(&failed).terminal_error, failure);
    }
    for result in [
        SkillEventResult::Empty,
        SkillEventResult::Output {
            offset: 8,
            data: b"ignored".to_vec(),
        },
    ] {
        let failed = complete(current.clone(), event("io.read_output", result));
        assert_eq!(failed.status, "failed");
        assert_eq!(
            state(&failed).terminal_error,
            "skill read returned invalid output"
        );
        assert_eq!(state(&failed).content, b"partial");
    }
    let failed = complete(
        current.clone(),
        event(
            "io.read_completed",
            SkillEventResult::Completed { size: -1 },
        ),
    );
    assert_eq!(
        state(&failed).terminal_error,
        "skill read returned an invalid completion"
    );
    let failed = complete(current, event("primitive.failed", SkillEventResult::Empty));
    assert_eq!(
        state(&failed).terminal_error,
        "skill-use primitive failed with an invalid result"
    );
}

#[test]
fn skill_cancellation_and_failure_precede_correlation_but_not_source_checks() {
    for status in ["completed", "failed", "canceled"] {
        assert_eq!(
            advance_skill(operation(status, b"saved"), None)
                .unwrap_err()
                .0,
            format!("advance skill-use operation \"owned\": terminal status {status:?}")
        );
    }
    assert_eq!(
        advance_skill(
            operation("canceling", b"saved"),
            Some(event("unexpected", SkillEventResult::Empty))
        )
        .unwrap_err()
        .0,
        "advance canceling skill-use operation \"owned\": unexpected primitive event"
    );
    let canceled = advance_skill(operation("canceling", b"saved"), None)
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(canceled.status, "canceled");
    assert_eq!(state(&canceled).content, b"saved");
    assert_eq!(
        state(&canceled).terminal_error,
        "skill-use operation canceled"
    );
    let mut canceled = event(
        "primitive.canceled",
        SkillEventResult::Raw(Json::parse("42").unwrap()),
    );
    canceled.correlation_id = "wrong".into();
    assert_eq!(
        complete(operation("awaiting", b"saved"), canceled).status,
        "canceled"
    );
    let mut failed = event(
        "primitive.failed",
        SkillEventResult::Failure("owned error".into()),
    );
    failed.correlation_id = "wrong".into();
    assert_eq!(
        state(&complete(operation("awaiting", b"saved"), failed)).terminal_error,
        "owned error"
    );
    let mut foreign = event("primitive.canceled", SkillEventResult::Empty);
    foreign.source = "other".into();
    assert_eq!(
        complete(operation("awaiting", b"saved"), foreign).status,
        "failed"
    );
}

#[test]
fn skill_wire_rejects_positional_byte_arrays_and_keeps_typed_error_order() {
    for (raw, expected) in [
        (
            r#"{"Path":"x","Content":[]}"#,
            "JSON array into Go []uint8 within \"/Content\"",
        ),
        (
            r#"{"Path":42,"Content":[]}"#,
            "JSON number into Go string within \"/Path\"",
        ),
        (
            r#"{"Content":[],"Path":42}"#,
            "JSON array into Go []uint8 within \"/Content\"",
        ),
        (
            r#"{"Path":"x","Content":"YQ==\n"}"#,
            "JSON string into Go []uint8 within \"/Content\": illegal character '\\n' at offset 4",
        ),
    ] {
        let mut current = operation("ready", b"");
        current.state = Json::parse(raw).unwrap();
        assert_eq!(
            decode_skill(&current).unwrap_err().0,
            format!(
                "decode skill-use operation \"owned\" state: json: cannot unmarshal {expected}"
            )
        );
    }
    for value in ["null", r#""""#, r#""YR==""#] {
        let mut current = operation("ready", b"");
        current.state = Json::parse(&format!(r#"{{"Path":"x","Content":{value}}}"#)).unwrap();
        assert_eq!(
            state(&current).content,
            if value == r#""YR==""# {
                b"a".as_slice()
            } else {
                b"".as_slice()
            }
        );
    }
}
