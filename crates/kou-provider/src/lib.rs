use kou_domain::{Json, LlmItem};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

pub mod anthropic;
pub mod clients;
pub mod codex;
pub mod responses;
pub mod sse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Model {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: String,
    pub max_output_tokens: Option<i64>,
    #[serde(deserialize_with = "null_default")]
    pub reasoning_effort: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Tool {
    #[serde(rename = "Type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    pub parameters: Option<BTreeMap<String, Json>>,
}

pub type Request = RequestData<Vec<LlmItem>>;
pub type SharedRequest = RequestData<Vec<std::sync::Arc<LlmItem>>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(
    default,
    rename_all = "PascalCase",
    bound(deserialize = "I: Deserialize<'de> + Default")
)]
pub struct RequestData<I> {
    #[serde(deserialize_with = "null_default")]
    pub model: Model,
    #[serde(deserialize_with = "null_default")]
    pub input: I,
    #[serde(deserialize_with = "null_default")]
    pub tools: Vec<Tool>,
}

/// Both legacy owned DTOs and engine snapshots use the same borrowed encoders.
/// Snapshotting a conversation copies Arc handles, not tool output bodies.
pub trait History: Send + Sync {
    fn iter(&self) -> impl ExactSizeIterator<Item = &LlmItem> + DoubleEndedIterator;
    fn len(&self) -> usize {
        self.iter().len()
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl History for Vec<LlmItem> {
    fn iter(&self) -> impl ExactSizeIterator<Item = &LlmItem> + DoubleEndedIterator {
        self.as_slice().iter()
    }
}
impl History for Vec<std::sync::Arc<LlmItem>> {
    fn iter(&self) -> impl ExactSizeIterator<Item = &LlmItem> + DoubleEndedIterator {
        self.as_slice().iter().map(std::sync::Arc::as_ref)
    }
}

fn null_default<'de, D, T>(de: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

// Serde structs also accept positional arrays; Go's JSON contracts do not.
fn object_or_null<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de> + Default> serde::de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object or null")
        }
        fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<T, E> {
            Ok(T::default())
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            map: A,
        ) -> std::result::Result<T, A::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_any(Object(std::marker::PhantomData))
}

fn optional_object<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de> + Default> serde::de::Visitor<'de> for Object<T> {
        type Value = Option<T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> std::result::Result<Self::Value, D::Error> {
            object_or_null(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(Object(std::marker::PhantomData))
}
