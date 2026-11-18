//! kou-conveyor's own agent as a [`kou_thread::Backend`].
//!
//! The agent stays in Go: this crate starts `kou-conveyor-runner` for each
//! prompt the way the terminal cockpit does, folds the JSONL the runner
//! writes into [`kou_thread::Update`]s, and lists and replays the sessions a
//! project keeps in `.harness/sessions`. The session files, the advisory
//! locks, the saved connection and the preferences are the Go cockpits' own,
//! so the terminal cockpit, the web cockpit and this backend can take turns
//! on one session.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! let runner = kou_native::locate_runner().expect("kou-conveyor-runner is built");
//! let backend = kou_native::connect(kou_native::Options {
//!     runner,
//!     cwd: std::env::current_dir()?,
//!     session: None,
//!     model: None,
//! })?;
//! backend
//!     .commands
//!     .send_blocking(kou_thread::Command::Prompt(vec![kou_thread::Block::Text(
//!         "Explain the test setup".into(),
//!     )]))?;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod connection;
mod emit;
#[cfg(test)]
mod fixtures;
mod fold;
mod present;
mod rewind;
mod runner;
mod session;
mod sessions;
mod text;
mod transcript;
mod wire;

pub use sessions::{SessionSummary, sessions};

/// The models a session of kou's agent can run with, before there is one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    /// The models of the connection runs use, the best known providers
    /// first, the newest first.
    pub models: Vec<kou_thread::Choice>,
    /// Who serves them, each with its models that an agent runs on.
    pub providers: Vec<kou_thread::Provider>,
    /// The one a new session starts with: the model last chosen in any
    /// cockpit, else the one the connection names, else its default.
    pub current: Option<String>,
}

/// What a new session in `project` would offer: the connection's list of
/// models, asked of the gateway or the endpoint the user's settings name. A
/// request for a list; nothing a model answers. Blocks while it is fetched:
/// call it off the main thread.
pub fn models(project: &std::path::Path) -> Offered {
    let setup = connection::Setup {
        settings_file: connection::default_settings_file(),
        provider: None,
        env: Vec::new(),
    };
    let catalog = connection::list_models(&setup, project);
    let last = Some(connection::Preferences::load(&setup).last_model).filter(|m| !m.is_empty());
    let current = last
        .or_else(|| catalog.named.clone())
        .or_else(|| catalog.fallback.clone())
        .or_else(|| catalog.first().map(str::to_owned));
    Offered {
        models: catalog.models,
        providers: catalog.providers,
        current,
    }
}

use std::path::PathBuf;

/// Finds the runner: `$KOU_RUNNER` (or the cockpits' `$KOU_CONVEYOR_RUNNER`),
/// then `PATH`, then `bin/` of the repository the executable or the working
/// directory is in.
pub fn locate_runner() -> Option<PathBuf> {
    runner::locate()
}

/// What a session is opened with.
#[derive(Clone, Debug)]
pub struct Options {
    /// The `kou-conveyor-runner` executable, as [`locate_runner`] finds it.
    pub runner: PathBuf,
    /// The project directory the session belongs to: the agent's workspace,
    /// whose `.harness/sessions` holds the session.
    pub cwd: PathBuf,
    /// Resume this session; a new one otherwise. An ID no session has starts
    /// a new session under it, as in the cockpits.
    pub session: Option<String>,
    /// The model of the session's prompts. `None` goes on with the model the
    /// session's last prompt ran with, or for a new session with the model
    /// last chosen in any cockpit, else the connection's.
    pub model: Option<String>,
}

/// Where the cockpits' files are and what a launch adds to the runner's
/// environment. [`connect`] uses [`Setup::user`]; tests and embedders that
/// bring their own connection use [`connect_with`].
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// The cockpits' `settings.json`, beside which `preferences.json`,
    /// `gateway.json` and the user's plugins live. `None` leaves the
    /// connection to the environment and keeps no preferences.
    pub settings_file: Option<PathBuf>,
    /// A runner provider (`openai`, `anthropic`, ...) configured by the
    /// environment, which takes precedence over the saved connection, as the
    /// cockpits' `-provider` flag does.
    pub provider: Option<String>,
    /// Variables for the runner, after all the others.
    pub env: Vec<(String, String)>,
}

impl Setup {
    /// The user's own setup: `$KOU_CONVEYOR_CONFIG`, or `settings.json` in
    /// `kou-conveyor` of the user's configuration directory, which is where
    /// both Go cockpits keep it.
    pub fn user() -> Self {
        Self {
            settings_file: connection::default_settings_file(),
            ..Self::default()
        }
    }
}

/// Opens a session of kou's own agent with the user's saved connection.
///
/// Returns at once; threads of the backend's own do the rest. When resuming,
/// the stored conversation is replayed as updates before
/// [`kou_thread::Update::Ready`]; a run a cockpit left running apart from
/// itself is taken up and followed.
pub fn connect(options: Options) -> anyhow::Result<kou_thread::Backend> {
    connect_with(options, Setup::user())
}

/// [`connect`] with a setup of the caller's choosing.
pub fn connect_with(options: Options, setup: Setup) -> anyhow::Result<kou_thread::Backend> {
    session::connect(
        options,
        connection::Setup {
            settings_file: setup.settings_file,
            provider: setup.provider,
            env: setup.env,
        },
    )
}
