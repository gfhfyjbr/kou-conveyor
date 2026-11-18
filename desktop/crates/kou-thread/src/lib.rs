//! The conversation every native agent pane renders, and the two channels a
//! backend drives it through.
//!
//! A backend (an ACP agent, the kou runner) is a pair of channels: it reads
//! [`Command`]s and writes [`Update`]s. It owns its own threads and processes;
//! nothing here depends on an async runtime, so GPUI's executor, a test or a
//! plain thread can sit on either end. The shapes follow the Agent Client
//! Protocol's session updates, which the kou runner's events also map onto.

pub mod elicitation;
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

/// A surface a tool call made: a small UI the agent shows the user (kou's
/// generative UI). Its tool writes each version of a surface to a file of
/// the workspace and names it on the first line of what it answers,
/// `UI <surface> v<N> (<path>)`, with ` deleted` before the path when the
/// call took the surface away; the surface as text follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceRef {
    /// The surface's id, of the agent's own choosing.
    pub surface: String,
    /// Which version of it the call made, counted from 1.
    pub version: u32,
    /// The call took the surface away.
    pub deleted: bool,
    /// The version's file, as a path in the workspace.
    pub path: String,
}

impl SurfaceRef {
    /// The surface named on the first line of `output`, if one is.
    pub fn of(output: &str) -> Option<SurfaceRef> {
        // (A harness that shows a tool's output as a fenced block.)
        let output = output.trim_start();
        let output = match output.strip_prefix("```") {
            Some(fenced) => fenced.split_once('\n').map_or("", |(_, rest)| rest),
            None => output,
        };
        let line = output.lines().next()?.trim_end();
        let rest = line.strip_prefix("UI ")?;
        let (surface, rest) = rest.split_once(" v")?;
        let named = !surface.is_empty()
            && surface.len() <= 48
            && surface.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
            })
            && !surface.starts_with(['-', '_']);
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let version: u32 = rest[..digits].parse().ok()?;
        let rest = &rest[digits..];
        let (deleted, rest) = match rest.strip_prefix(" deleted") {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let path = rest.strip_prefix(" (")?.strip_suffix(')')?;
        let inside = path.starts_with(".harness/ui/")
            && path.ends_with(".json")
            && !path.split('/').any(|part| part == ".." || part.is_empty())
            && path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte));
        (named && version > 0 && inside).then(|| SurfaceRef {
            surface: surface.to_owned(),
            version,
            deleted,
            path: path.to_owned(),
        })
    }
}

/// A session's title without what an agent's own bookkeeping put after it.
///
/// Claude's SDK names a session forked from another "<title> (fork)", and
/// an edit of a prompt already sent runs in such a fork, so each edit would
/// add one more to the end.
pub fn plain_title(title: &str) -> &str {
    let mut title = title.trim();
    while let Some(rest) = title.strip_suffix("(fork)") {
        title = rest.trim_end();
    }
    title
}

impl ToolCall {
    /// The surface this call made, when what it answered names one.
    pub fn surface(&self) -> Option<SurfaceRef> {
        self.content.iter().find_map(|content| match content {
            ToolContent::Terminal { output, .. } => SurfaceRef::of(output),
            ToolContent::Text(text) => SurfaceRef::of(text),
            _ => None,
        })
    }
}

#[cfg(test)]
mod title_tests {
    use super::plain_title;

    #[test]
    fn a_title_loses_the_marks_an_agent_puts_on_a_fork_however_many() {
        assert_eq!(
            plain_title("Rust status (fork) (fork) (fork)"),
            "Rust status"
        );
        assert_eq!(plain_title("  Rust status (fork)  "), "Rust status");
        assert_eq!(plain_title("A fork (of a road)"), "A fork (of a road)");
        assert_eq!(plain_title("(fork)"), "");
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;

    #[test]
    fn a_call_names_the_surface_it_made_on_the_first_line_of_what_it_answers() {
        let made = SurfaceRef::of(
            "UI bench v3 (.harness/ui/s-1/bench/v3.json)\nwarning: x\n\n◇ Benchmarks",
        );
        assert_eq!(
            made,
            Some(SurfaceRef {
                surface: "bench".into(),
                version: 3,
                deleted: false,
                path: ".harness/ui/s-1/bench/v3.json".into(),
            })
        );
        let gone = SurfaceRef::of("UI bench v4 deleted (.harness/ui/s-1/bench/v4.json)").unwrap();
        assert!(gone.deleted && gone.version == 4);
        // A harness that shows a tool's output as a fenced block.
        let fenced =
            SurfaceRef::of("```console\nUI a_b-1 v1 (.harness/ui/local/a_b-1/v1.json)\n```");
        assert_eq!(fenced.unwrap().surface, "a_b-1");
        // Nothing that merely looks like it, and no path out of its folder.
        for other in [
            "",
            "UI needs a surface",
            "the UI bench v3 (.harness/ui/s/bench/v3.json)",
            "UI Bench v3 (.harness/ui/s/Bench/v3.json)",
            "UI bench v0 (.harness/ui/s/bench/v0.json)",
            "UI bench v3 (.harness/ui/../../etc/passwd.json)",
            "UI bench v3 (/etc/passwd.json)",
            "UI bench v3 (.harness/ui/s/bench/v3.txt)",
            "UI bench v3 (.harness/ui/s/bench/v3.json) and more",
        ] {
            assert_eq!(SurfaceRef::of(other), None, "{other}");
        }
        // The call's own content is where it is looked for.
        let call = ToolCall {
            content: vec![
                ToolContent::Text("not it".into()),
                ToolContent::Text("UI bench v1 (.harness/ui/s/bench/v1.json)".into()),
            ],
            ..ToolCall::default()
        };
        assert_eq!(call.surface().unwrap().version, 1);
        assert_eq!(ToolCall::default().surface(), None);
    }
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

/// One answer on offer to a [`Question`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// The answer, as it is shown and as it is sent back.
    pub label: String,
    /// What choosing it means, when the agent said.
    pub description: Option<String>,
}

/// One thing an agent asks the user, with answers to choose from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// A word or two for it, when the agent gave one: "Auth method".
    pub header: Option<String>,
    /// The question, as the agent put it.
    pub text: String,
    /// The answers on offer; none for a question answered only in the
    /// user's own words.
    pub options: Vec<QuestionOption>,
    /// Several of the options may be chosen.
    pub multiple: bool,
    /// An answer in the user's own words is taken, beside the options or in
    /// their place.
    pub free: bool,
}

/// The backend asks the user something before a tool call runs: whether it
/// may, or (with [`questions`](Self::questions)) what the call itself asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Identifies the request in [`Command::Permit`] and [`Command::Answer`];
    /// unique within a thread.
    pub id: u64,
    /// The call in question. If the thread does not have it yet, it is added.
    pub tool: ToolCall,
    /// The answers on offer. Of a request with questions, the ways to leave
    /// them unanswered (a refusal), if any.
    pub options: Vec<PermissionOption>,
    /// What the call asks the user, when it is one that asks: each is
    /// answered, and the answers go back together with [`Command::Answer`].
    /// Empty for a plain request for leave.
    #[serde(default)]
    pub questions: Vec<Question>,
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

/// Permission presets available for this account and working directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    pub choices: Vec<Choice>,
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirm: Vec<String>,
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
    /// The backend takes a [`Update::Permission`] back unanswered, by its
    /// id: it was answered elsewhere, or what it was asked for stopped.
    Withdrawn(u64),
    /// The slash commands on offer, replacing the previous list.
    Commands(Vec<Choice>),
    /// The mode changed.
    Mode(String),
    /// The permissions offered and the preset currently in force.
    Access(Access),
    /// The model changed.
    Model(String),
    /// How hard the model in force may be asked to think: the levels on
    /// offer, lowest first, replacing any said before (none: it has no such
    /// choice), and the one in force. Said after [`Update::Ready`] and again
    /// when the model or the level changes, or never.
    Efforts {
        /// The levels, lowest first.
        levels: Vec<Choice>,
        /// The level in force, when the backend knows it.
        current: Option<String>,
    },
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
    /// A prompt sent now is taken at once though a turn is open: the agent
    /// has given its answer and only waits for, or works on, what it left
    /// running in the background. Nothing is stopped to send one. A backend
    /// says so anew each time this changes.
    pub listening: bool,
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
    /// Answer the questions of a permission request
    /// ([`PermissionRequest::questions`]).
    Answer {
        /// [`PermissionRequest::id`].
        request: u64,
        /// One list for each question, in their order: the labels of the
        /// options chosen, then the user's own words, if they gave any.
        /// An empty list leaves that question unanswered.
        answers: Vec<Vec<String>>,
    },
    /// Switch mode.
    SetMode(String),
    /// Select a permission preset for subsequent turns.
    SetAccess(String),
    /// Switch model.
    SetModel(String),
    /// Switch the level of [`Update::Efforts`], by its id.
    SetEffort(String),
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
