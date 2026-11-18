//! An Agent Client Protocol client that drives a kou-thread backend.
//!
//! [`connect`] starts an agent that speaks the [Agent Client Protocol] over its
//! standard input and output (JSON-RPC 2.0, one message per line) and presents
//! it as a [`kou_thread::Backend`]: the interface sends [`kou_thread::Command`]s
//! and reads [`kou_thread::Update`]s, and never sees the protocol.
//!
//! This is protocol version 1. The wire types come from the protocol's own
//! schema crate; the connection runs on plain threads, one that owns the
//! session's state and one each for the agent's output, its standard error, its
//! input, its exit and the interface's commands. There is no async runtime.
//!
//! The client serves the methods an agent may call back: reading and writing
//! text files, and running commands (`terminal/*`) with `std::process`, whose
//! output is mirrored into the tool call that shows the terminal.
//!
//! What [`Options`] attaches to a session (MCP servers, added instructions,
//! `_meta`) goes with every request that opens it, and only as far as the
//! agent's advertised capabilities allow. An agent that needs a login ends
//! the connection with a notice that says how; the client never logs in.
//!
//! A prompt already sent can be edited ([`kou_thread::Request::Rewind`]) when
//! the agent forks sessions up to one of its messages and [`Options::fork_meta`]
//! says how it is told which: the session is forked up to the agent's last
//! message before that prompt, or a new one is opened for the first prompt,
//! and the edited prompt runs there. The fork then is the thread's session,
//! announced with another [`kou_thread::Update::Ready`]; the session it came
//! from is left as it was, and let go of on this connection.
//!
//! [Agent Client Protocol]: https://agentclientprotocol.com

mod convert;
mod fs;
mod list;
mod process;
mod rpc;
mod selectors;
mod session;
mod terminal;
mod uri;

pub use list::{Offered, PastSession, models, sessions};

use anyhow::Context as _;
use kou_thread::Backend;
use process::Process;
use session::{Event, Session, Wiring};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
};

/// How to start an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The agent's executable.
    pub program: PathBuf,
    /// Its arguments, the ones that put it in protocol mode included.
    pub args: Vec<String>,
    /// Variables added to this process's environment for it.
    pub env: Vec<(String, String)>,
    /// The directory the process starts in. Empty leaves it where this process is.
    pub cwd: PathBuf,
}

/// What session to open on the agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The session's working directory. Must be absolute.
    pub cwd: PathBuf,
    /// Load this session instead of creating one, when the agent can. An agent
    /// that cannot, or does not know the session, gets a new one and the
    /// thread a notice.
    pub resume: Option<String>,
    /// This client's name, as told to the agent.
    pub client_name: String,
    /// This client's version, as told to the agent.
    pub client_version: String,
    /// MCP servers the agent starts for this session. They are sent with
    /// `session/new`, and again with `session/load`, `session/resume` or
    /// `session/fork`, which the protocol requires because an agent does not
    /// keep them. A server over a transport the agent does not advertise is
    /// left out with a notice.
    pub mcp_servers: Vec<McpServer>,
    /// Text added to the agent's own instructions for this session. It is sent
    /// only to an agent that advertises `sessionCapabilities.systemPrompt`, and
    /// in `append` mode; other agents never see it.
    pub instructions: Option<String>,
    /// `_meta` for `session/new`, `session/load`, `session/resume` and
    /// `session/fork`, where some agents take launch options of their own
    /// (Claude's adapter reads `claudeCode.options`, for one).
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
    /// A few lines for an agent that takes no added instructions (it does
    /// not advertise `sessionCapabilities.systemPrompt`): they go with the
    /// first prompt of a new session, marked as the app's, and the thread
    /// does not show them as the user's. An agent that takes instructions
    /// gets [`Options::instructions`] instead and never this.
    pub preface: Option<String>,
    /// What the user should run to log in, said when the agent answers that
    /// it needs a login. This client never logs in by itself.
    pub login_hint: Option<String>,
    /// The model the session is to run with, chosen before it existed: set
    /// once the session is ready, when the agent offers it and it is not
    /// the one the session came up with. An agent that does not offer it
    /// keeps its own, with a notice.
    pub model: Option<String>,
    /// How `session/fork` is told the point a fork ends at, for an agent that
    /// forks a session up to one of its messages: `_meta` with the string
    /// [`FORK_MESSAGE_ID`] where the ACP `messageId` of the last agent message
    /// to keep goes. The protocol has no field for it, so the agent's own
    /// extension is spelled out here; it is merged into [`Options::meta`].
    ///
    /// With it, and an agent that advertises `sessionCapabilities.fork`, a
    /// prompt already sent can be edited ([`kou_thread::Offers::rewind`]).
    /// An agent that forked whole sessions whatever this says would make an
    /// edit run after everything instead, so it is set only for agents known
    /// to take it.
    pub fork_meta: Option<serde_json::Map<String, serde_json::Value>>,
    /// Tells the agent that this client shows the commands the agent runs
    /// itself as terminals (the `terminal_output` extension that Zed's
    /// adapters share: `_meta.terminal_info`, `terminal_output` and
    /// `terminal_exit` on a tool call's reports). An adapter that takes it
    /// reports a command's output and exit apart from its call, and so can
    /// say that a command went on in the background after its call ended,
    /// and when it ended. Off unless set: an adapter is asked for it only
    /// when what it does with it is known.
    pub display_terminals: bool,
    /// The agent keeps a prompt's turn open after its answer for as long as
    /// what it started in the background runs, and takes the next prompt
    /// meanwhile: Claude's adapter, which ends all of that work when it is
    /// told to stop instead. A prompt sent then goes at once and the turn
    /// before it is over (see [`kou_thread::Offers::listening`]).
    pub holds_turns: bool,
}

/// What stands in [`Options::fork_meta`] for the id of the message a fork ends at.
pub const FORK_MESSAGE_ID: &str = "{messageId}";

impl Options {
    /// A new session in `cwd`, with nothing attached.
    pub fn new(
        cwd: impl Into<PathBuf>,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            client_name: client_name.into(),
            client_version: client_version.into(),
            ..Self::default()
        }
    }
}

/// An MCP server for the agent to connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServer {
    /// A program the agent starts and talks to over its standard input and
    /// output. Every agent must take this kind.
    Stdio {
        /// The name the agent shows it under and prefixes its tools with.
        name: String,
        /// The program, by absolute path.
        command: PathBuf,
        /// Its arguments.
        args: Vec<String>,
        /// Variables set for it. Agents differ in what else of their own
        /// environment a server gets, so whatever it needs belongs here.
        env: Vec<(String, String)>,
    },
    /// A server reached over streamable HTTP, for agents that advertise
    /// `mcpCapabilities.http`.
    Http {
        /// The name the agent shows it under.
        name: String,
        /// Its address.
        url: String,
        /// Headers sent with every request.
        headers: Vec<(String, String)>,
    },
}

impl McpServer {
    /// The name the agent knows it by.
    pub fn name(&self) -> &str {
        match self {
            Self::Stdio { name, .. } | Self::Http { name, .. } => name,
        }
    }
}

/// How many of the agent's last standard error lines are kept for the reason
/// given when it dies.
const LAST_WORDS: usize = 10;
/// The longest such line, in characters.
const LAST_WORD_LENGTH: usize = 400;

/// Start the agent and return at once. The handshake, the session and all that
/// follows arrive as updates; a failure after this point is an
/// [`kou_thread::Update::Closed`] with the reason.
///
/// Fails only if the process cannot be started.
pub fn connect(launch: Launch, options: Options) -> anyhow::Result<Backend> {
    anyhow::ensure!(
        options.cwd.is_absolute(),
        "the session's working directory must be absolute, got {}",
        options.cwd.display()
    );
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .envs(launch.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !launch.cwd.as_os_str().is_empty() {
        command.current_dir(&launch.cwd);
    }
    process::own_group(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {}", launch.program.display()))?;
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());

    let (events, inbox) = mpsc::channel();
    let exited = events.clone();
    let process = Process::watch(child, "kou-acp agent exit", move |exit| {
        let _ = exited.send(Event::Exited(exit));
    })
    .context("could not watch the agent process")?;
    let started = start_threads(&launch, options, pipes, events, inbox, process.clone());
    if started.is_err() {
        // Half a connection is none: do not leave the agent running.
        process.kill();
    }
    started
}

type Pipes = (
    Option<std::process::ChildStdin>,
    Option<std::process::ChildStdout>,
    Option<std::process::ChildStderr>,
);

fn start_threads(
    launch: &Launch,
    options: Options,
    pipes: Pipes,
    events: mpsc::Sender<Event>,
    inbox: mpsc::Receiver<Event>,
    process: Arc<Process>,
) -> anyhow::Result<Backend> {
    let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
        anyhow::bail!("the agent process has no pipes");
    };
    let name = launch
        .program
        .file_name()
        .unwrap_or(launch.program.as_os_str())
        .to_string_lossy()
        .into_owned();
    let (backend, end) = kou_thread::channel();
    let (outbox, lines) = mpsc::channel::<String>();
    let last_words = Arc::new(Mutex::new(VecDeque::new()));
    let spawn = |name: &str, body: Box<dyn FnOnce() + Send>| {
        thread::Builder::new()
            .name(name.into())
            .spawn(body)
            .with_context(|| format!("could not start the {name} thread"))
    };

    spawn(
        "kou-acp agent input",
        Box::new(move || write_lines(stdin, lines)),
    )?;
    {
        let (events, name) = (events.clone(), name.clone());
        spawn(
            "kou-acp agent output",
            Box::new(move || read_messages(stdout, &name, &events)),
        )?;
    }
    {
        let (events, last_words) = (events.clone(), last_words.clone());
        spawn(
            "kou-acp agent errors",
            Box::new(move || read_errors(stderr, &name, &last_words, &events)),
        )?;
    }
    {
        let (events, commands) = (events.clone(), end.commands.clone());
        spawn(
            "kou-acp commands",
            Box::new(move || {
                while let Ok(command) = commands.recv_blocking() {
                    if events.send(Event::Command(command)).is_err() {
                        return;
                    }
                }
                let _ = events.send(Event::CommandsClosed);
            }),
        )?;
    }
    {
        let (events, requests) = (events.clone(), end.requests.clone());
        spawn(
            "kou-acp requests",
            Box::new(move || {
                while let Ok(request) = requests.recv_blocking() {
                    if events.send(Event::Request(request)).is_err() {
                        return;
                    }
                }
            }),
        )?;
    }
    let session = Session::new(
        launch.clone(),
        options,
        Wiring {
            events: inbox,
            event_sender: events,
            outbox,
            updates: end.updates,
            commands: end.commands,
            requests: end.requests,
            process,
            stderr: last_words,
        },
    );
    spawn("kou-acp session", Box::new(move || session.run()))?;
    Ok(backend)
}

/// The agent's input: one message per line, flushed as it is written. Ends,
/// closing the pipe, when the session drops its sender or the agent stops reading.
fn write_lines(mut stdin: impl Write, lines: mpsc::Receiver<String>) {
    for line in lines {
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            log::debug!("the agent stopped reading: {error}");
            return;
        }
    }
}

/// The agent's output: one message per line. A line that is not one is logged
/// and skipped, as the stream after it is still good.
fn read_messages(stdout: impl Read, name: &str, events: &mpsc::Sender<Event>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                log::debug!("reading from {name} failed: {error}");
                break;
            }
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        log::trace!("from {name}: {text}");
        match rpc::parse(text) {
            Ok(messages) => {
                for message in messages {
                    if events.send(Event::Incoming(message)).is_err() {
                        return;
                    }
                }
            }
            Err(error) => log::warn!(
                "{name} wrote a line that is not a message ({error}): {}",
                clip(text, LAST_WORD_LENGTH)
            ),
        }
    }
    let _ = events.send(Event::StdoutClosed);
}

/// The agent's standard error is its log: it goes to ours, and its last lines
/// are kept to explain a death.
fn read_errors(
    stderr: impl Read,
    name: &str,
    last_words: &Mutex<VecDeque<String>>,
    events: &mpsc::Sender<Event>,
) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        if text.trim().is_empty() {
            continue;
        }
        log::debug!("{name}: {text}");
        let mut kept = last_words
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if kept.len() == LAST_WORDS {
            kept.pop_front();
        }
        kept.push_back(clip(text, LAST_WORD_LENGTH));
    }
    let _ = events.send(Event::StderrClosed);
}

/// At most `limit` characters of `text`.
fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_line_is_clipped_between_characters() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("ффффф", 5), "ффффф");
        assert_eq!(clip("ффффф!", 5), "ффффф...");
    }

    #[test]
    fn a_relative_working_directory_is_refused_before_anything_starts() {
        let error = connect(
            Launch {
                program: "/no/such/agent".into(),
                args: vec![],
                env: vec![],
                cwd: PathBuf::new(),
            },
            Options::new("relative", "test", "0"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("must be absolute"), "{error}");
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let error = connect(
            Launch {
                program: "/no/such/agent".into(),
                args: vec![],
                env: vec![],
                cwd: PathBuf::new(),
            },
            Options::new(std::env::temp_dir(), "test", "0"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "could not start /no/such/agent");
    }
}
