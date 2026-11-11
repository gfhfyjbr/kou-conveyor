use super::*;
use serde_json::value::RawValue;

/// What Go's `json.Marshal(registry.StaticDefinitions(), json.Deterministic(true))`
/// writes with every built-in tool enabled. `compat/registry_parity.py` checks
/// this copy against Go and rewrites it with `--update-golden`.
const GO_DEFINITIONS: &str = include_str!("static_definitions.json");

#[test]
fn built_in_definitions_are_the_text_go_encodes() {
    let encoded = serde_json::to_string(built_in().as_slice()).unwrap();
    assert_eq!(encoded, GO_DEFINITIONS.trim_end_matches('\n'));
}

#[test]
fn built_in_definitions_follow_the_static_names() {
    for (definition, name) in built_in().iter().zip(STATIC_NAMES) {
        assert_eq!(definition.tool.name, name);
        assert_eq!(definition.tool.kind, TOOL_FUNCTION);
        assert!(!definition.tool.description.is_empty());
        assert!(definition.metadata.is_missing());
    }
    assert_eq!(
        STATIC_NAMES,
        [
            "Bash",
            "Read",
            "Edit",
            "Write",
            "apply_patch",
            "ViewImage",
            "TranscriptSearch",
            "Code",
            "SkillUse"
        ]
    );
    assert_eq!(DEFAULT_SHELL_TIMEOUT, 600);
}

// Providers send the schema values as they are held: a name out of order or a
// space between tokens would change the cached prefix of every request.
#[test]
fn built_in_schemas_are_written_sorted_and_without_whitespace() {
    fn sorted(text: &str) {
        match text.as_bytes().first() {
            Some(b'{') => {
                let members = arguments::ordered(text).unwrap();
                assert!(
                    members.windows(2).all(|pair| pair[0].0 < pair[1].0),
                    "{text}"
                );
                members.iter().for_each(|(_, value)| sorted(value.get()));
            }
            Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(text)
                .unwrap()
                .iter()
                .for_each(|value| sorted(value.get())),
            _ => {}
        }
    }
    for definition in built_in() {
        let parameters = definition.tool.parameters.as_ref().unwrap();
        assert_eq!(
            parameters.keys().collect::<Vec<_>>(),
            ["properties", "required", "type"]
        );
        for value in parameters.values() {
            let text = value.as_str().unwrap();
            sorted(text);
            let mut compact = Vec::new();
            kou_domain::write_compact_json(&mut compact, value).unwrap();
            assert_eq!(compact, text.as_bytes());
        }
    }
}

#[test]
fn definitions_encode_as_go_does_and_keep_schema_values_as_written() {
    let tool = Tool {
        kind: TOOL_FUNCTION.into(),
        name: "GitGlance".into(),
        ..Tool::default()
    };
    let definition = Definition {
        tool,
        ..Definition::default()
    };
    // Go writes a nil schema map as an empty object and absent metadata as null.
    assert_eq!(
        serde_json::to_string(&definition).unwrap(),
        r#"{"Tool":{"Type":"function","Name":"GitGlance","Description":"","Parameters":{}},"Metadata":null}"#
    );
    let decoded: Definition =
        serde_json::from_str(r#"{"Tool":{"Name":null,"Parameters":null},"Metadata":{"k": 1}}"#)
            .unwrap();
    assert_eq!(decoded.tool, Tool::default());
    assert_eq!(decoded.metadata.as_str(), Some(r#"{"k": 1}"#));
    let text = r#"{"Type":"hosted","Name":"web_search","Description":"d","Parameters":{"type":"object","a":{"z":1,"b":[2, 3]}}}"#;
    let decoded: Tool = serde_json::from_str(text).unwrap();
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        r#"{"Type":"hosted","Name":"web_search","Description":"d","Parameters":{"a":{"z":1,"b":[2, 3]},"type":"object"}}"#
    );
}
