use crate::{
    Error, History, RequestData, Result,
    responses::{self, Adapter, CancellationToken, Reporter, RequestOptions, RespondError},
};
use kou_domain::{Json, Response};
use reqwest::{
    Client as HttpClient,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue},
};
use serde::Deserialize;

pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    OpenAi,
    Fireworks,
    OpenRouter,
    Ollama,
}
impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Fireworks => "fireworks",
            Self::OpenRouter => "OpenRouter",
            Self::Ollama => "ollama",
        }
    }
}

/// Credentials are explicit: construction does not consult the environment,
/// keychain, auth files or a running gateway.
pub struct Config {
    pub service: Service,
    pub api_key: String,
    pub base_url: String,
    pub max_attempts: Option<usize>,
    pub cache_ttl: String,
    pub limits: responses::Limits,
}
impl Config {
    pub fn new(service: Service) -> Self {
        Self {
            service,
            api_key: String::new(),
            base_url: String::new(),
            max_attempts: None,
            cache_ttl: String::new(),
            limits: responses::Limits::default(),
        }
    }
    pub fn responses_config(&self) -> Result<responses::Config> {
        if self.service != Service::Ollama && self.api_key.trim().is_empty() {
            return Err(Error(format!(
                "{} API key must be set",
                self.service.name()
            )));
        }
        let base = self.base_url.trim().trim_end_matches('/');
        let base = if base.is_empty() {
            if self.service == Service::Ollama {
                OLLAMA_BASE_URL
            } else {
                return Err(Error(format!(
                    "{} base URL must be set",
                    self.service.name()
                )));
            }
        } else {
            base
        };
        let mut config = responses::Config::new(format!("{base}/responses"));
        config.limits = self.limits;
        if let Some(attempts) = self.max_attempts {
            if attempts == 0 {
                return Err(Error("max attempts must be positive".into()));
            }
            config.retry.max_attempts = attempts;
        }
        config
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.service != Service::Ollama {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|_| Error("invalid provider authorization header".into()))?;
            value.set_sensitive(true);
            config.headers.insert(AUTHORIZATION, value);
        }
        match self.service {
            Service::OpenAi => config.prompt_cache_key_field = true,
            Service::Fireworks => {
                config.cache_key_header = Some(HeaderName::from_static("x-session-affinity"))
            }
            Service::OpenRouter => {
                let ttl = self.cache_ttl.trim().to_ascii_lowercase();
                let ttl = if ttl.is_empty() { "1h" } else { &ttl };
                if !matches!(ttl, "5m" | "1h") {
                    return Err(Error("cache TTL must be 5m or 1h".into()));
                }
                config.cache_key_header = Some(HeaderName::from_static("x-session-id"));
                config.extensions.insert(
                    "cache_control".into(),
                    Json::parse(&format!(r#"{{"type":"ephemeral","ttl":"{ttl}"}}"#))
                        .map_err(|error| Error(error.to_string()))?,
                );
            }
            Service::Ollama => {}
        }
        Ok(config)
    }
}

#[derive(Clone)]
pub struct Client {
    adapter: Adapter,
    service: Service,
}
impl Client {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            adapter: Adapter::new(config.responses_config()?)?,
            service: config.service,
        })
    }
    pub fn with_client(config: Config, client: HttpClient) -> Result<Self> {
        Ok(Self {
            adapter: Adapter::with_client(config.responses_config()?, client)?,
            service: config.service,
        })
    }
    pub async fn respond(
        &self,
        request: &RequestData<impl History>,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
    ) -> std::result::Result<Response, RespondError> {
        let response = if self.service == Service::Ollama {
            struct WithoutTrace<'a, R: ?Sized>(&'a mut R);
            impl<R: Reporter + ?Sized> Reporter for WithoutTrace<'_, R> {
                fn stream(&mut self, event: responses::StreamEvent) {
                    self.0.stream(event);
                }
                fn warning(&mut self, message: &str) {
                    self.0.warning(message);
                }
                fn retry(&mut self, retry: responses::Retry<'_>) {
                    self.0.retry(retry);
                }
            }
            self.adapter
                .respond(request, options, cancellation, &mut WithoutTrace(reporter))
                .await?
        } else {
            self.adapter
                .respond(request, options, cancellation, reporter)
                .await?
        };
        if self.service == Service::Fireworks {
            fireworks_usage(response).map_err(Into::into)
        } else {
            Ok(response)
        }
    }
}
fn fireworks_usage(mut response: Response) -> Result<Response> {
    if response.usage.raw.is_missing() {
        return Ok(response);
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct PromptDetails {
        cached_tokens: Option<i64>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct CompletionDetails {
        reasoning_tokens: Option<i64>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Usage {
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
        #[serde(deserialize_with = "crate::optional_object")]
        prompt_tokens_details: Option<PromptDetails>,
        #[serde(deserialize_with = "crate::optional_object")]
        completion_tokens_details: Option<CompletionDetails>,
    }
    let usage: Option<Usage> = crate::optional_object(&mut serde_json::Deserializer::from_str(
        response.usage.raw.as_str().unwrap_or("null"),
    ))
    .map_err(|error| Error(format!("decode Fireworks response usage: {error}")))?;
    if let Some(usage) = usage {
        if let Some(tokens) = usage.prompt_tokens {
            response.usage.input_tokens = tokens;
        }
        if let Some(tokens) = usage.completion_tokens {
            response.usage.output_tokens = tokens;
        }
        if let Some(tokens) = usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens)
        {
            response.usage.cached_input_tokens = tokens;
        }
        if let Some(tokens) = usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens)
        {
            response.usage.reasoning_tokens = tokens;
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_configuration_preserves_auth_cache_placement_and_validation() {
        for (service, cache_header, cache_field) in [
            (Service::OpenAi, None, true),
            (Service::Fireworks, Some("x-session-affinity"), false),
            (Service::OpenRouter, Some("x-session-id"), false),
            (Service::Ollama, None, false),
        ] {
            let mut source = Config::new(service);
            if service != Service::Ollama {
                assert_eq!(
                    source.responses_config().err().unwrap().to_string(),
                    format!("{} API key must be set", service.name())
                );
                source.api_key = " fixture-key ".into();
                assert_eq!(
                    source.responses_config().err().unwrap().to_string(),
                    format!("{} base URL must be set", service.name())
                );
            }
            source.base_url = " http://127.0.0.1:9000/v1/// ".into();
            let config = source.responses_config().unwrap();
            assert_eq!(config.endpoint, "http://127.0.0.1:9000/v1/responses");
            assert_eq!(config.headers[CONTENT_TYPE], "application/json");
            assert_eq!(config.prompt_cache_key_field, cache_field);
            assert_eq!(
                config.cache_key_header.as_ref().map(HeaderName::as_str),
                cache_header
            );
            if service == Service::Ollama {
                assert!(!config.headers.contains_key(AUTHORIZATION));
            } else {
                assert_eq!(config.headers[AUTHORIZATION], "Bearer  fixture-key ");
                assert!(config.headers[AUTHORIZATION].is_sensitive());
            }
            source.max_attempts = Some(0);
            assert!(source.responses_config().is_err());
        }
        assert_eq!(
            Config::new(Service::Ollama)
                .responses_config()
                .unwrap()
                .endpoint,
            "http://localhost:11434/v1/responses"
        );
        let mut source = Config::new(Service::OpenRouter);
        source.api_key = "fixture-key".into();
        source.base_url = "http://127.0.0.1".into();
        for ttl in ["", "5m", " 1H ", "5M"] {
            source.cache_ttl = ttl.into();
            assert!(source.responses_config().is_ok());
        }
        source.cache_ttl = "30m".into();
        assert_eq!(
            source.responses_config().err().unwrap().to_string(),
            "cache TTL must be 5m or 1h"
        );
        source.api_key = "private\nsecret".into();
        let error = source.responses_config().err().unwrap().to_string();
        assert!(!error.contains("private") && !error.contains("secret"));
    }
    #[test]
    fn fireworks_overrides_only_present_fields_and_preserves_raw_usage() {
        let mut response=responses::decode_response(br#"{"status":"completed","output":[],"usage":{"input_tokens":8,"output_tokens":4,"input_tokens_details":{"cached_tokens":1},"output_tokens_details":{"reasoning_tokens":2},"prompt_tokens":11,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":5}}}"#).unwrap();
        let raw = response.usage.raw.clone();
        response = fireworks_usage(response).unwrap();
        assert_eq!(
            (
                response.usage.input_tokens,
                response.usage.output_tokens,
                response.usage.cached_input_tokens,
                response.usage.reasoning_tokens
            ),
            (11, 7, 3, 5)
        );
        assert_eq!(response.usage.raw, raw);
        for usage in [r#"{"prompt_tokens":null}"#, r#"{}"#, "null"] {
            response.usage.raw = Json::parse(usage).unwrap();
            let previous = response.usage.clone();
            response = fireworks_usage(response).unwrap();
            assert_eq!(response.usage, previous);
        }
        response.usage.raw = Json::parse(r#"{"prompt_tokens":"invalid"}"#).unwrap();
        assert!(
            fireworks_usage(response)
                .unwrap_err()
                .0
                .starts_with("decode Fireworks response usage:")
        );
    }
}
