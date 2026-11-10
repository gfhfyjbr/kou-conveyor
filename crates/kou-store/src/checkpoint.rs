use crate::{Error, Result};
use kou_domain::{Json, Snapshot};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const VERSION: u32 = 1;

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct Envelope {
    pub(crate) version: u32,
    pub(crate) offset: u64,
    #[serde(rename = "SHA256")]
    pub(crate) sha256: String,
    pub(crate) data: Json,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct Data {
    pub(crate) head: CheckpointHead,
    pub(crate) state: Json,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct CheckpointHead {
    pub(crate) snapshot: Snapshot,
    pub(crate) sequence: u64,
    pub(crate) latest_turn: String,
    #[serde(deserialize_with = "null_default")]
    pub(crate) turns: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub(crate) owned_turns: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub(crate) responded_turns: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub(crate) calls: Vec<Call>,
    #[serde(deserialize_with = "null_default")]
    pub(crate) operations: Vec<CheckpointOperation>,
    #[serde(rename = "ExternalInputIDs", deserialize_with = "null_default")]
    pub(crate) external_input_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Call {
    pub(crate) turn: String,
    pub(crate) call: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CheckpointOperation {
    pub(crate) operation: kou_domain::Operation,
    pub(crate) offset: u64,
    pub(crate) archived: bool,
    pub(crate) pending: bool,
}

fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    de: D,
) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

pub(crate) fn encode(data: Data, offset: u64) -> Result<Json> {
    let mut bytes = Vec::new();
    kou_domain::write_compact_json(&mut bytes, &data).map_err(|e| Error::Invalid(e.to_string()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let data = Json::parse(std::str::from_utf8(&bytes).map_err(|e| Error::Invalid(e.to_string()))?)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let envelope = Envelope {
        version: VERSION,
        offset,
        sha256,
        data,
    };
    let bytes = serde_json::to_string(&envelope).map_err(|e| Error::Invalid(e.to_string()))?;
    Json::parse(&bytes).map_err(|e| Error::Invalid(e.to_string()))
}

pub(crate) fn decode(raw: Json, offset: u64) -> Option<Data> {
    let envelope: Envelope = raw.decode().ok()?;
    if envelope.version != VERSION || envelope.offset != offset {
        return None;
    }
    let bytes = envelope.data.as_str()?;
    if envelope.sha256 != format!("{:x}", Sha256::digest(bytes.as_bytes())) {
        return None;
    }
    let value: Data = envelope.data.decode().ok()?;
    if value.state.is_missing() {
        return None;
    }
    Some(value)
}
