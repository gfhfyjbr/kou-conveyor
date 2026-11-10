pub mod policy;
mod request;
mod response;
mod stream;
mod transport;
pub(crate) use response::{optional_raw, text};
pub(crate) use transport::http_client;

pub use policy::{RespondError, RetryPolicy};
pub use transport::{
    Adapter, CancellationToken, Config, Exchange, Limits, Reporter, RequestOptions, Retry,
};

pub use stream::{ApiError, ResponseState, StreamDecoder, StreamEvent, StreamLimits};

pub use request::request_body;
pub use response::decode_response;

pub fn prompt_cache_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    if key.is_empty() {
        String::new()
    } else {
        format!("{:x}", Sha256::digest(key.as_bytes()))
    }
}
