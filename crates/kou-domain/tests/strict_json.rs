use kou_domain::{Record, validate_json};

#[test]
fn deep_opaque_values_use_go_nesting_limit_not_the_rust_call_stack() {
    for depth in [127, 128, 1000, 9999, 10_000] {
        let value = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert!(validate_json(value.as_bytes()).is_ok(), "depth {depth}");
    }
    let too_deep = format!("{}0{}", "[".repeat(10_001), "]".repeat(10_001));
    assert!(validate_json(too_deep.as_bytes()).is_err());
    let opaque = format!("{}0{}", "[".repeat(2000), "]".repeat(2000));
    let record = format!(
        r#"{{"type":"session","data":{{"Version":2,"Session":{{"ID":"s","CreatedAt":"2026-01-01T00:00:00Z"}},"unknown":{opaque}}}}}"#
    );
    assert!(Record::decode(record.as_bytes()).is_ok());
}

#[test]
fn rejects_unpaired_surrogates_in_values_and_unknown_fields() {
    for value in [
        r#""\ud800""#,
        r#"{"unknown":"\udc00"}"#,
        r#"{"unknown":"\ud800x"}"#,
    ] {
        assert!(validate_json(value.as_bytes()).is_err());
    }
    assert!(validate_json(br#"{"x":"\ud83d\ude00"}"#).is_ok());
    assert!(validate_json(&[b'"', 0xff, b'"']).is_err());
}

#[test]
fn object_keys_only_not_braces_inside_strings_are_deduplicated() {
    for valid in [
        r#"{"x":{"same":1},"y":{"same":2}}"#,
        r#"{"x":"{\"a\":1,\"a\":2}","y":"[}"}"#,
        r#"{"":"\\\"","nested":[[],{"a":1}]}"#,
        r#"[{},[],1e999,-1e-999,true,false,null,"💻"]"#,
    ] {
        assert!(validate_json(valid.as_bytes()).is_ok(), "{valid}");
    }
    assert!(validate_json(br#"{"":"a","":"b"}"#).is_err());
}
