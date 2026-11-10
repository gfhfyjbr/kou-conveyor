use kou_domain::LlmData;
use kou_provider::responses::{ResponseState, StreamDecoder, StreamLimits};
use serde_json::{Value, json};

fn observe(state: &mut ResponseState, value: Value) {
    state
        .observe(serde_json::to_string(&value).unwrap().as_bytes(), |_| {})
        .unwrap();
}
fn message(text: &str) -> Value {
    json!({"type":"message","id":text,"content":[{"type":"output_text","text":text}]})
}

#[test]
fn event_and_nested_error_arrays_are_rejected_instead_of_positional_structs() {
    for raw in [
        br#"["error"]"#.as_slice(),
        br#"{"type":"error","error":["code","message"]}"#,
    ] {
        let mut state = ResponseState::new(StreamLimits::default()).unwrap();
        let error = state
            .observe(raw, |_| panic!("invalid event was reported"))
            .unwrap_err();
        assert!(error.0.starts_with("invalid Responses stream event JSON:"));
        assert!(state.api_error().is_none());
    }
}

#[test]
fn fallback_items_are_index_ordered_and_duplicates_replace_without_overwriting_terminal_fields() {
    let mut state = ResponseState::new(StreamLimits::default()).unwrap();
    for (index, text) in [(9, "last"), (0, "first"), (9, "replaced")] {
        observe(
            &mut state,
            json!({"type":"response.output_item.done","output_index":index,"item":message(text)}),
        );
    }
    observe(
        &mut state,
        json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[],"usage":{"input_tokens":12},"vendor":"keep"}}),
    );
    let raw = state.into_body().unwrap();
    let value: Value = serde_json::from_str(raw.get()).unwrap();
    assert_eq!(value["vendor"], "keep");
    assert_eq!(value["output"][0]["id"], "first");
    assert_eq!(value["output"][1]["id"], "replaced");
}

#[test]
fn terminal_nonempty_output_wins_and_late_items_are_ignored() {
    let mut state = ResponseState::new(StreamLimits::default()).unwrap();
    observe(
        &mut state,
        json!({"type":"response.output_item.done","output_index":0,"item":message("discard")}),
    );
    observe(
        &mut state,
        json!({"type":"response.completed","response":{"status":"completed","output":[message("authoritative")]}}),
    );
    observe(
        &mut state,
        json!({"type":"response.output_item.done","output_index":1,"item":message("late")}),
    );
    let response = state.into_response().unwrap();
    let output = response.output.unwrap();
    assert_eq!(output.len(), 1);
    assert!(matches!(&output[0].data,LlmData::Message(m) if m.text == "authoritative"));
}

#[test]
fn indexed_deltas_and_summary_paragraph_separators_match_go() {
    let mut state = ResponseState::new(StreamLimits::default()).unwrap();
    let mut events = Vec::new();
    for value in [
        json!({"type":"response.output_text.delta","output_index":2,"delta":"hello"}),
        json!({"type":"response.reasoning_summary_text.delta","output_index":1,"summary_index":0,"delta":"think"}),
        json!({"type":"response.reasoning_summary_text.delta","output_index":1,"summary_index":1,"delta":"hidden"}),
        json!({"type":"response.reasoning_text.delta","output_index":3,"delta":"ignored raw reasoning"}),
        json!({"type":"response.reasoning_summary_part.added","output_index":1,"summary_index":1}),
        json!({"type":"response.output_text.delta","delta":"no index"}),
        json!({"type":"response.output_text.delta","output_index":-1,"delta":"negative"}),
        json!({"type":"response.output_text.delta","output_index":0,"delta":42}),
    ] {
        state
            .observe(serde_json::to_string(&value).unwrap().as_bytes(), |event| {
                events.push(event)
            })
            .unwrap();
    }
    assert_eq!(
        events
            .iter()
            .map(|e| (e.index, e.kind.as_str(), e.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (2, "text", "hello"),
            (1, "reasoning", "think"),
            (1, "reasoning", "hidden"),
            (1, "reasoning", "\n\n")
        ]
    );
}

#[test]
fn nested_and_flat_errors_are_sticky_and_preserve_native_metadata() {
    let mut state = ResponseState::new(StreamLimits::default()).unwrap();
    observe(
        &mut state,
        json!({"type":"error","error":{"code":"denied","message":"no","type":"permission_error","param":"input","resets_in_seconds":12}}),
    );
    let error = state.api_error().unwrap();
    assert_eq!(error.to_string(), "responses API error denied: no");
    assert_eq!(error.kind, "permission_error");
    assert_eq!(error.resets_in_seconds.as_str(), Some("12"));
    assert!(
        state
            .observe(b"{}", |_| {})
            .unwrap_err()
            .0
            .contains("denied")
    );
    assert!(state.into_response().unwrap_err().0.contains("denied"));
}

#[test]
fn state_caps_are_checked_before_mutating_and_sparse_indices_do_not_allocate_a_huge_array() {
    let limits = StreamLimits {
        retained_bytes: 120,
        output_items: 1,
        ..Default::default()
    };
    let mut state = ResponseState::new(limits).unwrap();
    observe(
        &mut state,
        json!({"type":"response.output_item.done","output_index":i64::MAX,"item":message("one")}),
    );
    let before = state.retained_bytes();
    assert!(state.observe(serde_json::to_string(&json!({"type":"response.output_item.done","output_index":0,"item":message("two")})).unwrap().as_bytes(),|_| {}).is_err());
    assert_eq!(state.retained_bytes(), before);
    assert!(state.observe(serde_json::to_string(&json!({"type":"response.output_item.done","output_index":i64::MAX,"item":message(&"x".repeat(200))})).unwrap().as_bytes(),|_| {}).is_err());
    assert_eq!(state.retained_bytes(), before);
}

#[test]
fn decoder_handles_every_chunk_size_and_never_uses_a_partial_terminal_frame() {
    let terminal = json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[message("中🎉")]}}).to_string();
    let wire = format!("\u{feff}:comment\r\ndata: {terminal}\r\n\r\ngarbage after terminal");
    for size in 1..=wire.len() {
        let mut decoder = StreamDecoder::new(StreamLimits::default()).unwrap();
        for bytes in wire.as_bytes().chunks(size) {
            if decoder.feed(bytes, |_| {}).unwrap() {
                break;
            }
        }
        let result = decoder.finish().unwrap();
        assert_eq!(result.id, "r");
    }
    for wire in [
        format!("data: {terminal}\n"),
        "data: [DONE]\n\n".into(),
        "".into(),
        "data: {\"type\":\"response.completed\"}\n\n".into(),
    ] {
        let mut decoder = StreamDecoder::new(StreamLimits::default()).unwrap();
        decoder.feed(wire.as_bytes(), |_| {}).unwrap();
        assert!(decoder.finish().unwrap_err().0.contains("unexpected EOF"));
    }
}
