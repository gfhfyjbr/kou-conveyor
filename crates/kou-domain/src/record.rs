use crate::{
    Fork, Input, Json, LlmData, LlmItem, ModelResponse, Operation, Session, Timestamp,
    ToolCallStatus, Turn, null_default, validate_json,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};

pub const FORMAT_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub sequence: u64,
    pub recorded_at: Timestamp,
    pub data: ItemData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemData {
    Fork(Fork),
    Input(Input),
    Turn(Turn),
    ModelResponse(ModelResponse),
    ToolCallStatus(ToolCallStatus),
}

impl ItemData {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Fork(_) => "fork",
            Self::Input(_) => "input",
            Self::Turn(_) => "turn",
            Self::ModelResponse(_) => "model_response",
            Self::ToolCallStatus(_) => "tool_call_status",
        }
    }
}

impl Serialize for ItemData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Fork(v) => v.serialize(serializer),
            Self::Input(v) => v.serialize(serializer),
            Self::Turn(v) => v.serialize(serializer),
            Self::ModelResponse(v) => v.serialize(serializer),
            Self::ToolCallStatus(v) => v.serialize(serializer),
        }
    }
}

impl Serialize for Item {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Item", 4)?;
        out.serialize_field("Sequence", &self.sequence)?;
        out.serialize_field("RecordedAt", &self.recorded_at)?;
        out.serialize_field("Kind", self.data.kind())?;
        out.serialize_field("Data", &self.data)?;
        out.end()
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct WireItem {
    #[serde(deserialize_with = "null_default")]
    sequence: u64,
    recorded_at: Timestamp,
    #[serde(deserialize_with = "null_default")]
    kind: String,
    data: Json,
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = WireItem::deserialize(de)?;
        if value.data.is_missing() || value.data.as_str() == Some("null") {
            return Err(de::Error::custom("item data must not be null or missing"));
        }
        let data = match value.kind.as_str() {
            "fork" => ItemData::Fork(value.data.decode().map_err(de::Error::custom)?),
            "input" => {
                let input: Input = value.data.decode().map_err(de::Error::custom)?;
                input.validate().map_err(de::Error::custom)?;
                ItemData::Input(input)
            }
            "turn" => ItemData::Turn(value.data.decode().map_err(de::Error::custom)?),
            "model_response" => {
                ItemData::ModelResponse(value.data.decode().map_err(de::Error::custom)?)
            }
            "tool_call_status" => {
                ItemData::ToolCallStatus(value.data.decode().map_err(de::Error::custom)?)
            }
            _ => return Err(de::Error::custom("unsupported item kind")),
        };
        Ok(Self {
            sequence: value.sequence,
            recorded_at: value.recorded_at,
            data,
        })
    }
}

impl Serialize for LlmData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Message(v) => v.serialize(serializer),
            Self::ToolCall(v) => v.serialize(serializer),
            Self::ToolResult(v) => v.serialize(serializer),
            Self::Reasoning(v) => v.serialize(serializer),
        }
    }
}

impl Serialize for LlmItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Item", 4)?;
        out.serialize_field("ProviderID", &self.provider_id)?;
        out.serialize_field("Type", self.data.kind())?;
        out.serialize_field("Data", &self.data)?;
        if !self.model.is_empty() {
            out.serialize_field("Model", &self.model)?;
        }
        out.end()
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct WireLlmItem {
    #[serde(rename = "ProviderID", deserialize_with = "null_default")]
    provider_id: String,
    #[serde(rename = "Type", deserialize_with = "null_default")]
    kind: String,
    data: Json,
    #[serde(deserialize_with = "null_default")]
    model: String,
}

impl<'de> Deserialize<'de> for LlmItem {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = WireLlmItem::deserialize(de)?;
        if value.data.is_missing() || value.data.as_str() == Some("null") {
            return Err(de::Error::custom(
                "LLM item data must not be null or missing",
            ));
        }
        let data = match value.kind.as_str() {
            "message" => LlmData::Message(value.data.decode().map_err(de::Error::custom)?),
            "tool_call" => LlmData::ToolCall(value.data.decode().map_err(de::Error::custom)?),
            "tool_result" => LlmData::ToolResult(value.data.decode().map_err(de::Error::custom)?),
            "reasoning" => LlmData::Reasoning(value.data.decode().map_err(de::Error::custom)?),
            _ => return Err(de::Error::custom("unsupported LLM item type")),
        };
        Ok(Self {
            provider_id: value.provider_id,
            model: value.model,
            data,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SessionRecord {
    #[serde(deserialize_with = "null_default")]
    pub version: u32,
    #[serde(deserialize_with = "null_default")]
    pub session: Session,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemRecord {
    pub item: Item,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_default"
    )]
    pub operations: Vec<ItemOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOperation {
    Saved(String),
    Full(Operation),
}

impl Serialize for ItemOperation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Saved(id) => id.serialize(serializer),
            Self::Full(operation) => operation.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ItemOperation {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        // Serde's untagged buffering cannot preserve nested RawValue fields.
        let value = Json::deserialize(de)?;
        if value.as_str().is_some_and(|raw| raw.starts_with('"')) {
            value.decode().map(Self::Saved).map_err(de::Error::custom)
        } else {
            value.decode().map(Self::Full).map_err(de::Error::custom)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct OperationRecord {
    #[serde(deserialize_with = "null_default")]
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Session(SessionRecord),
    Item(ItemRecord),
    Operation(OperationRecord),
    Checkpoint(Json),
}

impl Record {
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        validate_json(bytes)?;
        serde_json::from_slice(bytes)
    }

    pub fn encode_line(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = Vec::new();
        crate::write_compact_json(&mut bytes, self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Item(_) => "item",
            Self::Operation(_) => "operation",
            Self::Checkpoint(_) => "checkpoint",
        }
    }
}

impl Serialize for Record {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Record", 2)?;
        out.serialize_field("type", self.kind())?;
        match self {
            Self::Session(v) => out.serialize_field("data", v)?,
            Self::Item(v) => out.serialize_field("data", v)?,
            Self::Operation(v) => out.serialize_field("data", v)?,
            Self::Checkpoint(v) => out.serialize_field("data", v)?,
        }
        out.end()
    }
}

impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Envelope {
            #[serde(rename = "type", deserialize_with = "null_default")]
            kind: String,
            data: Json,
        }
        let value = Envelope::deserialize(de)?;
        match value.kind.as_str() {
            "session" => Ok(Self::Session(
                value.data.decode().map_err(de::Error::custom)?,
            )),
            "item" => Ok(Self::Item(value.data.decode().map_err(de::Error::custom)?)),
            "operation" => Ok(Self::Operation(
                value.data.decode().map_err(de::Error::custom)?,
            )),
            "checkpoint" => Ok(Self::Checkpoint(value.data)),
            _ => Err(de::Error::custom("unsupported record type")),
        }
    }
}
