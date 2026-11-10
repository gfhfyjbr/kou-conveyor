use super::{Error, Result};
use kou_domain::{Image, Json};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "PascalCase")]
pub struct ExternalMessage {
    #[serde(deserialize_with = "zero")]
    pub text: String,
    #[serde(deserialize_with = "zero", skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    #[serde(deserialize_with = "zero", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<LinkedFile>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "PascalCase")]
pub struct LinkedFile {
    #[serde(deserialize_with = "zero")]
    pub path: String,
    #[serde(deserialize_with = "zero", skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_false")]
    pub directory: bool,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_false")]
    pub binary: bool,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_zero")]
    pub size: i64,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_zero")]
    pub lines: i64,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_zero")]
    pub from: i64,
    #[serde(deserialize_with = "zero", skip_serializing_if = "is_zero")]
    pub to: i64,
    #[serde(deserialize_with = "zero", skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(deserialize_with = "zero", skip_serializing_if = "String::is_empty")]
    pub error: String,
}
fn is_false(value: &bool) -> bool {
    !value
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
pub(super) fn zero<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    de: D,
) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

pub fn external_payload(message: &ExternalMessage) -> Result<Json> {
    let bytes = if message.images.is_empty() && message.files.is_empty() {
        serde_json::to_string(&message.text)
    } else {
        validate(message)?;
        serde_json::to_string(message)
    }
    .map_err(|error| Error(error.to_string()))?;
    Json::parse(&bytes).map_err(|error| Error(error.to_string()))
}
pub fn decode_message(payload: &Json) -> Result<ExternalMessage> {
    let text = payload.as_str().unwrap_or_default().trim();
    if text.is_empty() {
        return Err(Error(
            "the payload is neither text nor a message with images or files".into(),
        ));
    }
    let message: ExternalMessage = match text.as_bytes()[0] {
        b'"' => {
            return serde_json::from_str(text)
                .map(|text| ExternalMessage {
                    text,
                    ..ExternalMessage::default()
                })
                .map_err(|error| Error(error.to_string()));
        }
        b'{' => serde_json::from_str(text).map_err(|error| Error(error.to_string()))?,
        _ => {
            return Err(Error(
                "the payload is neither text nor a message with images or files".into(),
            ));
        }
    };
    if message.images.is_empty() && message.files.is_empty() {
        return Err(Error(
            "a message without images or files is a JSON string of its text".into(),
        ));
    }
    validate(&message)?;
    Ok(message)
}
fn validate(message: &ExternalMessage) -> Result<()> {
    for (index, image) in message.images.iter().enumerate() {
        if image.url.trim().is_empty() {
            return Err(Error(format!("image {} has no URL", index + 1)));
        }
    }
    for (index, file) in message.files.iter().enumerate() {
        if file.path.trim().is_empty() {
            return Err(Error(format!("linked file {} has no path", index + 1)));
        }
        if file.from < 0 || file.to < file.from || (file.from == 0) != (file.to == 0) {
            return Err(Error(format!(
                "linked file {} shows lines {} to {}",
                index + 1,
                file.from,
                file.to
            )));
        }
        if file.lines < 0 || file.size < 0 {
            return Err(Error(format!(
                "linked file {} has a negative size",
                index + 1
            )));
        }
    }
    Ok(())
}
