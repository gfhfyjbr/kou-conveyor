use super::*;
use serde_json::value::RawValue;

#[cfg(test)]
mod tests;

const CORRELATION: &str = "skill-use-read";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SkillState {
    #[serde(deserialize_with = "null_default")]
    pub path: String,
    #[serde(with = "super::shell::wire::bytes")]
    pub content: Vec<u8>,
    #[serde(deserialize_with = "null_default")]
    pub terminal_error: String,
}

#[derive(Debug, Default)]
pub struct SkillEvent {
    pub kind: String,
    pub source: String,
    pub correlation_id: String,
    pub result: SkillEventResult,
}
#[derive(Debug, Default)]
pub enum SkillEventResult {
    #[default]
    Empty,
    Output {
        offset: i64,
        data: Vec<u8>,
    },
    Completed {
        size: i64,
    },
    Failure(String),
    Raw(Json),
}
#[derive(Debug, Default)]
pub struct SkillStep {
    pub operation: Option<Operation>,
    pub dispatches: Vec<shell::ReadRequest>,
}

pub fn skill_spec(path: String) -> Result<Spec> {
    let state = SkillState {
        path,
        ..SkillState::default()
    };
    validate(&state)?;
    Ok(Spec {
        kind: "skill_use".into(),
        version: 1,
        state: encode(&state)
            .map_err(|error| Error(format!("encode skill-use operation state: {error}")))?,
        max_output_length: 0,
        idempotency: Json::default(),
    })
}
pub fn decode_skill(current: &Operation) -> Result<SkillState> {
    if current.kind != "skill_use" {
        return Err(Error(format!(
            "decode skill-use operation {:?}: type {:?}: unsupported operation",
            current.id, current.kind
        )));
    }
    if current.version != 1 {
        return Err(Error(format!(
            "decode skill-use operation {:?}: version {}: unsupported operation",
            current.id, current.version
        )));
    }
    let state = decode_state(&current.state, "SkillUseState").map_err(|error| {
        Error(format!(
            "decode skill-use operation {:?} state: {}",
            current.id,
            wire_error(&current.state).unwrap_or(error.0)
        ))
    })?;
    validate(&state).map_err(|error| {
        Error(format!(
            "validate skill-use operation {:?} state: {error}",
            current.id
        ))
    })?;
    Ok(state)
}
fn validate(state: &SkillState) -> Result<()> {
    if state.path.is_empty() {
        Err(Error("skill path must be set".into()))
    } else {
        Ok(())
    }
}
pub fn advance_skill(current: Operation, event: Option<SkillEvent>) -> Result<SkillStep> {
    let state = decode_skill(&current)?;
    match current.status.as_str() {
        "ready" => {
            if event.is_some() {
                return Err(Error(format!(
                    "advance ready skill-use operation {:?}: unexpected primitive event",
                    current.id
                )));
            }
            if !state.content.is_empty() || !state.terminal_error.is_empty() {
                return Err(Error(format!(
                    "advance ready skill-use operation {:?}: state is not initial",
                    current.id
                )));
            }
            await_read(current, state)
        }
        "awaiting" => {
            if !state.terminal_error.is_empty() {
                return Err(Error(format!(
                    "advance awaiting skill-use operation {:?}: state is terminal",
                    current.id
                )));
            }
            match event {
                Some(event) => awaiting_event(current, state, event),
                None => await_read(current, state),
            }
        }
        "canceling" => {
            if event.is_some() {
                return Err(Error(format!(
                    "advance canceling skill-use operation {:?}: unexpected primitive event",
                    current.id
                )));
            }
            terminal(current, state, true, "skill-use operation canceled".into())
        }
        _ => Err(Error(format!(
            "advance skill-use operation {:?}: terminal status {:?}",
            current.id, current.status
        ))),
    }
}
fn awaiting_event(
    mut current: Operation,
    mut state: SkillState,
    event: SkillEvent,
) -> Result<SkillStep> {
    if event.source != current.id {
        let error = format!(
            "skill-use primitive event source is {:?}, want {:?}",
            event.source, current.id
        );
        return terminal(current, state, false, error);
    }
    if event.kind == "primitive.canceled" {
        return terminal(current, state, true, "skill-use operation canceled".into());
    }
    if event.kind == "primitive.failed" {
        let error = match event.result {
            SkillEventResult::Failure(error) => error,
            _ => "skill-use primitive failed with an invalid result".into(),
        };
        return terminal(current, state, false, error);
    }
    if event.correlation_id != CORRELATION {
        return terminal(
            current,
            state,
            false,
            format!(
                "skill-use primitive event correlation is {:?}, want {CORRELATION:?}",
                event.correlation_id
            ),
        );
    }
    match event.kind.as_str() {
        "io.read_output" => {
            let SkillEventResult::Output { offset, data } = event.result else {
                return terminal(
                    current,
                    state,
                    false,
                    "skill read returned invalid output".into(),
                );
            };
            if offset != state.content.len() as i64 {
                return terminal(
                    current,
                    state,
                    false,
                    "skill read returned invalid output".into(),
                );
            }
            if state.content.is_empty() {
                state.content = data;
            } else {
                state.content.extend_from_slice(&data);
            }
            checkpoint(current, state)
        }
        "io.read_completed" => {
            let SkillEventResult::Completed { size } = event.result else {
                return terminal(
                    current,
                    state,
                    false,
                    "skill read returned an invalid completion".into(),
                );
            };
            if size != state.content.len() as i64 {
                return terminal(
                    current,
                    state,
                    false,
                    "skill read returned an invalid completion".into(),
                );
            }
            current.status = "completed".into();
            checkpoint(current, state)
        }
        _ => terminal(
            current,
            state,
            false,
            format!("skill read returned unexpected event {:?}", event.kind),
        ),
    }
}
fn await_read(mut current: Operation, state: SkillState) -> Result<SkillStep> {
    let request = shell::ReadRequest {
        source: current.id.clone(),
        correlation_id: CORRELATION.into(),
        path: state.path.clone(),
        offset: state.content.len() as i64,
        count: i64::MAX,
    };
    current.status = "awaiting".into();
    let mut step = checkpoint(current, state)?;
    step.dispatches.push(request);
    Ok(step)
}
fn terminal(
    mut current: Operation,
    mut state: SkillState,
    canceled: bool,
    error: String,
) -> Result<SkillStep> {
    state.terminal_error = error;
    current.status = if canceled { "canceled" } else { "failed" }.into();
    checkpoint(current, state)
}
fn checkpoint(mut current: Operation, state: SkillState) -> Result<SkillStep> {
    current.state = encode(&state).map_err(|error| {
        Error(format!(
            "encode skill-use operation {:?} state: {error}",
            current.id
        ))
    })?;
    Ok(SkillStep {
        operation: Some(current),
        dispatches: Vec::new(),
    })
}

// Failure-only ordered scan: successful decoding does not walk or copy state twice.
fn wire_error(raw: &Json) -> Option<String> {
    struct Fields;
    impl<'de> serde::de::Visitor<'de> for Fields {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("skill-use state object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut found = None;
            let mut failed = false;
            // Returning early leaves unread fields and makes serde_json reject this visitor.
            while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                if failed {
                    continue;
                }
                let text = value.get();
                let kind = match text.as_bytes()[0] {
                    b'n' => continue,
                    b'[' => "array",
                    b'{' => "object",
                    b'\"' => "string",
                    b't' | b'f' => "boolean",
                    _ => "number",
                };
                let target = match key.as_str() {
                    "Path" | "TerminalError" if kind != "string" => "string",
                    "Content" if kind != "string" => "[]uint8",
                    "Content" => {
                        let value: String =
                            serde_json::from_str(text).map_err(serde::de::Error::custom)?;
                        if let Err(error) = shell::wire::bytes::decode(&value) {
                            failed = true;
                            found = error.starts_with("illegal character ").then(|| format!(
                                "json: cannot unmarshal JSON string into Go []uint8 within \"/Content\": {error}"
                            ));
                        }
                        continue;
                    }
                    _ => continue,
                };
                failed = true;
                found = Some(format!(
                    "json: cannot unmarshal JSON {kind} into Go {target} within \"/{key}\""
                ));
            }
            Ok(found)
        }
    }
    serde::Deserializer::deserialize_map(
        &mut serde_json::Deserializer::from_str(raw.as_str()?),
        Fields,
    )
    .ok()
    .flatten()
}
