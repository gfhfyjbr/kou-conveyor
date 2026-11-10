use super::*;

#[derive(Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
struct ValueState {
    value: Json,
}

pub fn value_spec(value: Json) -> Result<Spec> {
    if value.is_missing() {
        return Err(Error("value operation result must be valid JSON".into()));
    }
    Ok(Spec {
        kind: "value".into(),
        version: 1,
        state: encode(&ValueState { value })?,
        idempotency: Json::default(),
    })
}

pub fn decode_value(current: &Operation) -> Result<Json> {
    if current.kind != "value" {
        return Err(Error(format!(
            "decode value operation {:?}: type {:?}: unsupported operation",
            current.id, current.kind
        )));
    }
    if current.version != 1 {
        return Err(Error(format!(
            "decode value operation {:?}: version {}: unsupported operation",
            current.id, current.version
        )));
    }
    let state: ValueState = decode_state(&current.state, "ValueState").map_err(|error| {
        Error(format!(
            "decode value operation {:?} state: {error}",
            current.id
        ))
    })?;
    if state.value.is_missing() {
        return Err(Error(format!(
            "decode value operation {:?} state: result must be valid JSON",
            current.id
        )));
    }
    Ok(state.value)
}

pub fn advance_value(mut current: Operation, event: Option<Event>) -> Result<Step> {
    decode_value(&current)?;
    if event.is_some() {
        return Err(Error(format!(
            "advance value operation {:?}: unexpected primitive event",
            current.id
        )));
    }
    current.status = match current.status.as_str() {
        "ready" => "completed".into(),
        "canceling" => "canceled".into(),
        _ => {
            return Err(Error(format!(
                "advance value operation {:?}: status {:?}: unsupported operation",
                current.id, current.status
            )));
        }
    };
    Ok(Step {
        operation: current,
        dispatches: Vec::new(),
    })
}
