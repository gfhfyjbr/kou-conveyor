use crate::{
    Error, History, RequestData, Result, null_default, object_or_null,
    responses::{self, Adapter, CancellationToken, Reporter, RequestOptions, RespondError},
};
use base64::{
    Engine, alphabet,
    engine::{
        DecodePaddingMode,
        general_purpose::{GeneralPurpose, GeneralPurposeConfig},
    },
};
use kou_domain::{Response, validate_json};
use reqwest::{
    Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue},
};
use serde::Deserialize;
use std::{
    fs::OpenOptions,
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

pub const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const CREDENTIAL_BYTES: usize = 1 << 20;

#[derive(Default)]
pub struct Config {
    pub access_token: String,
    pub account_id: String,
    pub auth_file: Option<PathBuf>,
    pub base_url: String,
    pub max_attempts: Option<usize>,
    pub limits: responses::Limits,
}
struct Credentials {
    access_token: String,
    account_id: String,
}
impl Config {
    /// Only selects a source; it never reads the selected file, API keys or the
    /// process environment. The runner supplies its environment/home policy.
    pub fn from_environment(
        mut getenv: impl FnMut(&str) -> String,
        fallback_home: Option<&Path>,
    ) -> Result<Self> {
        let access_token = getenv("OPENAI_CODEX_ACCESS_TOKEN").trim().to_owned();
        let account_id = getenv("OPENAI_CODEX_ACCOUNT_ID").trim().to_owned();
        let file = getenv("OPENAI_CODEX_AUTH_FILE").trim().to_owned();
        if !file.is_empty() && (!access_token.is_empty() || !account_id.is_empty()) {
            return Err(Error("choose OPENAI_CODEX_AUTH_FILE or OPENAI_CODEX_ACCESS_TOKEN/OPENAI_CODEX_ACCOUNT_ID, not both".into()));
        }
        let mut config = Self {
            access_token,
            account_id,
            auth_file: (!file.is_empty()).then(|| PathBuf::from(file)),
            ..Self::default()
        };
        if !config.access_token.is_empty()
            || !config.account_id.is_empty()
            || config.auth_file.is_some()
        {
            return Ok(config);
        }
        let codex_home = getenv("CODEX_HOME").trim().to_owned();
        let home = if codex_home.is_empty() {
            let home = getenv("HOME").trim().to_owned();
            let home = if home.is_empty() {
                fallback_home
                    .ok_or_else(|| {
                        Error("find Codex home: home directory is not available".into())
                    })?
                    .to_owned()
            } else {
                PathBuf::from(home)
            };
            home.join(".codex")
        } else {
            PathBuf::from(codex_home)
        };
        config.auth_file = Some(clean_path(home.join("auth.json")));
        Ok(config)
    }
    fn credentials(&self, now: SystemTime) -> Result<Credentials> {
        let (mut token, mut account) = (
            self.access_token.trim().to_owned(),
            self.account_id.trim().to_owned(),
        );
        if let Some(path) = &self.auth_file {
            if !token.is_empty() || !account.is_empty() {
                return Err(Error(
                    "codex AuthFile cannot be combined with AccessToken or AccountID".into(),
                ));
            }
            (token, account) = read_auth_file(path)?;
        }
        if token.is_empty() {
            return Err(Error("codex access token must be set; use OPENAI_CODEX_ACCESS_TOKEN or a ChatGPT-authenticated Codex auth file".into()));
        }
        if token.starts_with("sk-") || !header_value(&token) {
            return Err(Error("codex requires a subscription access token, not an API key or invalid header value".into()));
        }
        if token.len() > CREDENTIAL_BYTES {
            return Err(Error(
                "codex access token exceeds the credential-byte limit".into(),
            ));
        }
        let (claimed, expires) = token_claims(&token)?;
        let now = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;
        if expires != 0 && now >= expires {
            return Err(Error("codex access token has expired; renew credentials externally (interactive login and token refresh are not implemented)".into()));
        }
        if account.is_empty() {
            account = claimed;
        } else if !claimed.is_empty() && claimed != account {
            return Err(Error(
                "codex account ID does not match the access token".into(),
            ));
        }
        if !header_value(&account) {
            return Err(Error("codex account ID must be set in OPENAI_CODEX_ACCOUNT_ID, the auth file, or the access token".into()));
        }
        Ok(Credentials {
            access_token: token,
            account_id: account,
        })
    }
}
fn clean_path(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if clean
                    .components()
                    .next_back()
                    .is_some_and(|part| matches!(part, Component::Normal(_)))
                {
                    clean.pop();
                } else if !clean.has_root() {
                    clean.push("..");
                }
            }
            part => clean.push(part.as_os_str()),
        }
    }
    if clean.as_os_str().is_empty() {
        clean.push(".");
    }
    clean
}
fn header_value(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
}

pub fn validate_base_url(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value == BASE_URL {
        return Ok(BASE_URL.into());
    }
    let invalid = || {
        Error(
            "codex base URL must be https://chatgpt.com/backend-api/codex or an explicit loopback IP endpoint"
                .into(),
        )
    };
    let url = Url::parse(value).map_err(|_| invalid())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some_and(|query| !query.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
        || !matches!(url.scheme(), "http" | "https")
    {
        return Err(invalid());
    }
    // WHATWG URL parsing canonicalizes integer/hex/short IPv4 spellings. The Go
    // contract accepts a loopback IP literal, not names or normalized aliases.
    let authority = value
        .split_once("://")
        .ok_or_else(invalid)?
        .1
        .split(['/', '?', '#'])
        .next()
        .ok_or_else(invalid)?;
    if authority.contains('@') {
        return Err(invalid());
    }
    let host = if let Some(ipv6) = authority.strip_prefix('[') {
        ipv6.split_once(']')
            .map(|(host, _)| host)
            .ok_or_else(invalid)?
    } else {
        authority.split(':').next().ok_or_else(invalid)?
    };
    if !host.parse::<IpAddr>().is_ok_and(|ip| match ip {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
    }) {
        return Err(invalid());
    }
    Ok(value.into())
}

#[derive(Clone)]
pub struct Client {
    adapter: Adapter,
}
impl Client {
    pub fn new(config: Config) -> Result<Self> {
        let base_url = validate_base_url(&config.base_url)?;
        if config.max_attempts == Some(0) {
            return Err(Error("max attempts must be positive".into()));
        }
        let credentials = config.credentials(SystemTime::now())?;
        let mut http = responses::Config::new(format!("{base_url}/responses"));
        http.limits = config.limits;
        if let Some(attempts) = config.max_attempts {
            http.retry.max_attempts = attempts;
        }
        http.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        for (name, text) in [
            (
                AUTHORIZATION,
                format!("Bearer {}", credentials.access_token),
            ),
            (
                HeaderName::from_static("chatgpt-account-id"),
                credentials.account_id,
            ),
        ] {
            let mut value = HeaderValue::from_str(&text)
                .map_err(|_| Error("invalid Codex credential header".into()))?;
            value.set_sensitive(true);
            http.headers.insert(name, value);
        }
        http.headers
            .insert("originator", HeaderValue::from_static("kou-conveyor"));
        http.headers
            .insert("user-agent", HeaderValue::from_static("kou-conveyor"));
        http.cache_key_header = Some(HeaderName::from_static("session-id"));
        http.prompt_cache_key_field = true;
        // No caller-supplied client can re-enable redirects and forward an account
        // token to a different endpoint. Credentials are snapshotted only once.
        Ok(Self {
            adapter: Adapter::with_client(
                http,
                responses::http_client(false, base_url != BASE_URL)?,
            )?,
        })
    }
    pub async fn respond(
        &self,
        request: &RequestData<impl History>,
        options: RequestOptions<'_>,
        cancellation: &CancellationToken,
        reporter: &mut (impl Reporter + ?Sized),
    ) -> std::result::Result<Response, RespondError> {
        if cancellation.is_cancelled() {
            return Err(RespondError::Cancelled);
        }
        if options
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(RespondError::DeadlineExceeded);
        }
        if request.model.max_output_tokens.is_some() {
            return Err(Error("codex does not support max_output_tokens".into()).into());
        }
        match self
            .adapter
            .respond(request, options, cancellation, reporter)
            .await
        {
            Err(error) if error.api_error().is_some_and(|api| api.status_code == 401) => {
                Err(RespondError::CredentialsRejected {
                    source: Box::new(error),
                })
            }
            result => result,
        }
    }
}
fn read_auth_file(path: &Path) -> Result<(String, String)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO must not block before fstat can reject it as a non-regular file.
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path).map_err(|error| Error(format!("open Codex auth file (provide existing ChatGPT credentials; login is not implemented): {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| Error(format!("inspect Codex auth file: {error}")))?;
    let regular = metadata.is_file();
    #[cfg(unix)]
    let regular = {
        use std::os::unix::fs::PermissionsExt;
        regular && metadata.permissions().mode() & 0o077 == 0
    };
    if !regular {
        return Err(Error(
            "codex auth file must be a regular file with private permissions (chmod 600)".into(),
        ));
    }
    let mut data = Vec::new();
    file.take(CREDENTIAL_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|error| Error(format!("read Codex auth file: {error}")))?;
    let invalid = || {
        Error("invalid Codex auth file; expected a JSON object with tokens.access_token and tokens.account_id".into())
    };
    if data.len() > CREDENTIAL_BYTES || validate_json(&data).is_err() {
        return Err(invalid());
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Tokens {
        #[serde(deserialize_with = "null_default")]
        access_token: String,
        #[serde(deserialize_with = "null_default")]
        account_id: String,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Auth {
        #[serde(rename = "auth_mode", deserialize_with = "null_default")]
        mode: String,
        #[serde(deserialize_with = "object_or_null")]
        tokens: Tokens,
    }
    let auth: Auth =
        object_or_null(&mut serde_json::Deserializer::from_slice(&data)).map_err(|_| invalid())?;
    if !auth.mode.is_empty() && auth.mode != "chatgpt" {
        return Err(Error(
            "codex auth file is not a ChatGPT subscription login".into(),
        ));
    }
    Ok((
        auth.tokens.access_token.trim().into(),
        auth.tokens.account_id.trim().into(),
    ))
}
// These unverified claims are routing/expiry hints. The remote server, not this
// parser, authenticates the subscription token and verifies its signature.
fn token_claims(token: &str) -> Result<(String, i64)> {
    let mut parts = token.split('.');
    let (_, Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok((String::new(), 0));
    };
    let invalid = || Error("invalid Codex access token claims".into());
    let decoder = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    let data = decoder.decode(payload).map_err(|_| invalid())?;
    if validate_json(&data).is_err() {
        return Err(invalid());
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Auth {
        #[serde(deserialize_with = "null_default")]
        chatgpt_account_id: String,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Claims {
        #[serde(deserialize_with = "null_default")]
        exp: i64,
        #[serde(
            rename = "https://api.openai.com/auth",
            deserialize_with = "object_or_null"
        )]
        auth: Auth,
    }
    let claims: Claims =
        object_or_null(&mut serde_json::Deserializer::from_slice(&data)).map_err(|_| invalid())?;
    Ok((claims.auth.chatgpt_account_id, claims.exp))
}

#[cfg(test)]
#[path = "codex/tests.rs"]
mod tests;
