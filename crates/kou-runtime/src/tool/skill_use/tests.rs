use super::*;
use crate::operation::{SkillState, advance_skill};

fn encode(value: &impl serde::Serialize) -> serde_json::Result<kou_domain::Json> {
    serde_json::from_str(&serde_json::to_string(value)?)
}

fn setup() -> (SkillUse, SkillCatalog, RegistrationId) {
    let catalog = SkillCatalog::default();
    let id = catalog
        .register(Skill {
            name: "review".into(),
            path: "/inert/SKILL.md".into(),
            description: "review privately".into(),
            manual: true,
        })
        .unwrap();
    (SkillUse::new(catalog.clone()), catalog, id)
}
fn completed(bytes: Vec<u8>) -> Arc<Operation> {
    let mut current = skill_spec("/inert/SKILL.md".into())
        .unwrap()
        .operation("owned".into());
    current.status = "completed".into();
    current.state = encode(&SkillState {
        path: "/inert/SKILL.md".into(),
        content: bytes,
        terminal_error: String::new(),
    })
    .unwrap();
    Arc::new(current)
}

#[test]
fn selection_is_pure_exact_dynamic_and_submits_only_a_skill_spec() {
    let (translator, catalog, id) = setup();
    assert!(!translator.waits());
    assert!(translator.running(&[]).is_empty());
    let mut specs = Vec::new();
    let mut submit = |spec| {
        specs.push(spec);
        "owned".into()
    };
    let status = translator.translate_call(
        &ToolCall {
            name: "SkillUse".into(),
            arguments: r#"{"name":"review","unused":1e999}"#.into(),
            ..ToolCall::default()
        },
        &mut submit,
    );
    assert!(status.error.is_empty());
    assert_eq!(status.waiting_for.unwrap(), ["owned"]);
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].kind, "skill_use");
    assert_eq!(specs[0].version, 1);
    let current = specs.pop().unwrap().operation("owned".into());
    let step = advance_skill(current, None).unwrap();
    assert_eq!(step.dispatches[0].path, "/inert/SKILL.md");
    catalog.unregister(id);
    let status = translator.translate(r#"{"name":"review"}"#, &mut |_| {
        panic!("unregistered skill submitted")
    });
    assert_eq!(status.error, "skill \"review\" is not registered");
    catalog
        .register(Skill {
            name: "review".into(),
            path: "/inert/new/SKILL.md".into(),
            description: "new metadata".into(),
            manual: false,
        })
        .unwrap();
    let mut state = None;
    let status = translator.translate(r#"{"name":"review"}"#, &mut |spec| {
        state = Some(spec.state);
        "new".into()
    });
    assert!(status.error.is_empty());
    assert!(
        state
            .unwrap()
            .as_str()
            .unwrap()
            .contains("/inert/new/SKILL.md")
    );
}

#[test]
fn argument_validation_keeps_unbounded_errors_and_call_identity_without_submission() {
    let (translator, _, _) = setup();
    for raw in [
        "",
        " \n\t",
        "null",
        "{}",
        r#"{"name":null}"#,
        r#"{"Name":"review"}"#,
        r#"{"name":" \t "}"#,
    ] {
        assert_eq!(
            translator
                .translate(raw, &mut |_| panic!("invalid selection submitted"))
                .error,
            "skill-use argument \"name\" must be set"
        );
    }
    for name in ["review ", "Review", "missing"] {
        let raw = serde_json::to_string(&serde_json::json!({ "name": name })).unwrap();
        assert_eq!(
            translator
                .translate(&raw, &mut |_| panic!("unknown skill submitted"))
                .error,
            format!("skill {} is not registered", quoted(name))
        );
    }
    let status = translator.translate_call(
        &ToolCall {
            name: "mcp__gateway_SkillUse".into(),
            arguments: "{".into(),
            ..ToolCall::default()
        },
        &mut |_| panic!("wrong name submitted"),
    );
    assert_eq!(
        status.error,
        "skill-use call name \"mcp__gateway_SkillUse\" does not match static tool \"SkillUse\""
    );
    let raw = serde_json::to_string(&serde_json::json!({ "name": "界".repeat(50_000) })).unwrap();
    let status = translator.translate(&raw, &mut |_| panic!("unknown long skill submitted"));
    assert!(!status.error_truncated);
    assert!(status.error.chars().count() > DEFAULT_MAX_OUTPUT_LENGTH);
    assert!(matches!(
        skill_name(r#"{"name":"review"}"#).unwrap().unwrap(),
        Cow::Borrowed("review")
    ));
    assert!(matches!(
        skill_name(r#"{"name":"rev\u0069ew"}"#).unwrap().unwrap(),
        Cow::Owned(_)
    ));
}

#[test]
fn results_preserve_go_invalid_byte_runs_and_move_valid_content_without_copying() {
    let (translator, _, _) = setup();
    for (bytes, expected) in [
        (b"owned instructions".to_vec(), "owned instructions"),
        (b"a\xff\xffb\xc0\x80c".to_vec(), "a�b�c"),
        (
            b"\xe4\xb8\x96\xe7\x95\x8c\xff\xfe\xe2\x82".to_vec(),
            "世界�",
        ),
        (
            vec![0xff, 0xfe, b'x', 0xff, 0xef, 0xbf, 0xbd, 0xff],
            "�x���",
        ),
    ] {
        let result = translator
            .result("call", &CallStatus::default(), &[completed(bytes)])
            .unwrap();
        assert_eq!(result.call_id, "call");
        assert_eq!(result.output[0].kind, "text");
        assert_eq!(result.output[0].value, expected);
    }
    let bytes = b"move once".to_vec();
    let pointer = bytes.as_ptr();
    let text = valid_text(bytes);
    assert_eq!(text.as_ptr(), pointer);
}

#[test]
fn result_status_and_decode_validation_match_the_skill_specific_contract() {
    let (translator, _, _) = setup();
    let status = CallStatus {
        error: "raw validation error".into(),
        ..CallStatus::default()
    };
    assert_eq!(
        translator
            .result("call", &status, &[completed(vec![]), completed(vec![])])
            .unwrap()
            .output[0]
            .value,
        "raw validation error"
    );
    for current_status in [
        "ready",
        "awaiting",
        "canceling",
        "failed",
        "canceled",
        "unknown",
    ] {
        let mut current = (*completed(b"ignored partial".to_vec())).clone();
        current.status = current_status.into();
        let current = Arc::new(current);
        let result = translator.result(
            "call",
            &CallStatus::default(),
            std::slice::from_ref(&current),
        );
        if matches!(current_status, "ready" | "awaiting" | "canceling") {
            assert_eq!(result.unwrap().output[0].value, "Skill is loading.");
        } else {
            assert!(result.is_err());
        }
        if matches!(current_status, "failed" | "canceled") {
            let mut current = (*current).clone();
            let mut state = decode_skill(&current).unwrap();
            state.terminal_error = "owned terminal error".into();
            current.state = encode(&state).unwrap();
            assert_eq!(
                translator
                    .result("call", &CallStatus::default(), &[Arc::new(current)])
                    .unwrap()
                    .output[0]
                    .value,
                "owned terminal error"
            );
        }
    }
    assert_eq!(
        translator
            .result("call", &CallStatus::default(), &[])
            .unwrap_err()
            .to_string(),
        "skill-use call \"call\" has 0 operations, want 1"
    );
    let mut current = (*completed(vec![])).clone();
    current.kind = "file".into();
    assert_eq!(
        translator
            .result("call", &CallStatus::default(), &[Arc::new(current)])
            .unwrap_err()
            .to_string(),
        "decode skill-use call \"call\" result: decode skill-use operation \"owned\": type \"file\": unsupported operation"
    );
}
