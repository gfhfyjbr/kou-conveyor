//! Where runs go: the connection the Go cockpits save and hand to the runner
//! in its environment, the accounts gateway they resolve it against, the
//! preferences both cockpits share, and the models the connection lists.
//!
//! The files are the cockpits' own (`settings.json`, `gateway.json` and
//! `preferences.json` beside each other, `.meta/<id>.json` beside a session),
//! so a choice made here is the one the Go cockpits see, and the other way
//! round.

use anyhow::{Context as _, Result, anyhow, bail};
use kou_thread::{Choice, Provider};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpStream, ToSocketAddrs as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// The reasoning efforts the runner accepts, lowest first.
pub(crate) const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The effort a run takes when nothing chose one, as the runner's own default.
pub(crate) const DEFAULT_EFFORT: &str = "high";

const PROVIDER: &str = "KOU_CONVEYOR_LLM_PROVIDER";
const BASE_URL: &str = "KOU_CONVEYOR_LLM_BASE_URL";
const API_KEY: &str = "KOU_CONVEYOR_LLM_API_KEY";
const MODEL: &str = "KOU_CONVEYOR_LLM_MODEL";
const CONFIG: &str = "KOU_CONVEYOR_CONFIG";

/// How long listing a connection's models may take before the session opens
/// without them.
const LIST_TIMEOUT: Duration = Duration::from_secs(8);

/// What a connection over an API uses when the settings leave something out.
struct ApiDefaults {
    provider: &'static str,
    base_url: &'static str,
    model: &'static str,
    key_variable: &'static str,
}

fn defaults_for(api: &str) -> Option<ApiDefaults> {
    match api {
        "responses" => Some(ApiDefaults {
            provider: "openai",
            base_url: "https://api.openai.com/v1",
            model: "gpt-6-astra",
            key_variable: "OPENAI_API_KEY",
        }),
        "messages" => Some(ApiDefaults {
            provider: "anthropic",
            base_url: "https://api.anthropic.com",
            model: "claude-opus-5",
            key_variable: "ANTHROPIC_API_KEY",
        }),
        _ => None,
    }
}

/// Where the cockpits' files are and what a launch adds to the environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Setup {
    /// `settings.json`; `None` leaves the connection to the environment.
    pub settings_file: Option<PathBuf>,
    /// A runner provider that takes precedence over the saved connection.
    pub provider: Option<String>,
    /// Variables for the runner, after all the others.
    pub env: Vec<(String, String)>,
}

impl Setup {
    /// The preferences file that goes with the settings.
    fn preferences_file(&self) -> Option<PathBuf> {
        Some(
            self.settings_file
                .as_ref()?
                .with_file_name("preferences.json"),
        )
    }

    /// The gateway's publication that goes with the settings.
    fn gateway_file(&self) -> Option<PathBuf> {
        Some(self.settings_file.as_ref()?.with_file_name("gateway.json"))
    }

    /// Looks a variable up as a run would see it: what the launch adds, the
    /// process environment, then the workspace's `.env` file.
    fn getenv(&self, workspace: &Path, name: &str) -> String {
        if let Some((_, value)) = self.env.iter().rev().find(|(known, _)| known == name) {
            return value.clone();
        }
        if let Ok(value) = std::env::var(name) {
            return value;
        }
        let Ok(file) = std::fs::read_to_string(workspace.join(".env")) else {
            return String::new();
        };
        file.lines()
            .filter_map(|line| line.trim().split_once('='))
            .filter(|(known, _)| known.trim() == name)
            .map(|(_, value)| value.trim().to_owned())
            .next_back()
            .unwrap_or_default()
    }
}

/// `KOU_CONVEYOR_CONFIG`, or `settings.json` in the user's configuration
/// directory, as the cockpits find it.
pub(crate) fn default_settings_file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG).filter(|path| !path.is_empty()) {
        return std::path::absolute(path).ok();
    }
    Some(
        dirs::config_dir()?
            .join("kou-conveyor")
            .join("settings.json"),
    )
}

/// The connection the cockpits save.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Settings {
    /// `""` (the environment decides), `responses`, `messages` or `gateway`.
    provider_type: String,
    base_url: String,
    api_key: String,
    model: String,
    /// The API runs speak through the gateway, when not the model's own.
    protocol: String,
    /// The used share of the context window that starts a compaction.
    auto_compact_percent: f64,
}

impl Settings {
    fn load(path: &Path) -> Result<Self> {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("read settings"),
        };
        let mut settings: Self = serde_json::from_slice(&data)
            .with_context(|| format!("read settings {}", path.display()))?;
        settings.provider_type = settings.provider_type.trim().to_ascii_lowercase();
        settings.base_url = settings.base_url.trim().trim_end_matches('/').to_owned();
        settings.api_key = settings.api_key.trim().to_owned();
        settings.model = settings.model.trim().to_owned();
        settings.protocol = settings.protocol.trim().to_ascii_lowercase();
        if !matches!(
            settings.provider_type.as_str(),
            "" | "responses" | "messages" | "gateway"
        ) {
            bail!(
                "settings {}: unknown provider type {:?}; use gateway, responses or messages",
                path.display(),
                settings.provider_type
            );
        }
        Ok(settings)
    }

    /// The connection a run with `model` uses: through the gateway the API
    /// follows the model, since a session may change providers.
    fn for_model(mut self, model: &str) -> Self {
        let model = model.trim();
        if model.is_empty() || model == self.model || self.provider_type.is_empty() {
            return self;
        }
        if self.provider_type == "gateway" && !self.model.is_empty() {
            self.protocol.clear();
        }
        self.model = model.to_owned();
        self
    }

    /// Resolves gateway settings into the endpoint the gateway listens on.
    fn through(self, gateway: &Gateway) -> Self {
        let api = if self.protocol.is_empty() {
            gateway_api(&self.model).to_owned()
        } else {
            self.protocol.clone()
        };
        let mut base_url = gateway.url.trim_end_matches('/').to_owned();
        // The Messages API's clients add /v1 themselves.
        if api == "responses" {
            base_url.push_str("/v1");
        }
        Self {
            provider_type: api,
            base_url,
            api_key: gateway.api_key.clone(),
            protocol: String::new(),
            ..self
        }
    }

    /// Applies the settings to a runner's environment. Settings that name an
    /// API define the whole connection: unset values are set empty, so the
    /// workspace's `.env` cannot fill them in.
    fn environment(&self, env: &mut Vec<(String, String)>) {
        if self.auto_compact_percent > 0.0 {
            set(
                env,
                "KOU_CONVEYOR_AUTO_COMPACT",
                &format!("{}%", self.auto_compact_percent),
            );
        }
        let Some(defaults) = defaults_for(&self.provider_type) else {
            return;
        };
        set(env, PROVIDER, defaults.provider);
        set(env, BASE_URL, &self.base_url);
        set(env, API_KEY, &self.api_key);
        set(env, MODEL, &self.model);
    }
}

fn set(env: &mut Vec<(String, String)>, name: &str, value: &str) {
    env.retain(|(known, _)| known != name);
    env.push((name.to_owned(), value.to_owned()));
}

/// The accounts gateway the browser cockpit runs, as it publishes itself.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Gateway {
    url: String,
    api_key: String,
    owner: String,
    /// A socket whose open connection keeps a managed gateway alive.
    lease_socket: String,
}

impl Gateway {
    fn load(path: Option<&Path>) -> Result<Self> {
        let off = || anyhow!("the accounts gateway is not running: kou-conveyor-web runs it");
        let path = path.ok_or_else(off)?;
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(off()),
            Err(error) => return Err(error.into()),
        };
        let gateway: Self = serde_json::from_slice(&data).unwrap_or_default();
        if gateway.url.is_empty() || gateway.api_key.is_empty() {
            bail!("{} is not a gateway's publication", path.display());
        }
        Ok(gateway)
    }

    /// Checks that the gateway answers, so a run that cannot reach it says so
    /// before it starts rather than at its first request.
    fn reach(&self) -> Result<()> {
        let (host, _) = split_http_url(&self.url)
            .ok_or_else(|| anyhow!("the gateway's address {:?} is not a URL", self.url))?;
        connect(&host, Duration::from_secs(2))
            .map(drop)
            .map_err(|_| {
                anyhow!(
                    "the accounts gateway is not running: nothing answers at {}",
                    self.url
                )
            })
    }
}

/// The API runs speak through the gateway for a model: Claude models the
/// Messages API, the others the Responses API.
fn gateway_api(model: &str) -> &'static str {
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    if name.starts_with("claude") {
        "messages"
    } else {
        "responses"
    }
}

/// Keeps a managed gateway alive for as long as it is held.
#[derive(Debug)]
pub(crate) struct Lease {
    #[cfg(unix)]
    _connection: std::os::unix::net::UnixStream,
}

#[cfg(unix)]
fn lease(gateway: &Gateway) -> Result<Lease> {
    use std::os::unix::net::UnixStream;
    let mut connection =
        UnixStream::connect(&gateway.lease_socket).context("lease the accounts gateway")?;
    connection.set_read_timeout(Some(Duration::from_secs(5)))?;
    connection.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(
        connection,
        "GET /internal/lease HTTP/1.1\r\nHost: accounts\r\nAuthorization: Bearer {}\r\nX-Kou-Gateway-Owner: {}\r\n\r\n",
        gateway.api_key, gateway.owner
    )?;
    let mut reader = BufReader::new(&connection);
    let (status, headers) = read_head(&mut reader)?;
    if status != 200 || headers.get("x-kou-gateway-owner") != Some(&gateway.owner) {
        bail!("lease the accounts gateway: the helper did not confirm its owner");
    }
    connection.set_read_timeout(None)?;
    Ok(Lease {
        _connection: connection,
    })
}

#[cfg(not(unix))]
fn lease(_gateway: &Gateway) -> Result<Lease> {
    bail!("the accounts gateway's lease needs a Unix socket")
}

/// What a runner is launched with.
#[derive(Debug, Default)]
pub(crate) struct Launch {
    /// Variables set over the process environment.
    pub env: Vec<(String, String)>,
    /// Held until the runner exits.
    pub lease: Option<Lease>,
    /// The runner provider the run asks for, when a cockpit can tell.
    pub provider: String,
}

/// Builds the environment of a run with `model`, as the cockpits' `Start`
/// does: the saved connection, resolved against the gateway when it goes
/// through it, the user's plugins and the tool profile and sandbox they chose.
pub(crate) fn launch(setup: &Setup, workspace: &Path, model: &str) -> Result<Launch> {
    // A serving-capacity hint belongs to a launch that resolved it; none here.
    let mut env = vec![(
        "KOU_CONVEYOR_SERVED_CONTEXT_WINDOW".to_owned(),
        String::new(),
    )];
    let mut provider = setup.provider.clone().unwrap_or_default();
    let mut resolved_gateway = None;
    if let Some(provider) = &setup.provider {
        if let Some(file) = &setup.settings_file
            && let Ok(settings) = Settings::load(file)
        {
            Settings {
                auto_compact_percent: settings.auto_compact_percent,
                ..Settings::default()
            }
            .environment(&mut env);
        }
        set(&mut env, PROVIDER, provider);
    } else if let Some(file) = &setup.settings_file {
        let mut settings = Settings::load(file)
            .map_err(|error| anyhow!("{error:#}; fix them in the connection settings"))?
            .for_model(model);
        if settings.provider_type == "gateway" {
            let gateway = Gateway::load(setup.gateway_file().as_deref())?;
            gateway.reach()?;
            settings = settings.through(&gateway);
            resolved_gateway = Some(gateway);
        }
        settings.environment(&mut env);
        if let Some(defaults) = defaults_for(&settings.provider_type) {
            provider = defaults.provider.to_owned();
        }
    }
    if provider.is_empty() {
        provider = setup.getenv(workspace, PROVIDER);
    }
    if let Some(file) = &setup.settings_file {
        set(&mut env, CONFIG, &file.to_string_lossy());
        let preferences = Preferences::load(setup);
        if !preferences.tool_profile.is_empty() {
            set(
                &mut env,
                "KOU_CONVEYOR_TOOL_PROFILE",
                &preferences.tool_profile,
            );
        }
        if !preferences.sandbox.is_empty() {
            set(&mut env, "KOU_CONVEYOR_SANDBOX", &preferences.sandbox);
        }
    }
    for (name, value) in &setup.env {
        set(&mut env, name, value);
    }
    // A managed gateway stays up only while something holds its lease.
    let gateway = resolved_gateway.or_else(|| Gateway::load(setup.gateway_file().as_deref()).ok());
    let mut held = None;
    if let Some(gateway) = gateway.filter(|gateway| !gateway.lease_socket.is_empty()) {
        let base = env
            .iter()
            .rev()
            .find(|(name, _)| name == BASE_URL)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| setup.getenv(workspace, BASE_URL));
        let base = base.trim_end_matches('/');
        if base.strip_suffix("/v1").unwrap_or(base) == gateway.url.trim_end_matches('/') {
            held = Some(lease(&gateway)?);
        }
    }
    Ok(Launch {
        env,
        lease: held,
        provider,
    })
}

/// Choices the cockpits keep that are not part of the connection.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    /// The effort last chosen in a composer, which a new session starts with.
    pub effort: String,
    /// The model last chosen in a composer, which a new session starts with.
    pub last_model: String,
    /// The built-in tools of the next runs: auto, edit, patch, code or shell.
    pub tool_profile: String,
    /// Where the next runs work: off, worktree or container.
    pub sandbox: String,
}

impl Preferences {
    /// Reads the preferences; a missing or unreadable file has none.
    pub(crate) fn load(setup: &Setup) -> Self {
        let Some(path) = setup.preferences_file() else {
            return Self::default();
        };
        let mut preferences: Self = std::fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        if !EFFORTS.contains(&preferences.effort.as_str()) {
            preferences.effort.clear();
        }
        if !valid_model(&preferences.last_model) {
            preferences.last_model.clear();
        }
        if !matches!(
            preferences.tool_profile.as_str(),
            "auto" | "edit" | "patch" | "code" | "shell"
        ) {
            preferences.tool_profile.clear();
        }
        if !matches!(
            preferences.sandbox.as_str(),
            "off" | "worktree" | "container"
        ) {
            preferences.sandbox.clear();
        }
        preferences
    }

    /// Records one preference, keeping the others, whatever cockpit wrote them.
    pub(crate) fn save(setup: &Setup, name: &str, value: &str) -> Result<()> {
        let path = setup
            .preferences_file()
            .ok_or_else(|| anyhow!("preferences are unavailable without a settings file"))?;
        update_json(&path, |object| {
            object.insert(name.to_owned(), Value::String(value.to_owned()));
        })
    }
}

/// Whether the runner could be handed the model ID.
pub(crate) fn valid_model(model: &str) -> bool {
    model.len() <= 256 && !model.contains(|ch: char| ch.is_whitespace() || ch.is_control())
}

/// What the cockpits add to a session the runner owns.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Meta {
    /// A title of the user's choosing.
    pub title: String,
    /// The title a model made of the first prompt.
    pub auto_title: String,
    /// The session's own effort.
    pub effort: String,
}

impl Meta {
    /// Reads a session's metadata; a session without any has none.
    pub(crate) fn load(session_dir: &Path, id: &str) -> Self {
        let mut meta: Self = std::fs::read(meta_path(session_dir, id))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        meta.title = crate::text::headline(&crate::text::clean(&meta.title), 120);
        meta.auto_title = crate::text::headline(&crate::text::clean(&meta.auto_title), 120);
        if !EFFORTS.contains(&meta.effort.as_str()) {
            meta.effort.clear();
        }
        meta
    }

    /// The title the session is shown under, if it has one.
    pub(crate) fn shown_title(&self) -> Option<&str> {
        [&self.title, &self.auto_title]
            .into_iter()
            .map(String::as_str)
            .find(|title| !title.is_empty())
    }

    /// Keeps the effort with the session, as the cockpits do when a run starts.
    pub(crate) fn save_effort(session_dir: &Path, id: &str, effort: &str) -> Result<()> {
        update_json(&meta_path(session_dir, id), |object| {
            object.insert("effort".to_owned(), Value::String(effort.to_owned()));
        })
    }
}

fn meta_path(session_dir: &Path, id: &str) -> PathBuf {
    session_dir.join(".meta").join(format!("{id}.json"))
}

/// Changes a JSON object file in place of itself: read, changed, written to a
/// file beside it and renamed over, so a reader never sees half of it.
fn update_json(path: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    let mut object = std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice::<Map<String, Value>>(&data).ok())
        .unwrap_or_default();
    change(&mut object);
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no directory", path.display()))?;
    create_private_dir(directory)?;
    let mut data = serde_json::to_vec(&object)?;
    data.push(b'\n');
    let temporary = directory.join(format!(
        ".{}.{}.new",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    write_private(&temporary, &data)?;
    std::fs::rename(&temporary, path).with_context(|| format!("save {}", path.display()))
}

/// Creates a directory only its owner can read, as the cockpits' are.
pub(crate) fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .with_context(|| format!("create {}", path.display()))
}

/// Writes a file only its owner can read.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(data))
        .with_context(|| format!("write {}", path.display()))
}

/// The models a prompt can run with over the connection runs use.
#[derive(Clone, Debug, Default)]
pub(crate) struct Catalog {
    /// The models, the best known providers first, the newest first.
    pub models: Vec<Choice>,
    /// Who serves them, in the same order, each with its models that an
    /// agent runs on: those that make media are found by searching.
    pub providers: Vec<Provider>,
    /// Why the connection listed none.
    pub error: Option<String>,
    /// The model the settings of an endpoint or the environment name.
    pub named: Option<String>,
    /// The provider's default, which a run without a model takes.
    pub fallback: Option<String>,
    /// The context window the environment sets for every model.
    pub context_window: Option<u64>,
}

impl Catalog {
    /// The first model listed that an agent runs on.
    pub(crate) fn first(&self) -> Option<&str> {
        self.models
            .iter()
            .find(|model| !is_media(&model.id))
            .map(|model| model.id.as_str())
    }
}

/// Lists the models of the connection runs use: the gateway's, or those of
/// the endpoint the settings or the environment name. A connection that
/// lists none still runs any model it is given.
pub(crate) fn list_models(setup: &Setup, workspace: &Path) -> Catalog {
    let mut catalog = Catalog {
        context_window: parse_tokens(&setup.getenv(workspace, "KOU_CONVEYOR_CONTEXT_WINDOW")),
        ..Catalog::default()
    };
    let settings = match (&setup.provider, &setup.settings_file) {
        (None, Some(file)) => match Settings::load(file) {
            Ok(settings) => settings,
            Err(error) => {
                catalog.error = Some(format!("{error:#}"));
                Settings::default()
            }
        },
        _ => Settings::default(),
    };
    let listed = if settings.provider_type == "gateway" {
        Gateway::load(setup.gateway_file().as_deref()).and_then(|gateway| {
            gateway.reach()?;
            let url = format!("{}/v1/models", gateway.url.trim_end_matches('/'));
            fetch_models(
                &url,
                &[("Authorization", format!("Bearer {}", gateway.api_key))],
            )
        })
    } else {
        let (api, base_url, key) = if settings.provider_type.is_empty() {
            let provider = setup
                .provider
                .clone()
                .filter(|provider| !provider.is_empty())
                .unwrap_or_else(|| setup.getenv(workspace, PROVIDER));
            let api = match provider.as_str() {
                "" | "openai" => "responses",
                "anthropic" => "messages",
                // Other providers are configured by their own environment.
                _ => return catalog,
            };
            catalog.named = Some(setup.getenv(workspace, MODEL)).filter(|m| !m.is_empty());
            let key = setup.getenv(workspace, API_KEY);
            (api, setup.getenv(workspace, BASE_URL), key)
        } else {
            catalog.named = Some(settings.model.clone()).filter(|m| !m.is_empty());
            (
                settings.provider_type.as_str(),
                settings.base_url.clone(),
                settings.api_key.clone(),
            )
        };
        let Some(defaults) = defaults_for(api) else {
            return catalog;
        };
        catalog.fallback = Some(defaults.model.to_owned());
        let key = Some(key)
            .filter(|key| !key.trim().is_empty())
            .unwrap_or_else(|| setup.getenv(workspace, defaults.key_variable));
        let base_url = Some(base_url)
            .filter(|base| !base.trim().is_empty())
            .unwrap_or_else(|| defaults.base_url.to_owned());
        let base_url = base_url.trim_end_matches('/');
        if key.trim().is_empty() {
            Err(anyhow!(
                "no API key: enter one in the connection settings or set {}",
                defaults.key_variable
            ))
        } else if api == "messages" {
            fetch_models(
                &format!("{base_url}/v1/models?limit=1000"),
                &[
                    ("x-api-key", key.trim().to_owned()),
                    ("anthropic-version", "2023-06-01".to_owned()),
                ],
            )
        } else {
            fetch_models(
                &format!("{base_url}/models"),
                &[("Authorization", format!("Bearer {}", key.trim()))],
            )
        }
    };
    match listed {
        Ok(mut models) => {
            sort_models(&mut models);
            catalog.providers = providers(&models);
            catalog.models = models.into_iter().map(ListedModel::choice).collect();
        }
        Err(error) => catalog.error = Some(format!("{error:#}")),
    }
    catalog
}

/// A positive token count, optionally in thousands (k) or millions (m).
fn parse_tokens(value: &str) -> Option<u64> {
    let value = value.trim().to_ascii_lowercase();
    let (digits, scale) = match value.as_bytes().last()? {
        b'k' => (&value[..value.len() - 1], 1e3),
        b'm' => (&value[..value.len() - 1], 1e6),
        _ => (value.as_str(), 1.0),
    };
    let tokens = digits.trim().parse::<f64>().ok()? * scale;
    (1.0..9e15).contains(&tokens).then_some(tokens as u64)
}

/// A model as an endpoint lists it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ListedModel {
    id: String,
    owned_by: String,
    created: i64,
    display_name: String,
    #[serde(skip)]
    provider: String,
}

impl ListedModel {
    fn choice(self) -> Choice {
        let mut description = provider_name(&self.provider);
        if is_media(&self.id) {
            description.push_str(", makes media");
        }
        Choice {
            name: if self.display_name.is_empty() {
                self.id.clone()
            } else {
                self.display_name
            },
            id: self.id,
            description: Some(description),
        }
    }
}

/// The providers of sorted models, in the order they come, each with its
/// models that an agent runs on. A model whose maker is unknown is Other's.
fn providers(models: &[ListedModel]) -> Vec<Provider> {
    let mut providers: Vec<Provider> = Vec::new();
    for model in models.iter().filter(|model| !is_media(&model.id)) {
        match providers
            .iter_mut()
            .find(|provider| provider.id == model.provider)
        {
            Some(provider) => provider.models.push(model.id.clone()),
            None => providers.push(Provider {
                id: model.provider.clone(),
                name: provider_name(&model.provider),
                models: vec![model.id.clone()],
            }),
        }
    }
    providers
}

fn fetch_models(url: &str, headers: &[(&str, String)]) -> Result<Vec<ListedModel>> {
    #[derive(Deserialize)]
    struct List {
        data: Vec<ListedModel>,
    }
    let (status, body) = http_get(url, headers)?;
    match status {
        200 => {}
        401 | 403 => bail!("the endpoint rejected the API key ({status})"),
        404 => bail!("the server has no model list at {url}; check the base URL"),
        _ => bail!("the endpoint answered {status} to its model list"),
    }
    let list: List = serde_json::from_slice(&body)
        .map_err(|_| anyhow!("the endpoint's model list is not in the OpenAI format"))?;
    Ok(list
        .data
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .map(|mut model| {
            model.provider = provider_of(&model);
            model
        })
        .collect())
}

/// Who makes a model: what the list says, else what its ID tells.
fn provider_of(model: &ListedModel) -> String {
    const FAMILIES: [(&str, &str); 22] = [
        ("claude", "anthropic"),
        ("gpt", "openai"),
        ("chatgpt", "openai"),
        ("codex", "openai"),
        ("o1", "openai"),
        ("o3", "openai"),
        ("o4", "openai"),
        ("gemini", "google"),
        ("gemma", "google"),
        ("grok", "xai"),
        ("kimi", "moonshot"),
        ("moonshot", "moonshot"),
        ("deepseek", "deepseek"),
        ("qwen", "qwen"),
        ("qwq", "qwen"),
        ("llama", "meta"),
        ("muse", "meta"),
        ("mistral", "mistral"),
        ("codestral", "mistral"),
        ("devstral", "mistral"),
        ("glm", "zhipu"),
        ("minimax", "minimax"),
    ];
    let listed = model.owned_by.trim().to_ascii_lowercase();
    if !listed.is_empty() {
        return listed;
    }
    let id = model.id.to_ascii_lowercase();
    let name = match id.split_once('/') {
        Some((vendor, _)) if !vendor.is_empty() => return vendor.to_owned(),
        _ => id.as_str(),
    };
    FAMILIES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, provider)| (*provider).to_owned())
        .unwrap_or_default()
}

/// A model provider's name for people.
fn provider_name(provider: &str) -> String {
    const NAMES: [(&str, &str); 19] = [
        ("anthropic", "Anthropic"),
        ("openai", "OpenAI"),
        ("google", "Google"),
        ("xai", "xAI"),
        ("moonshot", "Moonshot"),
        ("meta", "Meta"),
        ("deepseek", "DeepSeek"),
        ("mistral", "Mistral"),
        ("qwen", "Qwen"),
        ("zhipu", "Zhipu"),
        ("antigravity", "Antigravity"),
        ("cognition", "Cognition"),
        ("minimax", "MiniMax"),
        ("iflow", "iFlow"),
        ("openrouter", "OpenRouter"),
        ("ollama", "Ollama"),
        ("groq", "Groq"),
        ("cohere", "Cohere"),
        ("amazon", "Amazon"),
    ];
    if let Some((_, name)) = NAMES.iter().find(|(id, _)| *id == provider) {
        return (*name).to_owned();
    }
    let mut chars = provider.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Other".to_owned(),
    }
}

/// Whether a model makes images, video or sound, which an agent does not run on.
fn is_media(id: &str) -> bool {
    const WORDS: [&str; 16] = [
        "imagine",
        "image",
        "images",
        "video",
        "tts",
        "audio",
        "speech",
        "transcribe",
        "whisper",
        "embedding",
        "embeddings",
        "embed",
        "dall-e",
        "sora",
        "veo",
        "imagen",
    ];
    let id = id.to_ascii_lowercase();
    let boundary = |ch: Option<char>, start: bool| match ch {
        None => true,
        Some(ch) => matches!(ch, '-' | '_' | '.') || (start && ch == '/'),
    };
    WORDS.iter().any(|word| {
        id.match_indices(word).any(|(at, _)| {
            boundary(id[..at].chars().next_back(), true)
                && boundary(id[at + word.len()..].chars().next(), false)
        })
    })
}

/// Orders models by provider, the best known first, then the newest first,
/// then by ID; models that make media come last.
fn sort_models(models: &mut [ListedModel]) {
    const RANK: [&str; 10] = [
        "anthropic",
        "openai",
        "google",
        "xai",
        "moonshot",
        "deepseek",
        "qwen",
        "meta",
        "mistral",
        "zhipu",
    ];
    let rank = |provider: &str| {
        RANK.iter()
            .position(|known| *known == provider)
            .unwrap_or(RANK.len())
    };
    models.sort_by(|a, b| {
        is_media(&a.id)
            .cmp(&is_media(&b.id))
            .then(rank(&a.provider).cmp(&rank(&b.provider)))
            .then_with(|| a.provider.cmp(&b.provider))
            .then(b.created.cmp(&a.created))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// The host (with its port) and the path of an `http://` URL.
fn split_http_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return None;
    }
    let host = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:80")
    };
    Some((host, path.to_owned()))
}

fn connect(host: &str, timeout: Duration) -> Result<TcpStream> {
    let mut last = anyhow!("{host} has no address");
    for address in host.to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error.into(),
        }
    }
    Err(last)
}

/// Reads a response's status and headers, the names in lower case.
fn read_head(reader: &mut impl std::io::BufRead) -> Result<(u16, HashMap<String, String>)> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| anyhow!("not an HTTP response"))?;
    let mut headers = HashMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Ok((status, headers))
}

/// Gets a URL: over plain HTTP by itself, as the local gateway speaks, and
/// through the system's `curl` for HTTPS, which keeps a TLS stack out of the
/// application for the one request that needs it.
fn http_get(url: &str, headers: &[(&str, String)]) -> Result<(u16, Vec<u8>)> {
    let Some((host, path)) = split_http_url(url) else {
        return curl_get(url, headers);
    };
    let mut stream =
        connect(&host, Duration::from_secs(2)).with_context(|| format!("cannot reach {host}"))?;
    stream.set_read_timeout(Some(LIST_TIMEOUT))?;
    stream.set_write_timeout(Some(LIST_TIMEOUT))?;
    let mut request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let (status, head) = read_head(&mut reader)?;
    let mut body = Vec::new();
    if head
        .get("transfer-encoding")
        .is_some_and(|encoding| encoding.to_ascii_lowercase().contains("chunked"))
    {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size)?;
            let size = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16)
                .map_err(|_| anyhow!("a chunk without a size"))?;
            if size == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            let mut end = [0; 2];
            reader.read_exact(&mut end)?;
        }
    } else if let Some(length) = head.get("content-length").and_then(|l| l.parse().ok()) {
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
    } else {
        reader.read_to_end(&mut body)?;
    }
    Ok((status, body))
}

fn curl_get(url: &str, headers: &[(&str, String)]) -> Result<(u16, Vec<u8>)> {
    // The request goes in on stdin, so the key shows in no process listing.
    let quoted = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let mut config = format!("url = \"{}\"\n", quoted(url));
    for (name, value) in headers {
        config.push_str(&format!(
            "header = \"{}: {}\"\n",
            quoted(name),
            quoted(value)
        ));
    }
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--max-time",
            &LIST_TIMEOUT.as_secs().to_string(),
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("HTTPS endpoints are listed with curl, which did not start")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("curl takes no input"))?
        .write_all(config.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "cannot reach the endpoint: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut body = output.stdout;
    let cut = body
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| anyhow!("curl reported no status"))?;
    let status = std::str::from_utf8(&body[cut + 1..])
        .ok()
        .and_then(|status| status.trim().parse().ok())
        .ok_or_else(|| anyhow!("curl reported no status"))?;
    body.truncate(cut);
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serves one HTTP response to each connection, and says what it was asked.
    fn serve(responses: Vec<String>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        let (asked, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                let _ = asked.send(request);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{address}"), requests)
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn setup_in(directory: &Path) -> Setup {
        Setup {
            settings_file: Some(directory.join("settings.json")),
            ..Setup::default()
        }
    }

    #[test]
    fn gateway_settings_resolve_to_its_endpoint_by_the_model_s_api() {
        let directory = tempfile::tempdir().expect("a directory");
        // The reachability probe, then nothing else.
        let (url, _requests) = serve(vec![String::new(), String::new()]);
        let setup = setup_in(directory.path());
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"provider_type":"gateway","model":"mimo-v2","auto_compact_percent":80}"#,
        )
        .expect("settings");
        std::fs::write(
            directory.path().join("gateway.json"),
            format!(r#"{{"url":"{url}","api_key":"gw-key","owner":"o"}}"#),
        )
        .expect("a publication");
        std::fs::write(
            directory.path().join("preferences.json"),
            r#"{"tool_profile":"edit","sandbox":"nonsense","last_model":"gpt-x"}"#,
        )
        .expect("preferences");

        let launch = launch(&setup, directory.path(), "claude-opus-5").expect("a launch");
        let value = |name: &str| {
            launch
                .env
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value(PROVIDER), Some("anthropic"));
        assert_eq!(value(BASE_URL), Some(url.as_str()));
        assert_eq!(value(API_KEY), Some("gw-key"));
        assert_eq!(value(MODEL), Some("claude-opus-5"));
        assert_eq!(value("KOU_CONVEYOR_AUTO_COMPACT"), Some("80%"));
        assert_eq!(value("KOU_CONVEYOR_TOOL_PROFILE"), Some("edit"));
        assert_eq!(value("KOU_CONVEYOR_SANDBOX"), None);
        assert_eq!(value("KOU_CONVEYOR_SERVED_CONTEXT_WINDOW"), Some(""));
        assert_eq!(launch.provider, "anthropic");
        assert!(launch.lease.is_none());

        let launch = launch_for(&setup, directory.path(), "gpt-6.1-sol");
        assert!(launch.contains(&(BASE_URL.to_owned(), format!("{url}/v1"))));
        assert!(launch.contains(&(PROVIDER.to_owned(), "openai".to_owned())));
    }

    fn launch_for(setup: &Setup, workspace: &Path, model: &str) -> Vec<(String, String)> {
        launch(setup, workspace, model).expect("a launch").env
    }

    #[test]
    fn a_gateway_that_is_not_running_says_so_before_the_run() {
        let directory = tempfile::tempdir().expect("a directory");
        let setup = setup_in(directory.path());
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"provider_type":"gateway"}"#,
        )
        .expect("settings");
        let error = launch(&setup, directory.path(), "").expect_err("no gateway");
        assert!(error.to_string().contains("not running"), "{error}");
    }

    #[test]
    fn a_provider_bypasses_the_saved_connection_and_extra_variables_win() {
        let directory = tempfile::tempdir().expect("a directory");
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"provider_type":"messages","api_key":"saved"}"#,
        )
        .expect("settings");
        let setup = Setup {
            provider: Some("openai".into()),
            env: vec![(BASE_URL.into(), "http://127.0.0.1:1/v1".into())],
            ..setup_in(directory.path())
        };
        let env = launch_for(&setup, directory.path(), "m");
        assert!(env.contains(&(PROVIDER.to_owned(), "openai".to_owned())));
        assert!(env.contains(&(BASE_URL.to_owned(), "http://127.0.0.1:1/v1".to_owned())));
        assert!(!env.iter().any(|(name, _)| name == API_KEY));
    }

    #[test]
    fn the_gateway_s_models_are_listed_by_provider_with_media_last() {
        let directory = tempfile::tempdir().expect("a directory");
        let models = r#"{"data":[
            {"id":"gpt-image-2","owned_by":"openai","created":9},
            {"id":"grok-4.7","created":5},
            {"id":"claude-opus-5","owned_by":"anthropic","created":3},
            {"id":"gpt-6.1-sol","owned_by":"openai","created":7},
            {"id":"claude-fable-5","owned_by":"anthropic","created":4}
        ]}"#;
        let (url, requests) = serve(vec![String::new(), json_response(models)]);
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"provider_type":"gateway"}"#,
        )
        .expect("settings");
        std::fs::write(
            directory.path().join("gateway.json"),
            format!(r#"{{"url":"{url}/","api_key":"gw-key"}}"#),
        )
        .expect("a publication");
        let catalog = list_models(&setup_in(directory.path()), directory.path());
        assert_eq!(catalog.error, None);
        let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-fable-5",
                "claude-opus-5",
                "gpt-6.1-sol",
                "grok-4.7",
                "gpt-image-2"
            ]
        );
        assert_eq!(catalog.models[3].description.as_deref(), Some("xAI"));
        assert_eq!(
            catalog.models[4].description.as_deref(),
            Some("OpenAI, makes media")
        );
        assert_eq!(catalog.first(), Some("claude-fable-5"));
        // By provider, as the web cockpit's picker shows them; media only by search.
        let grouped: Vec<(&str, &str, Vec<&str>)> = catalog
            .providers
            .iter()
            .map(|provider| {
                (
                    provider.id.as_str(),
                    provider.name.as_str(),
                    provider.models.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            grouped,
            [
                (
                    "anthropic",
                    "Anthropic",
                    vec!["claude-fable-5", "claude-opus-5"]
                ),
                ("openai", "OpenAI", vec!["gpt-6.1-sol"]),
                ("xai", "xAI", vec!["grok-4.7"]),
            ]
        );
        assert_eq!(
            catalog.named, None,
            "the gateway's models are chosen in a composer"
        );
        let _probe = requests.recv().expect("the probe");
        let request = requests.recv().expect("the request");
        assert!(
            request.starts_with("GET /v1/models HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("Authorization: Bearer gw-key\r\n"),
            "{request}"
        );
    }

    #[test]
    fn an_endpoint_s_chunked_model_list_is_read_and_a_refusal_is_explained() {
        let directory = tempfile::tempdir().expect("a directory");
        let chunked = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n{\"data\":[{\"id\":\"\r\n9\r\nmodel-a\"}\r\n2\r\n]}\r\n0\r\n\r\n";
        let (url, _requests) = serve(vec![
            chunked.to_owned(),
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_owned(),
        ]);
        let setup = Setup {
            provider: Some("openai".into()),
            env: vec![
                (BASE_URL.into(), format!("{url}/v1")),
                (API_KEY.into(), "k".into()),
                (MODEL.into(), "model-a".into()),
                ("KOU_CONVEYOR_CONTEXT_WINDOW".into(), "200k".into()),
            ],
            ..Setup::default()
        };
        let catalog = list_models(&setup, directory.path());
        assert_eq!(catalog.error, None);
        assert_eq!(catalog.models[0].id, "model-a");
        assert_eq!(catalog.named.as_deref(), Some("model-a"));
        assert_eq!(catalog.context_window, Some(200_000));

        let catalog = list_models(&setup, directory.path());
        assert!(catalog.models.is_empty());
        assert_eq!(
            catalog.error.as_deref(),
            Some("the endpoint rejected the API key (401)")
        );
    }

    #[test]
    fn curl_gets_a_url_with_its_headers_out_of_the_process_list() {
        if Command::new("curl").arg("--version").output().is_err() {
            eprintln!("skipped: no curl here");
            return;
        }
        // HTTPS endpoints go through curl; plain HTTP shows it works the same.
        let (url, requests) = serve(vec![
            json_response(r#"{"data":[{"id":"model-a"}]}"#),
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned(),
        ]);
        let headers = [("Authorization", "Bearer a \"quoted\" key".to_owned())];
        let (status, body) = curl_get(&format!("{url}/v1/models"), &headers).expect("an answer");
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"data":[{"id":"model-a"}]}"#);
        let request = requests.recv().expect("the request");
        assert!(
            request.starts_with("GET /v1/models HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("Authorization: Bearer a \"quoted\" key\r\n"),
            "{request}"
        );
        let (status, body) = curl_get(&format!("{url}/missing"), &headers).expect("an answer");
        assert_eq!((status, body.as_slice()), (404, b"".as_slice()));
    }

    #[test]
    fn preferences_and_meta_keep_what_other_cockpits_wrote() {
        let directory = tempfile::tempdir().expect("a directory");
        let setup = setup_in(directory.path());
        std::fs::write(
            directory.path().join("preferences.json"),
            r##"{"effort":"max","accent":"pink","radius":12}"##,
        )
        .expect("preferences");
        Preferences::save(&setup, "last_model", "claude-opus-5").expect("saved");
        let saved: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("preferences.json")).expect("a file"),
        )
        .expect("JSON");
        assert_eq!(
            saved,
            serde_json::json!({"effort":"max","accent":"pink","radius":12,"last_model":"claude-opus-5"})
        );
        let preferences = Preferences::load(&setup);
        assert_eq!(
            (preferences.effort.as_str(), preferences.last_model.as_str()),
            ("max", "claude-opus-5")
        );

        let sessions = directory.path().join("sessions");
        Meta::save_effort(&sessions, "s1", "xhigh").expect("saved");
        std::fs::write(
            sessions.join(".meta/s2.json"),
            r#"{"title":"  Issue #2 \n OSC 7501 ","effort":"extreme","pinned":true}"#,
        )
        .expect("meta");
        assert_eq!(Meta::load(&sessions, "s1").effort, "xhigh");
        let meta = Meta::load(&sessions, "s2");
        assert_eq!(meta.shown_title(), Some("Issue #2 OSC 7501"));
        assert_eq!(meta.effort, "");
        Meta::save_effort(&sessions, "s2", "low").expect("saved");
        let saved: Value =
            serde_json::from_slice(&std::fs::read(sessions.join(".meta/s2.json")).expect("a file"))
                .expect("JSON");
        assert_eq!(saved["pinned"], true);
        assert_eq!(saved["effort"], "low");
    }

    #[cfg(unix)]
    #[test]
    fn a_managed_gateway_is_leased_for_the_run() {
        use std::os::unix::net::UnixListener;
        let directory = tempfile::tempdir().expect("a directory");
        let socket = directory.path().join("lease.sock");
        let listener = UnixListener::bind(&socket).expect("a socket");
        let (asked, requests) = std::sync::mpsc::channel();
        let (closed, closes) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("a client");
            let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
            let mut request = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            asked.send(request).expect("a listener");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Kou-Gateway-Owner: owner-1\r\n\r\n",
                )
                .expect("an answer");
            // The lease lasts until the client closes the connection.
            let mut rest = Vec::new();
            let _ = reader.read_to_end(&mut rest);
            closed.send(()).expect("a listener");
        });
        let (url, _requests) = serve(vec![String::new()]);
        std::fs::write(
            directory.path().join("settings.json"),
            r#"{"provider_type":"gateway","model":"gpt-x"}"#,
        )
        .expect("settings");
        std::fs::write(
            directory.path().join("gateway.json"),
            format!(
                r#"{{"url":"{url}","api_key":"gw-key","owner":"owner-1","lease_socket":"{}"}}"#,
                socket.display()
            ),
        )
        .expect("a publication");
        let launch = launch(&setup_in(directory.path()), directory.path(), "").expect("a launch");
        let request = requests.recv().expect("the lease request");
        assert!(
            request.starts_with("GET /internal/lease HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("Authorization: Bearer gw-key\r\n"),
            "{request}"
        );
        assert!(
            request.contains("X-Kou-Gateway-Owner: owner-1\r\n"),
            "{request}"
        );
        assert!(launch.lease.is_some());
        assert!(
            closes.recv_timeout(Duration::from_millis(200)).is_err(),
            "the lease is held while the launch lives"
        );
        drop(launch);
        closes
            .recv_timeout(Duration::from_secs(5))
            .expect("the lease ends with the run");
    }
}
