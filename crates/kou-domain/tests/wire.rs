use kou_domain::{Input, Json, Record, Timestamp, validate_json};

#[test]
fn input_validation_preserves_go_control_and_delivery_rules() {
    for (kind, payload, delivery, valid) in [
        ("external", "null", "", true),
        ("external", "{}", "after_tools", true),
        ("crash", "{}", "after_tools", false),
        ("control", r#"{"Mode":"hard","Reason":"stop"}"#, "", true),
        ("control", r#"{"Mode":"hard","Parameters":null}"#, "", false),
        ("control", r#"{"Mode":"heartbeat","Reason":""}"#, "", false),
        (
            "control",
            r#"{"Mode":"heartbeat","Reason":"tick"}"#,
            "",
            true,
        ),
        (
            "control",
            r#"{"Mode":"settings","Parameters":{"ReasoningEffort":"high"}}"#,
            "",
            true,
        ),
        (
            "control",
            r#"{"Mode":"settings","Parameters":{"ReasoningEffort":"invalid"}}"#,
            "",
            false,
        ),
        (
            "control",
            r#"{"Mode":"settings","Parameters":{"ReasoningEffort":"high","Unknown":1}}"#,
            "",
            false,
        ),
        ("control", r#"{"Mode":"hard","Unknown":1}"#, "", false),
        ("control", r#"{"Mode":"soft"}"#, "", false),
    ] {
        let input = Input {
            id: "id".into(),
            kind: kind.into(),
            payload: Json::parse(payload).unwrap(),
            delivery: delivery.into(),
        };
        assert_eq!(input.validate().is_ok(), valid, "{payload}");
    }
}

#[test]
fn omitted_and_explicit_empty_output_remain_distinct() {
    let prefix = r#"{"type":"item","data":{"Item":{"Sequence":1,"RecordedAt":"2026-01-01T00:00:00Z","Kind":"model_response","Data":{"TurnID":"turn","Response":RESPONSE}}}}"#;
    for response in [
        r#"{"ID":"r","Stop":"complete","Usage":{},"Failure":null}"#,
        r#"{"ID":"r","Stop":"complete","Output":[],"Usage":{},"Failure":null}"#,
    ] {
        let text = prefix.replace("RESPONSE", response);
        let encoded = serde_json::to_value(Record::decode(text.as_bytes()).unwrap()).unwrap();
        let result = &encoded["data"]["Item"]["Data"]["Response"];
        assert_eq!(result.get("Output").is_some(), response.contains("Output"));
    }
}

#[test]
fn explicit_null_raw_state_is_not_omitted() {
    let bytes = br#"{"type":"operation","data":{"Operation":{"ID":"o","Type":"test","Version":1,"Status":"ready","State":null}}}"#;
    let record = Record::decode(bytes).unwrap();
    let encoded = serde_json::to_value(record).unwrap();
    assert!(encoded["data"]["Operation"].get("State").is_some());
    assert!(encoded["data"]["Operation"].get("Idempotency").is_none());
}

#[test]
fn numeric_opaque_json_is_not_limited_to_f64() {
    assert!(
        validate_json(br#"{"provider":{"n":1e999,"integer":123456789012345678901234567890}}"#)
            .is_ok()
    );
}

#[test]
fn rejects_null_payload_for_tagged_items() {
    for body in [
        r#"{"Sequence":1,"Kind":"turn","Data":null}"#,
        r#"{"Sequence":1,"Kind":"turn"}"#,
    ] {
        let record = format!(r#"{{"type":"item","data":{{"Item":{body}}}}}"#);
        assert!(Record::decode(record.as_bytes()).is_err());
    }
}

#[test]
fn timestamp_null_maps_to_go_zero() {
    let at: Timestamp = serde_json::from_str("null").unwrap();
    assert!(at.is_zero());
}
