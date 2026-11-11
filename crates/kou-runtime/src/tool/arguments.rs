use super::*;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt,
};

pub(super) fn object(text: &str) -> Result<HashMap<String, &RawValue>> {
    if let Err(error) = kou_domain::validate_json(text.as_bytes()) {
        return Err(Error(
            syntax_error(text).unwrap_or_else(|| format!("jsontext: {error}")),
        ));
    }
    let text = text.trim();
    if text == "null" {
        return Ok(HashMap::new());
    }
    if !text.starts_with('{') {
        return Err(Error(type_error(text, "map[string]jsontext.Value")));
    }
    serde_json::from_str(text).map_err(|error| Error(error.to_string()))
}
pub(super) fn ordered(text: &str) -> Result<Vec<(String, &RawValue)>> {
    serde_json::from_str::<Members<'_>>(text)
        .map(|members| members.0)
        .map_err(|error| Error(error.to_string()))
}
pub(super) fn string(raw: &RawValue) -> Result<Option<String>> {
    borrowed_string(raw).map(|value| value.map(Cow::into_owned))
}
pub(super) fn borrowed_string(raw: &RawValue) -> Result<Option<Cow<'_, str>>> {
    if raw.get() == "null" {
        return Ok(None);
    }
    if !raw.get().starts_with('"') {
        return Err(Error(type_error(raw.get(), "string")));
    }
    struct Text;
    impl<'de> Visitor<'de> for Text {
        type Value = Cow<'de, str>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string")
        }
        fn visit_borrowed_str<E: serde::de::Error>(
            self,
            value: &'de str,
        ) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Borrowed(value))
        }
        fn visit_str<E: serde::de::Error>(
            self,
            value: &str,
        ) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Owned(value.into()))
        }
        fn visit_string<E: serde::de::Error>(
            self,
            value: String,
        ) -> std::result::Result<Self::Value, E> {
            Ok(Cow::Owned(value))
        }
    }
    serde_json::Deserializer::from_str(raw.get())
        .deserialize_str(Text)
        .map(Some)
        .map_err(|error| Error(error.to_string()))
}
pub(super) fn output_limit(values: &HashMap<String, &RawValue>) -> Result<usize> {
    parse_max_output_length(values.get("max_output_length").copied())
}
fn syntax_error(text: &str) -> Option<String> {
    if let Some(error) = duplicate(text) {
        return Some(error);
    }
    let error = serde_json::from_str::<&RawValue>(text).err()?;
    if error.is_eof() {
        return Some(if text.is_empty() {
            "jsontext: unexpected EOF".into()
        } else {
            format!("jsontext: unexpected EOF after offset {}", text.len())
        });
    }
    if error.to_string().starts_with("trailing characters") {
        let before = text
            .split_inclusive('\n')
            .take(error.line().saturating_sub(1))
            .map(str::len)
            .sum::<usize>();
        let offset = before + error.column().saturating_sub(1);
        let character = text.get(offset..)?.chars().next()?;
        return Some(format!(
            "jsontext: invalid character {character:?} after top-level value after offset {offset}"
        ));
    }
    None
}

struct Members<'a>(Vec<(String, &'a RawValue)>);
impl<'de> Deserialize<'de> for Members<'de> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = Members<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut values = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    values.push(entry);
                }
                Ok(Members(values))
            }
        }
        de.deserialize_map(Entries)
    }
}
// Failure-only walk: retain borrowed raw values rather than a complete JSON tree.
// Choosing the earliest raw-value offset preserves nested-before-parent failures.
fn duplicate(text: &str) -> Option<String> {
    let root: &RawValue = serde_json::from_str(text).ok()?;
    let mut stack = vec![(root, String::new())];
    let mut found: Option<(usize, String, String)> = None;
    while let Some((value, pointer)) = stack.pop() {
        match value.get().as_bytes().first()? {
            b'{' => {
                let Members(entries) = serde_json::from_str(value.get()).ok()?;
                let mut names = HashSet::new();
                for (name, value) in entries {
                    if !names.insert(name.clone()) {
                        let offset = value.get().as_ptr() as usize;
                        if found.as_ref().is_none_or(|previous| offset < previous.0) {
                            found = Some((offset, name.clone(), pointer.clone()));
                        }
                    }
                    let name = name.replace('~', "~0").replace('/', "~1");
                    stack.push((value, format!("{pointer}/{name}")));
                }
            }
            b'[' => {
                let entries: Vec<&RawValue> = serde_json::from_str(value.get()).ok()?;
                for (index, value) in entries.into_iter().enumerate() {
                    stack.push((value, format!("{pointer}/{index}")));
                }
            }
            _ => {}
        }
    }
    let (_, name, pointer) = found?;
    Some(if pointer.is_empty() {
        format!("jsontext: duplicate object member name {name:?}")
    } else {
        format!("jsontext: duplicate object member name {name:?} within {pointer:?}")
    })
}
