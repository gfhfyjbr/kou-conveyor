//! The conversation every native agent pane renders, and the two channels a
//! backend drives it through.
//!
//! A backend (an ACP agent, the kou runner) is a pair of channels: it reads
//! [`Command`]s and writes [`Update`]s. It owns its own threads and processes;
//! nothing here depends on an async runtime, so GPUI's executor, a test or a
//! plain thread can sit on either end. The shapes follow the Agent Client
//! Protocol's session updates, which the kou runner's events also map onto.

pub mod files;
mod thread;

pub use thread::{Change, Entry, PendingPermission, Rewinding, Thread, ThreadState};

use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

/// A piece of a user prompt, or of a replayed user message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// Plain text, as typed.
    Text(String),
    /// An image, as its encoded bytes.
    Image {
        /// For example `image/png`.
        mime: String,
        /// The file's bytes, not base64.
        data: Arc<[u8]>,
    },
    /// A file the prompt refers to. `text` is its content when it is embedded.
    File {
        /// Absolute path.
        path: PathBuf,
        /// Embedded content, when the backend accepts embedded context.
        text: Option<String>,
    },
}

/// What a tool call does, for its icon and for grouping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reads files or data.
    Read,
    /// Changes files.
    Edit,
    /// Deletes files.
    Delete,
    /// Moves or renames files.
    Move,
    /// Searches.
    Search,
    /// Runs a command.
    Execute,
    /// Internal reasoning presented as a tool.
    Think,
    /// Fetches something from the network.
    Fetch,
    /// Anything else.
    #[default]
    Other,
}

/// Where a tool call stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Announced; input may still be streaming, or it waits for permission.
    #[default]
    Pending,
    /// Running.
    Running,
    /// Finished successfully.
    Done,
    /// Finished with an error.
    Failed,
    /// Stopped by the user or by a cancelled turn.
    Cancelled,
}

/// Something a tool call produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToolContent {
    /// Markdown.
    Text(String),
    /// A change to one file. `old` is `None` for a new file.
    Diff {
        /// Absolute path.
        path: PathBuf,
        /// The text replaced.
        old: Option<String>,
        /// The text after the change.
        new: String,
    },
    /// Output of a command the tool ran. Replaced as a whole when it grows.
    Terminal {
        /// The backend's terminal id.
        id: String,
        /// Everything written so far, escape sequences included.
        output: String,
        /// The exit status, once it has exited.
        exit: Option<i32>,
    },
    /// A picture the tool gave back: a screenshot, an image file it read.
    Image {
        /// For example `image/png`.
        mime: String,
        /// The file's bytes, not base64.
        data: Arc<[u8]>,
    },
}

/// A file position a tool call touches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Absolute path.
    pub path: PathBuf,
    /// One-based line, when known.
    pub line: Option<u32>,
}

/// A tool call, complete as of the update that carries it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The backend's id for the call; unique within a thread.
    pub id: String,
    /// A one-line description: `Read src/main.rs`, `Run cargo test`.
    pub title: String,
    /// See [`ToolKind`].
    pub kind: ToolKind,
    /// See [`ToolStatus`].
    pub status: ToolStatus,
    /// What it produced so far.
    pub content: Vec<ToolContent>,
    /// Files it touches.
    pub locations: Vec<Location>,
    /// The call's arguments as the model wrote them, for the detail view.
    pub raw_input: Option<serde_json::Value>,
    /// The call's raw result.
    pub raw_output: Option<serde_json::Value>,
    /// Set on a call that starts an agent of the agent's own (a subagent):
    /// its kind as the agent names it, empty when it names none. The call's
    /// title says what it was asked, and its content, in the end, what it
    /// reported.
    #[serde(default)]
    pub agent: Option<String>,
    /// The call that started the subagent which made this call, when a
    /// subagent made it.
    #[serde(default)]
    pub parent: Option<String>,
    /// What is known of what this call started and may have left running:
    /// a subagent (see [`ToolCall::agent`]), or a command's process.
    #[serde(default)]
    pub work: AgentWork,
}

/// What is known of what a call started: a subagent, or a command.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentWork {
    /// It was left to run in the background: the call is over once it is
    /// started, not when it is done. A subagent started so, or a command
    /// whose call ended while its process runs on.
    #[serde(default)]
    pub background: bool,
    /// What was left to run has ended, where the agent says when it does:
    /// it says so of a command, and not of a subagent.
    #[serde(default)]
    pub ended: bool,
    /// How many calls it has made so far.
    #[serde(default)]
    pub steps: usize,
    /// What it has said, in order, where the agent passes its words on.
    #[serde(default)]
    pub said: Vec<AgentNote>,
    /// The tokens it used in all, once it is done and its agent says.
    #[serde(default)]
    pub tokens: Option<u64>,
}

/// Something a subagent said between two of its calls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentNote {
    /// How many calls it had made when it said this.
    pub after: usize,
    /// Its words, as markdown.
    pub text: String,
}

/// A change to an existing tool call: `None` leaves a field alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolPatch {
    /// The call to change.
    pub id: String,
    /// New title.
    pub title: Option<String>,
    /// New kind.
    pub kind: Option<ToolKind>,
    /// New status.
    pub status: Option<ToolStatus>,
    /// Replacement for the whole content list.
    pub content: Option<Vec<ToolContent>>,
    /// Replacement for the locations.
    pub locations: Option<Vec<Location>>,
    /// Replacement for the raw input.
    pub raw_input: Option<serde_json::Value>,
    /// Replacement for the raw output.
    pub raw_output: Option<serde_json::Value>,
    /// See [`ToolCall::agent`].
    #[serde(default)]
    pub agent: Option<String>,
    /// See [`ToolCall::parent`].
    #[serde(default)]
    pub parent: Option<String>,
    /// See [`AgentWork::background`].
    #[serde(default)]
    pub background: Option<bool>,
    /// See [`AgentWork::ended`].
    #[serde(default)]
    pub ended: Option<bool>,
    /// See [`AgentWork::tokens`].
    #[serde(default)]
    pub tokens: Option<u64>,
}

/// One step of the agent's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// What the step is.
    pub text: String,
    /// Whether it is waiting, running or finished.
    pub status: PlanStatus,
}

/// Where a plan step stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Not started.
    #[default]
    Pending,
    /// Being worked on.
    Running,
    /// Finished.
    Done,
}

/// What one answer to a permission request means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// Allow this call.
    AllowOnce,
    /// Allow this and later calls like it.
    AllowAlways,
    /// Refuse this call.
    RejectOnce,
    /// Refuse this and later calls like it.
    RejectAlways,
}

/// One answer the user can give to a permission request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    /// The id to send back in [`Command::Permit`].
    pub id: String,
    /// The label the backend wants shown.
    pub label: String,
    /// What choosing it means.
    pub kind: PermissionKind,
}

/// The backend asks whether a tool call may run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Identifies the request in [`Command::Permit`]; unique within a thread.
    pub id: u64,
    /// The call in question. If the thread does not have it yet, it is added.
    pub tool: ToolCall,
    /// The answers on offer.
    pub options: Vec<PermissionOption>,
}

/// A mode, model or slash command the backend offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// The id to send back.
    pub id: String,
    /// The name to show.
    pub name: String,
    /// A longer description, when there is one.
    pub description: Option<String>,
}

/// Who serves some of the models a backend offers, for a picker that shows
/// the models by provider, as kou-conveyor's web cockpit does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Short and stable: `anthropic`, `openai`. A picker colours a provider by it.
    pub id: String,
    /// As people say it: `Anthropic`, `OpenAI`.
    pub name: String,
    /// Its models, as ids of the models [`Update::Ready`] offers, in the order
    /// to show them: the newest first.
    pub models: Vec<String>,
}

/// What the backend can take in a prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// [`Block::Image`] is accepted.
    pub images: bool,
    /// [`Block::File`] with embedded text is accepted.
    pub embedded_files: bool,
    /// An earlier session can be loaded by id.
    pub resume: bool,
}

/// Tokens and money spent, as far as the backend reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the context window right now.
    pub context_used: Option<u64>,
    /// The context window's size.
    pub context_limit: Option<u64>,
    /// Input tokens over the session.
    pub input_tokens: Option<u64>,
    /// Output tokens over the session.
    pub output_tokens: Option<u64>,
    /// Cost over the session, in US dollars.
    pub cost_usd: Option<f64>,
}

/// Why a turn ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// The agent finished.
    Finished,
    /// It ran out of output tokens or turns.
    Limit,
    /// The model refused.
    Refused,
    /// The user cancelled.
    Cancelled,
    /// It failed; the text is for the user.
    Failed(String),
}

/// How loud a notice is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    /// Something happened: context compacted, session resumed.
    Info,
    /// Something is off but the session continues.
    Warning,
    /// Something failed.
    Error,
}

/// What a backend tells the thread, in the order it happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Update {
    /// The session exists and can take a prompt. Sent first, and again when
    /// the conversation goes on in another session of the backend's: an
    /// edit ([`Request::Rewind`]) an agent runs in a fork of its session or
    /// a new one is announced so, before its [`Update::Rewound`].
    Ready {
        /// The backend's session id, usable to resume later: the latest
        /// one is the conversation's.
        session: String,
        /// What prompts may contain.
        capabilities: Capabilities,
        /// Modes on offer (plan, auto-edit, ...), possibly none.
        modes: Vec<Choice>,
        /// The mode in force.
        mode: Option<String>,
        /// Models on offer, possibly none.
        models: Vec<Choice>,
        /// The model in force.
        model: Option<String>,
    },
    /// Part of a user message, when an earlier session is replayed.
    UserChunk(Block),
    /// More of the assistant's answer, as markdown.
    AssistantChunk(String),
    /// More of the assistant's reasoning.
    ThoughtChunk(String),
    /// More of what a subagent says: the one the tool call `call` started.
    /// It is no part of the assistant's answer.
    AgentSaid {
        /// The call that started the subagent.
        call: String,
        /// More of its words, as markdown.
        text: String,
    },
    /// A new tool call.
    Tool(ToolCall),
    /// A change to a tool call already announced.
    ToolPatch(ToolPatch),
    /// The whole plan, replacing the previous one.
    Plan(Vec<PlanStep>),
    /// A tool call needs the user's answer before it runs.
    Permission(PermissionRequest),
    /// The slash commands on offer, replacing the previous list.
    Commands(Vec<Choice>),
    /// The mode changed.
    Mode(String),
    /// The model changed.
    Model(String),
    /// How the models on offer group by who serves them, replacing any earlier
    /// grouping. Sent after [`Update::Ready`], or never: a picker then groups
    /// the models by what their ids show, a `provider/` prefix, or lists them
    /// as they come. A model no provider lists is found by searching for it.
    Providers(Vec<Provider>),
    /// The session got a title.
    Title(String),
    /// New totals.
    Usage(Usage),
    /// The agent started working on a prompt.
    TurnStarted,
    /// The agent stopped working.
    TurnEnded(Stop),
    /// Something to tell the user that is not part of the conversation.
    Notice(Level, String),
    /// The backend is gone. Nothing follows. `Some` is why, when it was not asked to close.
    Closed(Option<String>),
    /// What the backend answers beyond [`Command`]: the [`Request`]s it takes and
    /// what its slash commands expect. Sent after [`Update::Ready`], or never, which
    /// offers nothing.
    Offers(Offers),
    /// The backend took the oldest [`Command::Prompt`] it has not answered
    /// yet: `Some` is the ID [`Request::Rewind`] knows that prompt by, `None`
    /// says it went to the session as no prompt (a command of the backend's,
    /// or one it turned away). A backend that offers [`Offers::rewind`]
    /// answers every prompt, in order; any other says nothing of the kind.
    PromptTaken(Option<String>),
    /// The ID [`Request::Rewind`] knows the message by that the backend just
    /// showed with [`Update::UserChunk`]s: a prompt of a session replayed, or
    /// a message the running turn took. Sent after the chunks, by a backend
    /// that offers [`Offers::rewind`].
    PromptId(String),
    /// What became of the last [`Request::Rewind`]: `Ok` with the ID of the
    /// edited prompt, which runs now in the session as it was before the
    /// prompt it replaces; `Err` with why nothing changed, for the user.
    Rewound(Result<String, String>),
}

/// What a backend answers beyond [`Command`], as it says in [`Update::Offers`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    /// [`Request::Steer`] reaches the running turn.
    pub steer: bool,
    /// [`Request::Rewind`] edits a prompt already sent: the session goes back
    /// to before it, and the edited prompt runs from there. Such a backend
    /// names its prompts ([`Update::PromptTaken`], [`Update::PromptId`]).
    pub rewind: bool,
    /// What the slash commands of [`Update::Commands`] take after their name,
    /// by command id, as the list of commands shows it: `[focus]`, `<level>`.
    pub arguments: Vec<(String, String)>,
}

/// What the user asks of a backend beyond [`Command`], on [`Backend::requests`].
/// A backend that did not offer a request in [`Update::Offers`] never reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Hand a message to the turn that is running: the agent reads it once the
    /// tool calls it is making finish, without waiting for the turn to end.
    /// When the agent takes it, the backend says so with [`Update::UserChunk`]s,
    /// in its place in the conversation. One the turn ends before is not sent.
    Steer(Vec<Block>),
    /// Edit a prompt already sent: the session goes back to how it was before
    /// the prompt `prompt` names (an ID of [`Update::PromptTaken`] or
    /// [`Update::PromptId`]), and `blocks` run from there as a new prompt.
    /// What the agent changed in files stays. `None` names no prompt of the
    /// session's (what is replaced never reached it): `blocks` simply run.
    /// Only while no turn runs. Answered with [`Update::Rewound`]. See
    /// [`Thread::rewind`] for the interface's side.
    Rewind {
        /// The prompt the session goes back to before.
        prompt: Option<String>,
        /// The edited prompt.
        blocks: Vec<Block>,
    },
}

/// What the user asks of a backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Send a prompt. Ignored by a backend that is mid-turn unless it queues.
    Prompt(Vec<Block>),
    /// Stop the running turn.
    Cancel,
    /// Answer a permission request with one of its options.
    Permit {
        /// [`PermissionRequest::id`].
        request: u64,
        /// [`PermissionOption::id`].
        option: String,
    },
    /// Switch mode.
    SetMode(String),
    /// Switch model.
    SetModel(String),
    /// End the session and the backend's processes.
    Close,
}

/// A live backend: send [`Command`]s, receive [`Update`]s. Dropping both ends closes it.
#[derive(Clone, Debug)]
pub struct Backend {
    /// To the backend. Unbounded: sending never blocks the interface.
    pub commands: async_channel::Sender<Command>,
    /// From the backend, in order. Ends after [`Update::Closed`].
    pub updates: async_channel::Receiver<Update>,
    /// To the backend, the [`Request`]s it offered in [`Update::Offers`]. Unbounded.
    pub requests: async_channel::Sender<Request>,
}

/// The backend's side of a [`Backend`], for the code that implements one.
#[derive(Clone, Debug)]
pub struct BackendEnd {
    /// From the interface.
    pub commands: async_channel::Receiver<Command>,
    /// To the interface.
    pub updates: async_channel::Sender<Update>,
    /// From the interface: what goes beyond [`Command`]. A backend that offers
    /// nothing leaves it unread.
    pub requests: async_channel::Receiver<Request>,
}

/// A connected pair of channel ends.
pub fn channel() -> (Backend, BackendEnd) {
    let (command_tx, command_rx) = async_channel::unbounded();
    let (update_tx, update_rx) = async_channel::unbounded();
    let (request_tx, request_rx) = async_channel::unbounded();
    (
        Backend {
            commands: command_tx,
            updates: update_rx,
            requests: request_tx,
        },
        BackendEnd {
            commands: command_rx,
            updates: update_tx,
            requests: request_rx,
        },
    )
}
