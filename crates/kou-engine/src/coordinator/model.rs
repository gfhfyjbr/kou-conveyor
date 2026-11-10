use super::journal::BoxFuture;
use kou_domain::Response;
use kou_provider::{SharedRequest, anthropic, clients, codex, responses};
use std::{sync::Arc, time::Duration};
pub use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFailure {
    Other,
    RateLimited(Duration),
    ContextOverflow { tokens: i64, limit: i64 },
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ModelError {
    pub message: String,
    pub kind: ModelFailure,
}
impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ModelFailure::Other,
        }
    }
}
pub type ModelResult = std::result::Result<Response, ModelError>;

/// The driver polls exactly one owned response future. Steering drops it rather
/// than retaining an old request task or accepting a late response.
pub trait Model: Send + Sync {
    fn respond(
        self: Arc<Self>,
        request: SharedRequest,
        cache_key: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, ModelResult>;
}
macro_rules! response_model {
    ($client:ty) => {
        impl Model for $client {
            fn respond(
                self: Arc<Self>,
                request: SharedRequest,
                cache_key: String,
                cancellation: CancellationToken,
            ) -> BoxFuture<'static, ModelResult> {
                Box::pin(async move {
                    <$client>::respond(
                        &self,
                        &request,
                        responses::RequestOptions {
                            cache_key: &cache_key,
                            deadline: None,
                        },
                        &cancellation,
                        &mut (),
                    )
                    .await
                    .map_err(ModelError::from)
                })
            }
        }
    };
}
response_model!(responses::Adapter);
response_model!(clients::Client);
response_model!(codex::Client);
response_model!(anthropic::Client);

impl From<responses::RespondError> for ModelError {
    fn from(error: responses::RespondError) -> Self {
        fn classify(error: &responses::RespondError) -> ModelFailure {
            match error {
                responses::RespondError::RateLimited { retry_after, .. } => {
                    ModelFailure::RateLimited(*retry_after)
                }
                responses::RespondError::ContextOverflow { tokens, limit, .. } => {
                    ModelFailure::ContextOverflow {
                        tokens: *tokens,
                        limit: *limit,
                    }
                }
                responses::RespondError::CredentialsRejected { source } => classify(source),
                _ => ModelFailure::Other,
            }
        }
        Self {
            kind: classify(&error),
            message: error.to_string(),
        }
    }
}
impl From<anthropic::RespondError> for ModelError {
    fn from(error: anthropic::RespondError) -> Self {
        let kind = match &error {
            anthropic::RespondError::RateLimited { retry_after, .. } => {
                ModelFailure::RateLimited(*retry_after)
            }
            anthropic::RespondError::ContextOverflow { tokens, limit, .. } => {
                ModelFailure::ContextOverflow {
                    tokens: *tokens,
                    limit: *limit,
                }
            }
            _ => ModelFailure::Other,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}
