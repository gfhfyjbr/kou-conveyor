use crate::{Error, Result};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::borrow::Cow;

pub(crate) enum Marker {
    Session,
    Item { fork: bool },
    Operation,
    Checkpoint,
}

/// The first pass only locates the inherited prefix. It must not decode or
/// allocate old tool bodies; full strict validation happens in replay.
pub(crate) fn marker(bytes: &[u8]) -> Result<Marker> {
    #[derive(Deserialize)]
    struct Envelope<'a> {
        #[serde(rename = "type", borrow)]
        kind: Cow<'a, str>,
        #[serde(default, borrow)]
        data: Option<&'a RawValue>,
    }
    #[derive(Default, Deserialize)]
    struct ItemEnvelope<'a> {
        #[serde(rename = "Item", default, borrow)]
        item: Option<ItemKind<'a>>,
    }
    #[derive(Deserialize)]
    struct ItemKind<'a> {
        #[serde(rename = "Kind", default, borrow)]
        kind: Option<Cow<'a, str>>,
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|source| Error::Decode { line: 0, source })?;
    Ok(match envelope.kind.as_ref() {
        "session" => Marker::Session,
        "item" => {
            let data = envelope
                .data
                .map(|raw| serde_json::from_str::<Option<ItemEnvelope<'_>>>(raw.get()))
                .transpose()
                .map_err(|source| Error::Decode { line: 0, source })?
                .flatten();
            Marker::Item {
                fork: data
                    .and_then(|data| data.item)
                    .and_then(|item| item.kind)
                    .as_deref()
                    == Some("fork"),
            }
        }
        "operation" => Marker::Operation,
        "checkpoint" => Marker::Checkpoint,
        _ => return Err(Error::Invalid("unknown record type".into())),
    })
}
