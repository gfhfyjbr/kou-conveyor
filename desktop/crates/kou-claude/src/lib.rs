//! Claude Code as a conversation's backend, with nothing between the app and
//! Claude Code itself.
//!
//! The app's other chats go through the Agent Client Protocol and an adapter
//! (`kou-acp`). For Claude Code that was three programs deep (the adapter,
//! the Agent SDK it wraps, then `claude`), and the protocol in the middle has
//! no words for what Claude Code does: work left running in the background,
//! a message taken into the turn that runs, a session that goes on by itself
//! when such work ends. The adapter made those up as private extensions and
//! held turns open to cover the rest, and the app guessed at what it was not
//! told. This crate speaks to `claude` the way the Agent SDK does, over its
//! own stream of JSON lines, where all of that is said outright.
//!
//! # The protocol
//!
//! `claude --output-format stream-json --input-format stream-json --verbose`
//! reads one JSON object a line on its input and writes one a line on its
//! output. Its shapes are the Agent SDK's types (`sdk.d.ts` of
//! `@anthropic-ai/claude-agent-sdk`, `SDKMessage` and the `SDKControl*`
//! types); how they are used is the open-source Python SDK's
//! (`_internal/transport/subprocess_cli.py` for the command line and the
//! environment, `_internal/query.py` for the exchange). In short:
//!
//! - **Out of Claude Code:** `system` (`init` with the session's id, then
//!   `task_started` / `task_updated` / `task_notification` for what runs in
//!   the background, `session_state_changed`, `compact_boundary`, ...),
//!   `stream_event` (an answer as it is written: text, thinking and tool
//!   input deltas; asked for with `--include-partial-messages`), `assistant`
//!   (a whole message: text, thinking, `tool_use` blocks), `user` (the tools'
//!   results, and prompts echoed back with `--replay-user-messages`),
//!   `result` (a cycle of the model's is over: why, usage, cost, and
//!   `origin` when the cycle was the model's own). Every message of a
//!   subagent carries `parent_tool_use_id`, the call that started it.
//! - **Into Claude Code:** `user` messages (a prompt; one written while a
//!   cycle runs is taken into it, as one typed into Claude Code's own
//!   interface is), and `control_request`s (`initialize`, `interrupt`,
//!   `set_model`, `set_permission_mode`, `get_context_usage`, ...), each
//!   answered by a `control_response` with the same `request_id`.
//! - **Asked by Claude Code:** `control_request` with `can_use_tool`, when a
//!   tool needs leave (`--permission-prompt-tool stdio`); the answer is a
//!   `control_response` that allows (with the input, and rules to remember
//!   when the user chose "always") or denies.
//!
//! The stream is not a published, frozen interface: it is what the SDKs
//! speak. A Claude Code that changes it is met by `tests/` here failing
//! against recorded streams, and by this crate ignoring what it does not
//! know (every unknown message and field is skipped, never an error).
//!
//! # What a turn is here
//!
//! With `CLAUDE_CODE_SDK_READS_SESSION_STATE=1` in its environment Claude
//! Code says `session_state_changed`: `running` while it works or anything
//! it started still runs or is still to be dealt with, `idle` once nothing
//! more is owed. That is the conversation's turn: begun at a prompt (or at
//! `running` when Claude Code goes on by itself), over at `idle`. A `result`
//! inside it says the model has given its answer; from then until `idle`
//! the turn is open only for what runs in the background, and the session
//! is *listening* ([`kou_thread::Offers::listening`]): a prompt sent then
//! goes at once and stops nothing.
//!
//! # Records
//!
//! Claude Code keeps each conversation as a file of JSON lines under its
//! home (`projects/<the folder's path with every character that is not a
//! letter or a digit made a dash>/<session id>.jsonl`). [`records`] reads
//! them: the list of a folder's conversations, and a conversation replayed
//! as the updates it would have been live. The same conversion of a tool's
//! call and result serves both ([`records::tools`]).

pub mod host;
pub mod records;

mod live;

use std::path::PathBuf;

pub use live::{connect, models};
pub use records::{PastSession, sessions};

/// An MCP server a session is started with, run by Claude Code as a child
/// that speaks on its standard input and output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// What a session is opened with.
#[derive(Clone, Debug)]
pub struct Options {
    /// The `claude` executable.
    pub program: PathBuf,
    /// The folder the session works in.
    pub cwd: PathBuf,
    /// Variables added to the app's own environment for it: the account's
    /// home and proxy, the pane's own. (The marks of a Claude Code session
    /// the app itself may have been started from are never passed on:
    /// `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SSE_PORT`.)
    pub env: Vec<(String, String)>,
    /// Claude Code's home, when it is not its own (`CLAUDE_CONFIG_DIR` of
    /// `env`): where the conversation's record is read from.
    pub home: Option<PathBuf>,
    /// The conversation to go on with, by its session id. Its record is
    /// replayed before anything else.
    pub resume: Option<String>,
    /// The model to start with; `None` is Claude Code's own choice.
    pub model: Option<String>,
    /// MCP servers to start the session with.
    pub mcp_servers: Vec<McpServer>,
    /// Text added to Claude Code's system prompt.
    pub instructions: Option<String>,
    /// Plugins to load from folders (`--plugin-dir`).
    pub plugins: Vec<PathBuf>,
    /// A note put before the first prompt of a new conversation, for the
    /// agent and not shown as the user's.
    pub preface: Option<String>,
    /// Keep Claude Code in a process of its own ([`host`]), so that it
    /// outlives the app: a session let go of without being closed works
    /// on, and one opened with the same keeper finds it again. `None`
    /// starts it as a child, which ends with the session.
    pub keep: Option<Keeper>,
}

/// How a session's Claude Code is kept apart from the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keeper {
    /// The program that is the host ([`host::run`]), and the words before
    /// the host's own arguments: the app's own binary and its subcommand.
    pub host: PathBuf,
    pub host_args: Vec<String>,
    /// Where its sockets go: a path without an ending, the session's own.
    /// A process of the session's listens at `<stem>-<n>.sock`, `n` counted
    /// from 1 (an edit of a sent prompt moves the session to a new
    /// process). The whole path must fit a Unix socket's address, about a
    /// hundred bytes.
    pub stem: PathBuf,
    /// How long a Claude Code that only waits is kept with nobody there.
    pub idle: std::time::Duration,
    /// How long one is kept with nobody there, whatever it does.
    pub away: std::time::Duration,
}

/// The models Claude Code offers an account, asked of it before any
/// conversation is opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    /// The models, in Claude Code's order.
    pub models: Vec<kou_thread::Choice>,
    /// The one a new conversation starts with.
    pub current: Option<String>,
}

impl Options {
    /// A session of `program` in `cwd`, with nothing else said.
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            cwd: cwd.into(),
            env: Vec::new(),
            home: None,
            resume: None,
            model: None,
            mcp_servers: Vec::new(),
            instructions: None,
            plugins: Vec::new(),
            preface: None,
            keep: None,
        }
    }
}
