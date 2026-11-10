use kou_domain::{Json, LlmData, LlmItem, Message, Reasoning};
use kou_provider::{
    Request,
    responses::{decode_response, request_body},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request_body(request, "", &BTreeMap::new()).unwrap()).unwrap()
}
fn request(input: Value) -> Request {
    serde_json::from_value(json!({"Model":{"ID":"current"},"Input":input})).unwrap()
}

#[test]
fn positional_arrays_cannot_impersonate_response_objects_or_token_details() {
    for body in [
        r#"["id","completed",[]]"#,
        r#"{"status":"completed","output":[],"usage":[11,7]}"#,
        r#"{"status":"completed","output":[],"usage":{"input_tokens_details":[3,5]}}"#,
        r#"{"status":"completed","output":[],"usage":{"output_tokens_details":[3]}}"#,
        r#"{"status":"completed","output":[["message"]]}"#,
        r#"{"status":"completed","output":[{"type":"reasoning","summary":[["secret"]]}]}"#,
        r#"{"status":"failed","error":["code","message"]}"#,
    ] {
        assert!(decode_response(body.as_bytes()).is_err(), "{body}");
    }
    assert!(
        decode_response(
            br#"{"status":"completed","output":[],"usage":{"input_tokens_details":null}}"#
        )
        .is_ok()
    );
}

#[test]
fn request_cross_model_replay_drops_reasoning_and_provider_ids_without_mutating_source() {
    let original = request(json!([
        {"Model":"old","ProviderID":"old-id","Type":"message","Data":{"Role":"assistant","Text":"old text","Phase":"final_answer"}},
        {"Model":"old","ProviderID":"old-reason","Type":"reasoning","Data":{"Raw":{"type":"reasoning","encrypted_content":"secret"}}},
        {"Model":"current","ProviderID":"own-id","Type":"message","Data":{"Role":"assistant","Text":"own"}},
        {"Type":"reasoning","Data":{"Raw":{"type":"thinking","signature":"not-openai"}}}
    ]));
    let copy = serde_json::to_vec(&original).unwrap();
    let body = body(&original);
    assert_eq!(body["input"].as_array().unwrap().len(), 2);
    assert_eq!(body["input"][0]["content"], "old text");
    assert!(body["input"][0].get("id").is_none());
    assert_eq!(body["input"][1]["id"], "own-id");
    assert_eq!(serde_json::to_vec(&original).unwrap(), copy);
}

#[test]
fn missing_results_complete_groups_and_late_images_keep_empty_text_boundaries() {
    let request = request(json!([
        {"Type":"tool_call","Data":{"CallID":"unrecorded","Name":"g","Arguments":"{}"}},
        {"Type":"tool_call","Data":{"CallID":"missing","Name":"f","Arguments":"{}"}},
        {"Type":"tool_result","Data":{"CallID":"missing","Output":[{"Kind":"text","Value":"first"}]}},
        {"Type":"message","Data":{"Role":"user","Text":"next"}},
        {"Type":"tool_result","Data":{"CallID":"missing","Output":[{"Kind":"image","Value":"data:image/png;base64,eA=="},{"Kind":"text","Value":""},{"Kind":"text","Value":"end"}]}}
    ]));
    let input = body(&request)["input"].as_array().unwrap().clone();
    assert_eq!(input[3]["type"], "function_call_output");
    assert!(
        input[3]["output"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No result was recorded")
    );
    assert_eq!(input[5]["role"], "user");
    assert_eq!(input[5]["content"][2]["text"], "\nend");
}

#[test]
fn malformed_arguments_become_a_safe_object_and_extension_overrides_are_rejected() {
    for arguments in ["", "null", "[]", "half an object", "{\"n\":1,\"n\":2}"] {
        let input = request(
            json!([{"Type":"tool_call","Data":{"CallID":"c","Name":"f","Arguments":arguments}}]),
        );
        let value = body(&input);
        let wrapped: Value =
            serde_json::from_str(value["input"][0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(wrapped["invalid_arguments"], arguments);
    }
    assert!(
        request_body(
            &Request::default(),
            "",
            &BTreeMap::from([("stream".into(), Json::parse("true").unwrap())])
        )
        .unwrap_err()
        .0
        .contains("overrides")
    );
}

#[test]
fn request_borrows_multi_megabyte_text_and_preserves_opaque_numbers() {
    let request = Request {
        input: vec![
            LlmItem {
                provider_id: String::new(),
                model: String::new(),
                data: LlmData::Message(Message {
                    role: "user".into(),
                    text: "x".repeat(4 << 20),
                    ..Default::default()
                }),
            },
            LlmItem {
                provider_id: String::new(),
                model: String::new(),
                data: LlmData::Reasoning(Reasoning {
                    raw: Json::parse("{\"type\":\"reasoning\",\"vendor\":1e999}").unwrap(),
                    ..Default::default()
                }),
            },
        ],
        ..Default::default()
    };
    let encoded = request_body(&request, "", &BTreeMap::new()).unwrap();
    assert!(encoded.windows(5).any(|w| w == b"1e999"));
    assert!(encoded.len() > 4 << 20);
}

#[test]
fn response_decodes_escaped_text_usage_and_retains_exact_opaque_reasoning() {
    let raw_reasoning = r#"{"id":"rs","type":"reasoning","summary":[],"vendor":1e999}"#;
    let body = format!(
        r#"{{"id":"r","status":"completed","output":[{{"id":"m","type":"message","phase":"final_answer","content":[{{"type":"output_text","text":"line\n中🎉"}}]}},{raw_reasoning}],"usage":{{"input_tokens":10,"input_tokens_details":{{"cached_tokens":4,"cache_write_tokens":2}},"output_tokens":8,"output_tokens_details":{{"reasoning_tokens":3}},"vendor":1e999}}}}"#
    );
    let response = decode_response(body.as_bytes()).unwrap();
    assert_eq!(response.stop, "complete");
    assert_eq!(
        (
            response.usage.input_tokens,
            response.usage.cached_input_tokens,
            response.usage.cache_write_input_tokens,
            response.usage.reasoning_tokens
        ),
        (10, 4, 2, 3)
    );
    let output = response.output.unwrap();
    assert!(
        matches!(&output[0].data, LlmData::Message(m) if m.text == "line\n中🎉" && m.phase == "final_answer")
    );
    assert!(
        matches!(&output[1].data, LlmData::Reasoning(r) if r.raw.as_str() == Some(raw_reasoning))
    );
}

#[test]
fn response_distinguishes_refusal_incomplete_failure_and_explicit_null_usage() {
    for (body, stop) in [
        (
            r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}"#,
            "max_output_tokens",
        ),
        (
            r#"{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}"#,
            "refused",
        ),
        (
            r#"{"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}]}"#,
            "complete",
        ),
    ] {
        assert_eq!(decode_response(body.as_bytes()).unwrap().stop, stop);
    }
    let failed =
        decode_response(br#"{"status":"failed","error":{"code":"server_error","message":"no"}}"#)
            .unwrap();
    assert_eq!(failed.failure.unwrap().code, "server_error");
    assert_eq!(
        decode_response(br#"{"status":"failed","error":null}"#)
            .unwrap()
            .failure
            .unwrap()
            .message,
        "response failed"
    );
    assert_eq!(
        decode_response(br#"{"status":"completed","usage":null}"#)
            .unwrap()
            .usage
            .raw
            .as_str(),
        Some("null")
    );
    assert!(
        decode_response(br#"{"status":"completed"}"#)
            .unwrap()
            .usage
            .raw
            .is_missing()
    );
    assert!(
        decode_response(b"{}")
            .unwrap_err()
            .0
            .contains("unsupported response status")
    );
    assert!(decode_response(br#"{"id":"a","id":"b"}"#).is_err());
}
