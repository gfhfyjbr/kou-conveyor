//! An MCP server that lets an agent in a pane drive the desktop app, and the
//! guidance that teaches it how.
//!
//! The app starts every harness with this server attached for that launch
//! only (`kou-desktop mcp`). The server speaks the Model Context Protocol on
//! stdio and turns each tool call into requests on the app's socket, as the
//! command line client does: [`serve`] is the whole server, [`Env`] is what it
//! knows about the pane it serves.
//!
//! The know-how comes in two flavours, one per means an agent can be given:
//! the server's tools, or the app's command line through the agent's own
//! shell (no server attached). Both are rendered from one source
//! ([`Flavour`]): short instructions, which the server returns to every client
//! and the app puts into the context at launch, and a long guide.
//! [`write_bundle`] writes each flavour as a brief, one plain Markdown file
//! and a Claude Code plugin holding the guide as a skill.

mod app;
mod bundle;
mod guidance;
mod rpc;
#[cfg(test)]
mod tests;
mod tools;

pub use bundle::{Bundle, Guidance, PLUGIN_NAME, SKILL_NAME, write_bundle};
pub use guidance::Flavour;
pub use rpc::{LEGACY_VERSIONS, MODERN_VERSIONS};

use std::io::{Read, Write};
use std::path::PathBuf;

/// What the server needs from the pane it serves: the variables every pane's
/// process is started with, and the directory relative paths are taken from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    /// The app's socket: `$KOU_SOCKET`, or the app's default path.
    pub socket: PathBuf,
    /// The pane the server's agent runs in: `$KOU_PANE`. Requests that need a
    /// default (a spawn's parent, "my tab") take it from here.
    pub pane: Option<String>,
    /// The session of that pane: `$KOU_SESSION`.
    pub session: Option<String>,
    /// The server's working directory, which is its harness's: a relative
    /// directory given to `start_agent` is taken from here.
    pub cwd: PathBuf,
}

impl Env {
    /// Read the environment of the current process.
    pub fn from_process() -> Env {
        let var = |name| {
            std::env::var(name)
                .ok()
                .map(|value: String| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Env {
            socket: kou_ipc::socket_path(),
            pane: var(kou_ipc::env::PANE),
            session: var(kou_ipc::env::SESSION),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// Serve MCP on a pair of streams, one JSON-RPC message per line, until
/// `input` ends. Returns the process's exit status: 0 when the client closed
/// the input, 1 when reading or writing failed.
///
/// Each tool call runs on a thread of its own, so a long `wait_for_panes`
/// does not hold up `ping` or other calls. Calls still running when the input
/// ends are cancelled, and `serve` returns once they have let go.
pub fn serve(input: impl Read, output: impl Write + Send, env: &Env) -> i32 {
    rpc::serve(input, output, env, &rpc::Timing::default())
}
