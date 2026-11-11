use super::*;
use crate::operation::{decode_skill, skill_spec};
use skills::quoted;
use std::borrow::Cow;

pub const SKILL_USE_NAME: &str = "SkillUse";

pub struct SkillUse {
    catalog: SkillCatalog,
}
impl SkillUse {
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
    fn select(&self, call_name: &str, arguments: &str) -> Result<Spec> {
        if !call_name.is_empty() && call_name != SKILL_USE_NAME {
            return Err(Error(format!(
                "skill-use call name {} does not match static tool \"SkillUse\"",
                quoted(call_name)
            )));
        }
        let text = if arguments.trim().is_empty() {
            "{}"
        } else {
            arguments
        };
        let name = skill_name(text)
            .map_err(|error| Error(format!("decode skill-use arguments: {error}")))?;
        let name = name.unwrap_or_default();
        if name.trim().is_empty() {
            return Err(Error("skill-use argument \"name\" must be set".into()));
        }
        let skill = self
            .catalog
            .resolve(&name)
            .ok_or_else(|| Error(format!("skill {} is not registered", quoted(&name))))?;
        skill_spec(skill.path.clone())
            .map_err(|error| Error(format!("build skill-use operation: {error}")))
    }
    fn call(
        &self,
        name: &str,
        arguments: &str,
        submit: &mut dyn FnMut(Spec) -> String,
    ) -> CallStatus {
        match self.select(name, arguments) {
            Ok(spec) => CallStatus {
                waiting_for: Some(vec![submit(spec)]),
                ..CallStatus::default()
            },
            Err(error) => CallStatus {
                error: error.to_string(),
                ..CallStatus::default()
            },
        }
    }
}
impl Translator for SkillUse {
    fn translate(&self, arguments: &str, submit: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        self.call("", arguments, submit)
    }
    fn translate_call(
        &self,
        call: &ToolCall,
        submit: &mut dyn FnMut(Spec) -> String,
    ) -> CallStatus {
        self.call(&call.name, &call.arguments, submit)
    }
    fn result(
        &self,
        call_id: &str,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<ToolResult> {
        if !status.error.is_empty() {
            return Ok(text_result(call_id, status.error.clone()));
        }
        let [current] = operations else {
            return Err(Error(format!(
                "skill-use call {} has {} operations, want 1",
                quoted(call_id),
                operations.len()
            )));
        };
        let state = decode_skill(current).map_err(|error| {
            Error(format!(
                "decode skill-use call {} result: {error}",
                quoted(call_id)
            ))
        })?;
        let text = match current.status.as_str() {
            "completed" => valid_text(state.content),
            "ready" | "awaiting" | "canceling" => "Skill is loading.".into(),
            "canceled" | "failed" => {
                if state.terminal_error.is_empty() {
                    return Err(Error(format!(
                        "skill-use call {} terminal operation {} has no error",
                        quoted(call_id),
                        quoted(&current.id)
                    )));
                }
                state.terminal_error
            }
            _ => {
                return Err(Error(format!(
                    "skill-use call {} operation {} has unsupported status {}",
                    quoted(call_id),
                    quoted(&current.id),
                    quoted(&current.status)
                )));
            }
        };
        Ok(text_result(call_id, text))
    }
}
fn skill_name(text: &str) -> Result<Option<Cow<'_, str>>> {
    use serde::{
        Deserializer,
        de::{MapAccess, Visitor},
    };
    use serde_json::value::RawValue;
    use std::{collections::HashSet, fmt};

    let syntax = || {
        arguments::object(text)
            .err()
            .unwrap_or_else(|| Error("invalid skill-use arguments".into()))
    };
    let root = text.trim();
    if root == "null" {
        return Ok(None);
    }
    if !root.starts_with('{') {
        if kou_domain::validate_json(text.as_bytes()).is_err() {
            return Err(syntax());
        }
        return Err(Error(type_error(root, "tool.skillUseArguments")));
    }
    struct Names<'a, 'e> {
        text: &'a str,
        diagnostic: &'e mut Option<Error>,
    }
    impl<'de> Visitor<'de> for Names<'de, '_> {
        type Value = Option<Cow<'de, str>>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("skill-use arguments")
        }
        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut names = HashSet::new();
            let mut name = None;
            while let Some(key) = map.next_key::<String>()? {
                let mut reject = |error: Error| {
                    *self.diagnostic = Some(error);
                    serde::de::Error::custom("invalid skill-use field")
                };
                if !names.insert(key.clone()) {
                    return Err(reject(Error(format!(
                        "jsontext: duplicate object member name {}",
                        quoted(&key)
                    ))));
                }
                let raw = map.next_value::<&RawValue>()?;
                if key == "name" && raw.get() != "null" && !raw.get().starts_with('"') {
                    return Err(reject(Error(format!(
                        "{} within \"/name\"",
                        type_error(raw.get(), "string")
                    ))));
                }
                if kou_domain::validate_json(raw.get().as_bytes()).is_err() {
                    return Err(reject(arguments::object(self.text).unwrap_err()));
                }
                if key == "name" {
                    name = arguments::borrowed_string(raw).map_err(reject)?;
                }
            }
            Ok(name)
        }
    }
    let mut diagnostic = None;
    let mut decoder = serde_json::Deserializer::from_str(text);
    let result = decoder.deserialize_map(Names {
        text,
        diagnostic: &mut diagnostic,
    });
    let name = result.map_err(|_| diagnostic.unwrap_or_else(syntax))?;
    decoder.end().map_err(|_| syntax())?;
    Ok(name)
}
fn valid_text(bytes: Vec<u8>) -> String {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return text,
        Err(error) => error.into_bytes(),
    };
    let mut rest = bytes.as_slice();
    let mut result = String::with_capacity(rest.len());
    let mut in_invalid = false;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                result.push_str(text);
                return result;
            }
            Err(error) => {
                let good = error.valid_up_to();
                result
                    .push_str(std::str::from_utf8(&rest[..good]).expect("validated UTF-8 prefix"));
                if good != 0 || !in_invalid {
                    result.push('\u{fffd}');
                }
                in_invalid = true;
                rest = &rest[good + 1..];
            }
        }
    }
}

#[cfg(test)]
mod tests;
