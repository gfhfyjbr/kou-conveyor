mod fingerprint;
mod policy;
mod request;
mod response;
mod stream;
mod transport;

pub use policy::{ApiError, RespondError};
pub use stream::{StreamDecoder, StreamLimits};
pub use transport::{CancellationToken, Client, Config, Limits, Reporter, RequestOptions};

pub use request::{
    AttemptOptions, ModelFeatures, RequestConfig, WireRequest, context_window, describe_model,
    prepare,
};
pub use response::decode_response;

use kou_domain::{Json, Usage};
use serde::{Deserialize, Serialize};

pub const BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-opus-5";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefix {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tools: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub breakpoints: Vec<Breakpoint>,
    #[serde(skip_serializing_if = "Json::is_missing")]
    pub edits: Json,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub message: i64,
    pub ttl: String,
}
impl Breakpoint {
    fn new(message: i64, ttl: &str) -> Self {
        Self {
            message,
            ttl: if ttl.is_empty() { "5m" } else { ttl }.into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PrefixReport {
    pub model: String,
    pub response: String,
    pub usage: Usage,
    pub prefix: Prefix,
}
