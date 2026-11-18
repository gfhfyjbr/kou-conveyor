//! MCP elicitation forms as conversation questions and typed answers.

use crate::{Question, QuestionOption};
use serde_json::{Map, Number, Value, json};

/// One field of an MCP server's form.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Its key in the content sent back.
    key: String,
    /// What it is called in a notice about it.
    name: String,
    kind: FieldKind,
    required: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum FieldKind {
    Text,
    Number,
    Integer,
    Boolean,
    /// One of these: what is shown, and the value sent.
    One(Vec<(String, Value)>),
    /// Any of these.
    Many(Vec<(String, Value)>),
}

/// The answers of a boolean field.
const YES: &str = "Yes";
const NO: &str = "No";

/// The fields of an MCP server's form (`requested_schema`), in the order
/// the server listed them (`property_order`, else the schema's own), and
/// each as a question. A field of a kind MCP's forms do not have (an
/// object, say) is left out: nothing could answer it.
pub fn form(schema: &Value, order: Option<&Value>) -> Vec<(Field, Question)> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut keys: Vec<&String> = order
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|key| properties.get_key_value(key).map(|(key, _)| key))
        .collect();
    for key in properties.keys() {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.into_iter()
        .filter_map(|key| {
            let property = &properties[key];
            let kind = field_kind(property)?;
            let text = |name: &str| {
                property
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            };
            let (title, description) = (text("title"), text("description"));
            let name = title.clone().or(description.clone()).unwrap_or(key.clone());
            let (header, text) = match (title, description) {
                (Some(title), Some(description)) => (Some(title), description),
                (title, description) => (None, title.or(description).unwrap_or(key.clone())),
            };
            let choices = |list: &[(String, Value)]| {
                list.iter()
                    .map(|(label, _)| QuestionOption {
                        label: label.clone(),
                        description: None,
                    })
                    .collect()
            };
            let options = match &kind {
                FieldKind::One(list) | FieldKind::Many(list) => choices(list),
                FieldKind::Boolean => [YES, NO]
                    .map(|label| QuestionOption {
                        label: label.into(),
                        description: None,
                    })
                    .to_vec(),
                _ => Vec::new(),
            };
            let question = Question {
                header,
                text,
                options,
                multiple: matches!(kind, FieldKind::Many(_)),
                free: matches!(
                    kind,
                    FieldKind::Text | FieldKind::Number | FieldKind::Integer
                ),
            };
            let field = Field {
                key: key.clone(),
                name,
                kind,
                required: required.contains(&key.as_str()),
            };
            Some((field, question))
        })
        .collect()
}

/// What a field of a form's schema takes: MCP's primitive fields, a choice
/// of values (`enum`, with `enumNames`; `oneOf` of `const` and `title`) or
/// several of them (an array whose `items` are such a choice).
fn field_kind(property: &Value) -> Option<FieldKind> {
    let kind = property.get("type").and_then(Value::as_str);
    if kind == Some("array") {
        let items = property.get("items")?;
        return Some(FieldKind::Many(choices(items, "anyOf")?));
    }
    if let Some(list) = choices(property, "oneOf") {
        return Some(FieldKind::One(list));
    }
    match kind {
        Some("string") | None => Some(FieldKind::Text),
        Some("number") => Some(FieldKind::Number),
        Some("integer") => Some(FieldKind::Integer),
        Some("boolean") => Some(FieldKind::Boolean),
        _ => None,
    }
}

/// The values a field offers to choose from, each with what is shown for
/// it: its `enum` (named by `enumNames` where given), or its list `titled`
/// of `const` and `title`.
fn choices(property: &Value, titled: &str) -> Option<Vec<(String, Value)>> {
    if let Some(values) = property.get("enum").and_then(Value::as_array) {
        let names = property.get("enumNames").and_then(Value::as_array);
        let list: Vec<(String, Value)> = values
            .iter()
            .enumerate()
            .map(|(at, value)| {
                let name = names
                    .and_then(|names| names.get(at))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| shown(value));
                (name, value.clone())
            })
            .collect();
        return (!list.is_empty()).then_some(list);
    }
    let list: Vec<(String, Value)> = property
        .get(titled)
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|choice| {
            let value = choice.get("const")?.clone();
            let name = choice
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| shown(&value));
            Some((name, value))
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

/// A value as it is shown: a string as itself.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The content of an accepted form: each field answered, typed as its
/// schema says; a field left unanswered is left out. An error says, for the
/// user, which answer does not fit its field.
pub fn form_content(fields: &[Field], answers: &[Vec<String>]) -> Result<Value, String> {
    let mut content = Map::new();
    for (at, field) in fields.iter().enumerate() {
        let items: Vec<&str> = answers
            .get(at)
            .into_iter()
            .flatten()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .collect();
        let name = &field.name;
        if items.is_empty() {
            if field.required {
                return Err(format!("\"{name}\" needs an answer."));
            }
            continue;
        }
        let one = || match items.as_slice() {
            [item] => Ok(*item),
            _ => Err(format!("\"{name}\" takes one answer.")),
        };
        let pick = |list: &[(String, Value)], item: &str| {
            list.iter()
                .find(|(label, _)| label == item)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| {
                    let labels: Vec<&str> = list.iter().map(|(label, _)| label.as_str()).collect();
                    format!("\"{name}\" takes one of: {}.", labels.join(", "))
                })
        };
        let value = match &field.kind {
            FieldKind::Text => json!(items.join(", ")),
            FieldKind::Number => {
                let item = one()?;
                item.parse::<f64>()
                    .ok()
                    .and_then(Number::from_f64)
                    .map(Value::Number)
                    .ok_or_else(|| format!("\"{name}\" takes a number, not \"{item}\"."))?
            }
            FieldKind::Integer => {
                let item = one()?;
                item.parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| format!("\"{name}\" takes a whole number, not \"{item}\"."))?
            }
            FieldKind::Boolean => match one()? {
                YES => Value::Bool(true),
                NO => Value::Bool(false),
                item => return Err(format!("\"{name}\" takes Yes or No, not \"{item}\".")),
            },
            FieldKind::One(list) => pick(list, one()?)?,
            FieldKind::Many(list) => Value::Array(
                items
                    .iter()
                    .map(|item| pick(list, item))
                    .collect::<Result<_, _>>()?,
            ),
        };
        content.insert(field.key.clone(), value);
    }
    Ok(Value::Object(content))
}

/// The answers of an accepted form in words, a line each, for its card.
pub fn form_summary(fields: &[Field], answers: &[Vec<String>]) -> String {
    fields
        .iter()
        .zip(answers)
        .filter(|(_, items)| items.iter().any(|item| !item.trim().is_empty()))
        .map(|(field, items)| format!("{}: {}", field.name, items.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "color": {"type": "string", "title": "Color", "description": "The color to paint it", "enum": ["red", "green"], "enumNames": ["Red", "Green"]},
                "size": {"type": "string", "title": "Size", "oneOf": [{"const": "s", "title": "Small"}, {"const": "l", "title": "Large"}]},
                "count": {"type": "integer", "title": "How many"},
                "ratio": {"type": "number", "description": "What share"},
                "sure": {"type": "boolean", "title": "Proceed?"},
                "tags": {"type": "array", "items": {"anyOf": [{"const": "a", "title": "A"}, {"const": "b", "title": "B"}]}},
                "name": {"type": "string"},
                "nested": {"type": "object", "properties": {}},
            },
            "required": ["count"],
        })
    }

    #[test]
    fn a_form_s_fields_are_asked_in_the_server_s_order_each_as_a_question() {
        let order = json!(["name", "sure", "nope"]);
        let form = form(&schema(), Some(&order));
        let keys: Vec<&str> = form.iter().map(|(field, _)| field.key.as_str()).collect();
        assert_eq!(
            keys,
            ["name", "sure", "color", "size", "count", "ratio", "tags"]
        );
        let question = |key: &str| {
            form.iter()
                .find(|(field, _)| field.key == key)
                .map(|(_, question)| question.clone())
                .unwrap()
        };
        let labels = |question: &Question| -> Vec<String> {
            question
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect()
        };
        let color = question("color");
        assert_eq!(color.header.as_deref(), Some("Color"));
        assert_eq!(color.text, "The color to paint it");
        assert_eq!(labels(&color), ["Red", "Green"]);
        assert!(!color.free && !color.multiple);
        assert_eq!(labels(&question("size")), ["Small", "Large"]);
        assert_eq!(labels(&question("sure")), ["Yes", "No"]);
        let count = question("count");
        assert_eq!((count.header, count.text.as_str()), (None, "How many"));
        assert!(count.free && count.options.is_empty());
        assert_eq!(question("ratio").text, "What share");
        assert_eq!(question("name").text, "name");
        assert!(question("tags").multiple);
    }

    #[test]
    fn a_form_s_answers_are_typed_as_its_schema_says() {
        let fields: Vec<Field> = form(&schema(), None)
            .into_iter()
            .map(|(field, _)| field)
            .collect();
        let answers = |items: [&[&str]; 7]| -> Vec<Vec<String>> {
            items
                .iter()
                .map(|items| items.iter().map(|item| (*item).to_owned()).collect())
                .collect()
        };
        // color, size, count, ratio, sure, tags, name
        let content = form_content(
            &fields,
            &answers([
                &["Green"],
                &["Large"],
                &["3"],
                &["0.5"],
                &["No"],
                &["A", "B"],
                &[],
            ]),
        );
        assert_eq!(
            content,
            Ok(
                json!({"color": "green", "size": "l", "count": 3, "ratio": 0.5, "sure": false, "tags": ["a", "b"]})
            )
        );
        let refused = |items| form_content(&fields, &answers(items)).unwrap_err();
        assert_eq!(
            refused([&[], &[], &["three"], &[], &[], &[], &[]]),
            r#""How many" takes a whole number, not "three"."#
        );
        assert_eq!(
            refused([&[], &[], &[], &[], &[], &[], &[]]),
            r#""How many" needs an answer."#
        );
        assert_eq!(
            refused([&["Blue"], &[], &["1"], &[], &[], &[], &[]]),
            r#""Color" takes one of: Red, Green."#
        );
        assert_eq!(
            refused([&[], &[], &["1"], &["half"], &[], &[], &[]]),
            r#""What share" takes a number, not "half"."#
        );
        assert_eq!(
            form_summary(
                &fields,
                &answers([&["Green"], &[], &["3"], &[], &[], &[], &[]])
            ),
            "Color: Green\nHow many: 3"
        );
    }
}
