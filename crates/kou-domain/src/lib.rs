mod json;
mod model;
mod printable;
mod quote;
mod record;
mod time;

pub use json::{Json, validate_json, write_compact_json};
pub use model::*;
pub use quote::{go_quote, go_quote_bytes};
pub use record::*;
pub use time::Timestamp;

pub(crate) fn null_default<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(de)?.unwrap_or_default())
}
