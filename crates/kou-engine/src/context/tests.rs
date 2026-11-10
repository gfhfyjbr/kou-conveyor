use super::*;
use kou_domain::{Image, Usage};
use kou_provider::{anthropic, responses};

fn response(text: &str) -> Response {
    Response {
        output: Some(vec![LlmItem {
            provider_id: String::new(),
            model: String::new(),
            data: LlmData::Message(Message {
                role: "assistant".into(),
                text: text.into(),
                ..Message::default()
            }),
        }]),
        ..Response::default()
    }
}
fn ask(builder: &mut Builder, text: &str) {
    builder
        .add_external_input(&Input {
            id: "input".into(),
            kind: "external".into(),
            payload: Json::parse(&serde_json::to_string(text).unwrap()).unwrap(),
            ..Input::default()
        })
        .unwrap();
    builder.commit();
}
fn call(builder: &mut Builder, id: &str, name: &str, args: &str) {
    builder.add_response(Response {
        output: Some(vec![LlmItem {
            provider_id: String::new(),
            model: String::new(),
            data: LlmData::ToolCall(ToolCall {
                call_id: id.into(),
                name: name.into(),
                arguments: args.into(),
            }),
        }]),
        ..Response::default()
    });
}
fn text(value: &str) -> Vec<ToolResultOutput> {
    vec![ToolResultOutput {
        kind: "text".into(),
        value: value.into(),
    }]
}

#[test]
fn snapshots_share_bodies_and_encode_without_an_owned_history_copy() {
    let mut current = Builder::default();
    call(&mut current, "large", "Read", r#"{"path":"file.rs"}"#);
    current.add_tool_result("large", text(&"x".repeat(2 << 20)), false);
    current.commit();
    let first = current.build();
    let second = current.build();
    for (a, b) in first.request.input.iter().zip(&second.request.input) {
        assert!(Arc::ptr_eq(a, b));
    }
    let wire = responses::request_body(&first.request, "", &BTreeMap::new()).unwrap();
    assert!(wire.len() > 2 << 20);
    let mut request = second.request;
    request.model.id = "claude-opus-5".into();
    let config = anthropic::RequestConfig::default();
    assert!(
        anthropic::prepare(&request, &config, anthropic::AttemptOptions::default())
            .unwrap()
            .encode(4 << 20)
            .unwrap()
            .len()
            > 2 << 20
    );
}
#[test]
fn masking_cannot_mutate_an_inflight_snapshot() {
    let mut current = Builder::default();
    call(&mut current, "old", "Read", "{}");
    current.add_tool_result("old", text(&"old output ".repeat(1000)), false);
    current.commit();
    current.add_tool_result("latest", text("latest"), false);
    current.commit();
    let first = current.build();
    current.set_masking(MaskOptions { results: 1 });
    let masked = current.build();
    assert!(!Arc::ptr_eq(
        &first.request.input[2],
        &masked.request.input[2]
    ));
    let LlmData::ToolResult(old) = &first.request.input[2].data else {
        panic!()
    };
    assert!(old.output[0].value.starts_with("old output"));
    assert!(Arc::ptr_eq(
        &first.request.input[0],
        &masked.request.input[0]
    ));
}
#[test]
fn earlier_prompt_retention_is_bounded_before_compaction() {
    let mut current = Builder::default();
    for _ in 0..100 {
        ask(&mut current, &"я".repeat(10000));
        current.add_response(response("answer"));
    }
    assert!(current.answered.iter().map(String::len).sum::<usize>() <= 40_000);
    assert!(current.answered.len() < 10);
    assert!(current.answered_omitted > 90);
    let made = current.compact(&response("summary"), None).unwrap();
    assert!(made.message.contains("earlier ones are left out"));
    let checkpoint = current.checkpoint(&[]).unwrap();
    let mut restored = Builder::default();
    restored.restore_checkpoint(&checkpoint).unwrap();
    ask(&mut current, "new");
    ask(&mut restored, "new");
    current.add_response(response("answer"));
    restored.add_response(response("answer"));
    assert_eq!(
        current.compact(&response("next"), None).unwrap().message,
        restored.compact(&response("next"), None).unwrap().message
    );
}
#[test]
fn checkpoints_record_only_durable_progress_projection() {
    let mut current = Builder::default();
    call(&mut current, "running", "Bash", "{}");
    current.add_tool_result("running", text("recorded"), true);
    current.commit();
    current.refresh_tool_result("running", text("ephemeral"));
    current.compact(&response("summary"), None).unwrap();
    let checkpoint = current.checkpoint(&["running".into()]).unwrap();
    assert!(!checkpoint.as_str().unwrap().contains("ephemeral"));
    let mut restored = Builder::default();
    restored.restore_checkpoint(&checkpoint).unwrap();
    assert!(
        !serde_json::to_string(&restored.build())
            .unwrap()
            .contains("ephemeral")
    );
    assert!(restored.staged.is_empty());
    let LlmData::ToolResult(progress) = &current.staged[0].data else {
        panic!()
    };
    assert!(progress.output[0].value.contains("ephemeral"));
}
#[test]
fn checkpoint_keeps_configuration_but_restores_effort() {
    let mut current = Builder::default();
    ask(&mut current, "question");
    current.add_response(response("answer"));
    current.set_model(Model {
        id: "old".into(),
        reasoning_effort: "low".into(),
        max_output_tokens: None,
    });
    current.compact(&response("summary"), None).unwrap();
    let checkpoint = current.checkpoint(&[]).unwrap();
    let mut restored = Builder::default();
    restored.set_system_prompt("new system");
    restored.set_model(Model {
        id: "new".into(),
        reasoning_effort: "max".into(),
        max_output_tokens: Some(42),
    });
    restored.set_tools(vec![Tool {
        name: "NewTool".into(),
        ..Tool::default()
    }]);
    restored.restore_checkpoint(&checkpoint).unwrap();
    let built = restored.build();
    assert_eq!(built.request.model.id, "new");
    assert_eq!(built.request.model.reasoning_effort, "low");
    assert_eq!(built.request.model.max_output_tokens, Some(42));
    assert_eq!(built.request.tools[0].name, "NewTool");
    let LlmData::Message(system) = &built.request.input[0].data else {
        panic!()
    };
    assert!(system.text.ends_with("new system"));
}
#[test]
fn invalid_checkpoint_is_transactional() {
    let mut current = Builder::default();
    ask(&mut current, "keep me");
    let before = serde_json::to_value(current.build()).unwrap();
    for state in [
        "null",
        "[]",
        "{}",
        r#"{"Version":1,"Compacted":-1}"#,
        r#"{"Version":1,"Committed":[{"Type":"message","Data":{"Role":"assistant","Text":"wrong"}}]}"#,
    ] {
        assert!(
            current
                .restore_checkpoint(&Json::parse(state).unwrap())
                .is_err()
        );
        assert_eq!(serde_json::to_value(current.build()).unwrap(), before);
    }
}
#[test]
fn pruned_failure_does_not_become_a_successful_file_touch() {
    let mut current = Builder::default();
    call(&mut current, "failed", "Read", r#"{"path":"missing.rs"}"#);
    current.add_tool_result(
        "failed",
        text(&format!("Error: {}", "missing ".repeat(1000))),
        false,
    );
    current.commit();
    current.add_tool_result("new", text("new"), false);
    current.commit();
    current.set_masking(MaskOptions { results: 1 });
    current.build();
    let made = current.compact(&response("summary"), None).unwrap();
    assert!(!made.message.contains("missing.rs"));
    let mut restored = Builder::default();
    restored
        .restore_checkpoint(&current.checkpoint(&[]).unwrap())
        .unwrap();
    assert!(
        !restored
            .compact(&response("next"), None)
            .unwrap()
            .message
            .contains("missing.rs")
    );
}
#[test]
fn pruning_adjusts_recorded_prefix_count_for_checkpoint_recovery() {
    let mut current = Builder::default();
    current.set_pruning(PruneOptions {
        budget: 1,
        thinking_turns: 1,
        ..PruneOptions::default()
    });
    for _ in 0..6 {
        ask(&mut current, "question");
        current.add_response(Response {
            output: Some(vec![
                LlmItem {
                    provider_id: String::new(),
                    model: String::new(),
                    data: LlmData::Reasoning(Reasoning {
                        raw: Json::parse(r#""thinking""#).unwrap(),
                        summary: None,
                    }),
                },
                LlmItem {
                    provider_id: String::new(),
                    model: String::new(),
                    data: LlmData::Message(Message {
                        text: "answer".into(),
                        role: "assistant".into(),
                        ..Message::default()
                    }),
                },
            ]),
            ..Response::default()
        });
        current.build();
    }
    assert!(current.thinking_left_out > 0);
    current.compact(&response("summary"), None).unwrap();
    let state: serde_json::Value = current.checkpoint(&[]).unwrap().decode().unwrap();
    assert_eq!(state["Compacted"], 18);
}
#[test]
fn image_window_excludes_staged_input_and_preserves_snapshot() {
    let mut current = Builder::default();
    for _ in 0..41 {
        let message = ExternalMessage {
            images: vec![Image {
                url: "https://example.invalid/image".into(),
                label: "shot".into(),
            }],
            ..ExternalMessage::default()
        };
        current
            .add_external_input(&Input {
                id: "image".into(),
                kind: "external".into(),
                payload: external_payload(&message).unwrap(),
                ..Input::default()
            })
            .unwrap();
        current.commit();
    }
    let built = current.build();
    assert_eq!(
        built
            .request
            .input
            .iter()
            .filter_map(|item| if let LlmData::Message(message) = &item.data {
                message.images.as_ref()
            } else {
                None
            })
            .map(Vec::len)
            .sum::<usize>(),
        30
    );
    assert_eq!(
        built.report.changes[0].reason,
        "11 older images were left out to keep the requests within the provider's limits"
    );
}
#[test]
fn notes_keep_utf8_boundaries_and_heartbeat_acknowledges_exact_text() {
    let raw = "я".repeat(20000);
    let shown = shown_notes(&raw, "/notes");
    assert!(shown.len() < NOTES_LIMIT + 300);
    assert!(!shows_whole_notes(&shown));
    let mut current = Builder::default();
    current.set_memory(Memory {
        notes: "/notes".into(),
        read_notes: Some(Arc::new(|| Ok("new".into()))),
        ..Memory::default()
    });
    current.compact(
        &response("summary"),
        Some(&Compacted {
            message: "kept summary".into(),
            notes: Some("old".into()),
        }),
    );
    let update = current.notes_update().unwrap();
    current
        .add_control(&ControlMessage {
            mode: "heartbeat".into(),
            reason: update,
            ..ControlMessage::default()
        })
        .unwrap();
    assert!(current.notes_update().is_none());
}
#[test]
fn tool_count_and_provider_usage_do_not_double_count_history() {
    let mut current = Builder::default();
    current.set_tools(vec![Tool {
        name: "tool".into(),
        description: "desc".into(),
        ..Tool::default()
    }]);
    ask(&mut current, "question");
    let mut answer = response("answer");
    answer.usage = Usage {
        input_tokens: 1000,
        output_tokens: 20,
        ..Usage::default()
    };
    current.add_response(answer);
    assert_eq!(current.build().estimated_tokens, 1020);
    ask(&mut current, "next");
    assert_eq!(current.build().estimated_tokens, 1025);
}
#[test]
fn external_payload_validates_file_and_image_references() {
    let message = ExternalMessage {
        images: vec![Image::default()],
        ..ExternalMessage::default()
    };
    assert_eq!(
        external_payload(&message).unwrap_err().to_string(),
        "image 1 has no URL"
    );
    assert_eq!(
        decode_message(&Json::parse(r#"{"Text":"text"}"#).unwrap())
            .unwrap_err()
            .to_string(),
        "a message without images or files is a JSON string of its text"
    );
    assert_eq!(
        decode_message(
            &external_payload(&ExternalMessage {
                text: "text".into(),
                ..ExternalMessage::default()
            })
            .unwrap()
        )
        .unwrap()
        .text,
        "text"
    );
}

#[test]
fn image_probes_validate_headers_without_decoding_pixels() {
    use base64::Engine;
    let mut header = Vec::from(*b"\x89PNG\r\n\x1a\n");
    let mut body = Vec::from(*b"IHDR");
    body.extend_from_slice(&300u32.to_be_bytes());
    body.extend_from_slice(&250u32.to_be_bytes());
    body.extend_from_slice(&[8, 0, 0, 0, 0]);
    header.extend_from_slice(&13u32.to_be_bytes());
    header.extend_from_slice(&body);
    header.extend_from_slice(&crc32fast::hash(&body).to_be_bytes());
    // Like Go DecodeConfig, grayscale config completes after IHDR, before IDAT.
    let encode = |bytes: &[u8]| {
        format!(
            "data:unknown;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    };
    assert_eq!(images::image_tokens(&encode(&header)), 100);
    header[32] ^= 1;
    assert_eq!(images::image_tokens(&encode(&header)), 1600);
    assert_eq!(
        images::image_tokens(&encode(b"GIF89a\x2c\x01\xfa\0\x80\0\0")),
        1600
    );
}

#[test]
fn image_byte_budget_removes_whole_old_groups_without_touching_pending_input() {
    let mut current = Builder::default();
    for _ in 0..6 {
        current.committed.push(item(LlmData::Message(Message {
            role: "user".into(),
            images: Some(vec![Image {
                url: "x".repeat(4 << 20),
                label: "shot".into(),
            }]),
            ..Message::default()
        })));
    }
    current.staged.push(item(LlmData::Message(Message {
        role: "user".into(),
        images: Some(vec![Image {
            url: "https://example.invalid/new".into(),
            label: "new".into(),
        }]),
        ..Message::default()
    })));
    current.bound_images();
    assert_eq!(current.images_left_out, 3);
    let bytes: usize = current
        .committed
        .iter()
        .filter_map(|item| {
            if let LlmData::Message(message) = &item.data {
                message.images.as_ref()
            } else {
                None
            }
        })
        .flatten()
        .map(|image| image.url.len())
        .sum();
    assert_eq!(bytes, 12 << 20);
    assert!(
        matches!(&current.staged[0].data, LlmData::Message(message) if message.images.as_ref().unwrap()[0].label == "new")
    );
}
