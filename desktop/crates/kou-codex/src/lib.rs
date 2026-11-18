//! Native Codex chats. Each pane owns an app-server connection, uses Codex
//! thread IDs in both chat and terminal, and leaves storage and sandboxing
//! to the CLI. Wire shapes come from `codex app-server generate-ts`.

mod access;
mod catalog;
mod items;
mod requests;
mod rpc;
mod session;

use anyhow::Result;
pub use catalog::{Offered, PastSession, models, sessions};
use kou_agents::Launch;
use kou_claude::host::Keep;
use kou_thread::{Backend, Update};
use std::time::{Duration, Instant};

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// Open or resume a pane on a worker thread. An explicit Close ends the
/// process. Dropping the view leaves a kept turn running.
pub fn connect(
    launch: Launch,
    resume: Option<String>,
    model: Option<String>,
    keep: Option<Keep>,
) -> Result<Backend> {
    connect_with_access(launch, resume, model, keep, None)
}

/// Apply the launcher's explicit permission choice before accepting the first prompt.
pub fn connect_with_access(
    launch: Launch,
    resume: Option<String>,
    model: Option<String>,
    keep: Option<Keep>,
    access: Option<String>,
) -> Result<Backend> {
    let (backend, end) = kou_thread::channel();
    std::thread::Builder::new()
        .name("codex chat".into())
        .spawn(move || {
            let result = futures::executor::block_on(session::run(
                &launch,
                resume,
                model,
                access,
                keep.as_ref(),
                &end,
            ));
            let _ = end.updates.try_send(Update::Closed(
                result.err().map(|error| format!("{error:#}")),
            ));
        })?;
    Ok(backend)
}
