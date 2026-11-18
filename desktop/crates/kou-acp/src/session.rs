//! One agent connection: the thread that owns all of its state.
//!
//! Everything that happens to a connection arrives here as an [`Event`], from
//! the threads that read the agent's output, wait for processes and forward
//! the interface's commands. Handling them one at a time on one thread is what
//! keeps the updates in the order things happened.

use crate::{
    Launch, Options, convert, fs,
    process::{Exit, Process},
    rpc::{self, Incoming, RpcError},
    selectors::{Selectors, Source, Target},
    terminal::{Snapshot, Spawn, TerminalEvent, Terminals},
};
use agent_client_protocol_schema::{MaybeUndefined, ProtocolVersion, v1 as acp};
use kou_thread::{
    Block, Capabilities, Command, Level, Offers, PermissionRequest, Request, Stop, ToolCall,
    ToolContent, ToolPatch, ToolStatus, Update, Usage,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

/// Something that happened to the connection.
pub(crate) enum Event {
    /// The agent sent a message.
    Incoming(Incoming),
    /// The agent's standard output ended.
    StdoutClosed,
    /// The agent's standard error ended.
    StderrClosed,
    /// The agent's process ended.
    Exited(Exit),
    /// The interface asks for something.
    Command(Command),
    /// The interface asks for something beyond the commands.
    Request(Request),
    /// The interface dropped its sending end.
    CommandsClosed,
    /// One of the terminals has news.
    Terminal(TerminalEvent),
}

/// What [`crate::connect`] hands to the session thread.
pub(crate) struct Wiring {
    pub events: mpsc::Receiver<Event>,
    /// For the terminals this session will create.
    pub event_sender: mpsc::Sender<Event>,
    /// Lines for the agent's standard input.
    pub outbox: mpsc::Sender<String>,
    pub updates: async_channel::Sender<Update>,
    pub commands: async_channel::Receiver<Command>,
    pub requests: async_channel::Receiver<Request>,
    pub process: Arc<Process>,
    /// The last lines the agent wrote to standard error.
    pub stderr: Arc<Mutex<VecDeque<String>>>,
}

/// How long the agent gets to leave by itself once its input has ended.
const LEAVE: Duration = Duration::from_millis(250);
/// How long the agent gets to stop after being asked to, before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// How long the rest of the agent's output is awaited once it has exited or
/// stopped talking: a child it left behind can hold the pipes open.
const DRAIN: Duration = Duration::from_millis(750);
/// The shortest time between two updates of one terminal's output in the
/// thread, each of which carries the whole output.
const MIRROR_INTERVAL: Duration = Duration::from_millis(50);
/// How often a session without a command channel checks whether anyone still
/// reads its updates.
const ORPHAN_POLL: Duration = Duration::from_millis(250);
/// How long a turn the agent began by itself may say nothing, with no call
/// of its running, before it is taken to be over: an agent that never says
/// that such a turn ended would otherwise leave it open for good.
const OWN_QUIET: Duration = Duration::from_secs(120);
/// How long the agent is not yet told of a stop of a turn it began by itself.
/// A prompt that follows the stop at once was forced in by the user (the
/// interface stops the turn, then sends it): the agent takes a prompt while
/// its own work goes on, so nothing is stopped for it. Told to stop first,
/// Claude Code stops the prompt that came right after instead, and ends
/// everything it left running in the background.
/// How long an agent started in the background may say and do nothing, with
/// no call of its running, before it is taken to be done. An agent's own
/// program says when it starts one and never that one has ended (see
/// [`Session::behind`]); one that is only slow is heard of again and counts
/// as running again from then.
const AGENT_QUIET: Duration = Duration::from_secs(150);

const MAKE_WAY: Duration = Duration::from_millis(750);
/// The request that hands a message to the turn that runs, instead of a
/// `session/prompt` that waits behind it: the steering extension, which an
/// agent says it takes in its answer to `initialize`
/// (`_meta.steering.supported`). Claude's adapter does. Nothing is stopped
/// for it: the agent reads the message at its next step, and what it left
/// running goes on.
const STEER_METHOD: &str = "_session/steering";

/// The kinds of session update version 1 of the protocol defines.
const KNOWN_UPDATES: [&str; 14] = [
    "user_message_chunk",
    "agent_message_chunk",
    "agent_thought_chunk",
    "tool_call",
    "tool_call_update",
    "plan",
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "session_info_update",
    "usage_update",
    "notice",
    "compaction_update",
    "compaction_summary_chunk",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// `initialize` is on its way.
    Initializing,
    /// A session is being created, loaded or resumed.
    Opening,
    /// The session takes prompts.
    Ready,
}

/// A request to the agent that has not been answered.
#[derive(Clone, Debug)]
enum Pending {
    Initialize,
    NewSession,
    /// `session/load` or `session/resume` of the session in `Session::session`.
    Reopen,
    /// `session/prompt` of the turn of this number (see [`Turn::serial`]).
    Prompt(u64),
    SetMode(String),
    SetOption {
        target: Target,
        value: String,
    },
    /// The session an edit runs in: `session/fork`, or `session/new` for an
    /// edit of the first prompt.
    Move {
        fork: bool,
    },
    /// `session/close` of a session an edit moved away from.
    Close,
    /// [`STEER_METHOD`], with the message handed over.
    Steer(Vec<Block>),
    /// [`STEER_METHOD`] for a prompt sent into a turn the agent holds open
    /// (see `Session::prompt_into_turn`): the prompt, and the name it was
    /// given.
    Into {
        blocks: Vec<Block>,
        id: Option<String>,
    },
}

/// What came before a prompt, as far as this client saw: what the session
/// an edit of that prompt runs in holds.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Before {
    /// Nothing: the prompt opened the session.
    Nothing,
    /// The agent's message of this ACP id, and everything before it.
    Message(String),
    /// Something this client did not see or cannot name: the session was
    /// resumed without its history, the agent gave a message no id, or a
    /// message of the user's came after the agent's last.
    Unknown,
}

/// A prompt waiting for the agent.
#[derive(Debug)]
struct Queued {
    blocks: Vec<Block>,
    /// The ID an edit already gave it; `None` for one the interface sent,
    /// named when it goes out.
    id: Option<String>,
}

/// An edit waiting for the session it runs in.
#[derive(Debug)]
struct Move {
    /// The edited prompt.
    blocks: Vec<Block>,
    /// What that session holds.
    before: Before,
}

/// The prompt the agent is working on.
#[derive(Debug)]
struct Turn {
    /// `session/cancel` was sent.
    cancelled: bool,
    /// What an agent that repeats the prompt back would send, until it sends
    /// anything else.
    echo: Option<Echo>,
    /// Which of the session's prompts it answers, counted from one: the
    /// answer to another prompt is of a turn that was handed off.
    serial: u64,
    /// The agent has given its answer (a cycle of the model's ended) and
    /// began no other: the turn is open only because the agent waits for
    /// what it left running. Only for an agent that holds turns
    /// ([`crate::Options::holds_turns`]).
    answered: bool,
    /// A message was handed to the turn, and no cycle has ended since: the
    /// next to end may be the one that message cut short, which is no
    /// answer to the turn.
    steered: bool,
}

#[derive(Debug)]
struct Echo {
    text: String,
    attachments: usize,
}

/// A permission request the user has not answered.
struct OpenPermission {
    /// The agent's JSON-RPC id, to answer with.
    rpc: Value,
    request: PermissionRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chunk {
    Message,
    Thought,
}

pub(crate) struct Session {
    events: mpsc::Receiver<Event>,
    /// `None` once the agent's standard input is closed.
    outbox: Option<mpsc::Sender<String>>,
    updates: async_channel::Sender<Update>,
    commands: async_channel::Receiver<Command>,
    requests: async_channel::Receiver<Request>,
    process: Arc<Process>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    launch: Launch,
    options: Options,

    next_request: u64,
    pending: HashMap<u64, Pending>,
    stage: Stage,
    /// The agent's own name once it gave one, the program's until then.
    agent: String,
    accepts: acp::PromptCapabilities,
    auth_methods: Vec<acp::AuthMethod>,
    can_load: bool,
    can_resume: bool,
    /// The MCP transports beyond stdio the agent takes.
    mcp: acp::McpCapabilities,
    /// Whether the agent takes `systemPrompt` when a session opens.
    takes_instructions: bool,
    /// The app's note ([`Options::preface`]) goes with the next prompt: a
    /// new session of an agent that takes no added instructions.
    preface_due: bool,
    /// Whether the agent takes `session/close`.
    can_close: bool,
    /// Prompts can be edited: the agent forks sessions, and the launch says
    /// how it is told where a fork ends.
    rewinds: bool,
    /// Servers already reported as left out, so a reopen does not repeat it.
    left_out_servers: bool,
    session: Option<String>,
    /// Updates that arrived before `session/new` said which session they are
    /// for, or for the session an edit moves to before it is answered.
    early: Vec<acp::SessionNotification>,
    selectors: Selectors,
    usage: Usage,

    /// What came before a prompt sent now.
    before: Before,
    /// What came before each prompt the thread knows by an ID.
    prompts: HashMap<String, Before>,
    /// How many prompts were named.
    named: u64,
    /// The user message the thread is being shown, by the ID it is named
    /// with ([`Update::PromptId`]) once something else comes.
    showing: Option<String>,
    /// An edit waiting for the session it runs in. Prompts wait for it.
    moving: Option<Move>,

    turn: Option<Turn>,
    queue: VecDeque<Queued>,
    permissions: Vec<OpenPermission>,
    next_permission: u64,
    /// The tool calls that may still change, complete as last reported. A
    /// permission request replaces the thread's copy, so it needs all of it.
    tools: Vec<ToolCall>,
    /// The kind and message id of the latest chunk, to tell messages apart.
    last_chunk: Option<(Chunk, String)>,
    /// The subagent that spoke last (by the call that started it) and the
    /// message it was in the middle of, to set its messages apart.
    last_said: Option<(String, Option<String>)>,
    /// Compactions already announced.
    compacted: Vec<String>,

    terminals: Terminals,
    /// `terminal/wait_for_exit` requests: the terminal and the JSON-RPC id.
    exit_waiters: Vec<(String, Value)>,
    /// When each terminal's output last went to the thread.
    mirrored: HashMap<String, Instant>,
    /// Terminals with output the thread has not seen, and when to send it.
    mirror_due: HashMap<String, Instant>,
    /// The terminals the agent shows its own commands in (see
    /// [`Options::display_terminals`]), by id: what each has written, and
    /// how it ended once it has.
    shown: HashMap<String, Shown>,
    /// A turn is open that the agent began by itself, with no prompt of the
    /// user's: it works on something that came to it (a command it left
    /// running ended, say). Since when it last said anything.
    own: Option<Instant>,
    /// A turn of the agent's own was stopped, and the agent is told when
    /// this comes, unless a prompt comes first (see [`MAKE_WAY`]).
    held_stop: Option<Instant>,
    /// The agents the agent started in the background that are not known
    /// to have ended, by the call that started each, with when each was
    /// last heard of (it said something, made a call, a call of its was
    /// reported on). The agent's program reports no end for these: the turn
    /// they were started in is held open until the last is done, and till
    /// then one that ended an hour ago looks like one at work. Two things
    /// are taken for an end: the agent running a cycle of its own because
    /// something in the background ended ([`convert::cycle_origin`]), and
    /// a long silence ([`AGENT_QUIET`]).
    behind: Vec<(String, Instant)>,
    /// Those taken to have ended: heard of again, one is at work again.
    done_behind: Vec<String>,
    /// When the agent itself began to speak in the cycle that runs.
    cycle_from: Option<Instant>,
    /// How many commands left in the background ended since the last cycle
    /// was over: the agent is told of each as of an agent that ended.
    commands_ended: usize,
    /// The agent takes a message into the turn that runs ([`STEER_METHOD`]).
    steers: bool,
    /// Prompts sent to the session so far.
    turns: u64,
    /// What the interface was last told of [`Session::listens`].
    listening: bool,

    stdout_closed: bool,
    stderr_closed: bool,
    exit: Option<Exit>,
    /// When the agent was first seen to be going away on its own.
    ending: Option<Instant>,
    commands_closed: bool,
    finished: bool,
}

impl Session {
    pub fn new(launch: Launch, options: Options, wiring: Wiring) -> Self {
        let sender = wiring.event_sender;
        let terminals = Terminals::new(move |event| {
            let _ = sender.send(Event::Terminal(event));
        });
        let agent = launch
            .program
            .file_name()
            .unwrap_or(launch.program.as_os_str())
            .to_string_lossy()
            .into_owned();
        Self {
            events: wiring.events,
            outbox: Some(wiring.outbox),
            updates: wiring.updates,
            commands: wiring.commands,
            requests: wiring.requests,
            process: wiring.process,
            stderr: wiring.stderr,
            launch,
            options,
            next_request: 0,
            pending: HashMap::new(),
            stage: Stage::Initializing,
            agent,
            accepts: acp::PromptCapabilities::new(),
            auth_methods: Vec::new(),
            can_load: false,
            can_resume: false,
            mcp: acp::McpCapabilities::new(),
            takes_instructions: false,
            preface_due: false,
            can_close: false,
            rewinds: false,
            left_out_servers: false,
            session: None,
            early: Vec::new(),
            selectors: Selectors::default(),
            usage: Usage::default(),
            before: Before::Nothing,
            prompts: HashMap::new(),
            named: 0,
            showing: None,
            moving: None,
            turn: None,
            queue: VecDeque::new(),
            permissions: Vec::new(),
            next_permission: 1,
            tools: Vec::new(),
            last_chunk: None,
            last_said: None,
            compacted: Vec::new(),
            terminals,
            exit_waiters: Vec::new(),
            mirrored: HashMap::new(),
            mirror_due: HashMap::new(),
            shown: HashMap::new(),
            own: None,
            held_stop: None,
            behind: Vec::new(),
            done_behind: Vec::new(),
            cycle_from: None,
            commands_ended: 0,
            steers: false,
            turns: 0,
            listening: false,
            stdout_closed: false,
            stderr_closed: false,
            exit: None,
            ending: None,
            commands_closed: false,
            finished: false,
        }
    }

    /// Drive the connection until it is closed.
    pub fn run(mut self) {
        self.initialize();
        while !self.finished {
            let event = match self.next_deadline() {
                Some(deadline) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match self.events.recv_timeout(wait) {
                        Ok(event) => Some(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match self.events.recv() {
                    Ok(event) => Some(event),
                    Err(_) => break,
                },
            };
            let idle = event.is_none();
            if let Some(event) = event {
                self.handle(event);
            }
            self.tick(idle);
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Incoming(Incoming::Response { id, result }) => self.on_response(&id, result),
            Event::Incoming(Incoming::Request { id, method, params }) => {
                self.on_request(id, &method, params)
            }
            Event::Incoming(Incoming::Notification { method, params }) => {
                self.on_notification(&method, params)
            }
            Event::StdoutClosed => {
                self.stdout_closed = true;
                self.ending.get_or_insert_with(Instant::now);
            }
            Event::StderrClosed => self.stderr_closed = true,
            Event::Exited(exit) => {
                self.exit = Some(exit);
                self.ending.get_or_insert_with(Instant::now);
            }
            Event::Command(command) => self.on_command(command),
            Event::Request(request) => self.on_request_of_the_interface(request),
            Event::CommandsClosed => self.commands_closed = true,
            Event::Terminal(TerminalEvent::Output(id)) => self.on_terminal_output(id),
            Event::Terminal(TerminalEvent::Exited(id)) => self.on_terminal_exit(&id),
        }
    }

    /// When the loop must wake up even if nothing happens.
    fn next_deadline(&self) -> Option<Instant> {
        let mut deadlines: Vec<Instant> = self.mirror_due.values().copied().collect();
        deadlines.extend(self.ending.map(|since| since + DRAIN));
        deadlines.extend(self.own.map(|since| since + OWN_QUIET));
        deadlines.extend(self.held_stop);
        deadlines.extend(self.behind.iter().map(|(_, heard)| *heard + AGENT_QUIET));
        if self.commands_closed {
            deadlines.push(Instant::now() + ORPHAN_POLL);
        }
        deadlines.into_iter().min()
    }

    /// What is due after an event, or after a wait in which none came (`idle`).
    fn tick(&mut self, idle: bool) {
        if self.finished {
            return;
        }
        let now = Instant::now();
        let due: Vec<String> = self
            .mirror_due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.mirror(&id);
        }
        // Whether a prompt is taken with a turn open, when that changed.
        let listening = self.listens();
        if listening != self.listening {
            self.listening = listening;
            self.send(Update::Offers(self.offers()));
        }
        // An agent in the background that has been silent for long, with
        // no call of its running, is done.
        let quiet: Vec<String> = self
            .behind
            .iter()
            .filter(|(id, heard)| *heard + AGENT_QUIET <= now && !self.runs_a_call(id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in quiet {
            self.agent_behind_ended(&id);
        }
        // (One with a call still running is looked at again later.)
        for (_, heard) in &mut self.behind {
            if *heard + AGENT_QUIET <= now {
                *heard = now;
            }
        }
        // A stop of the agent's own turn that no prompt followed is a stop.
        if self.held_stop.is_some_and(|at| at <= now) {
            self.held_stop = None;
            self.stop_agent();
            let (terminals, shown) = (&self.terminals, &self.shown);
            self.tools
                .retain(|call| shows_live_terminal(call, terminals, shown));
        }
        // A turn of the agent's own that has said nothing for long, with
        // nothing of its running, is over; with a call still running it is
        // looked at again later.
        if let Some(since) = self.own
            && now >= since + OWN_QUIET
        {
            let running = self
                .tools
                .iter()
                .any(|call| matches!(call.status, ToolStatus::Pending | ToolStatus::Running));
            if running {
                self.own = Some(now);
            } else {
                self.own_turn_ends(Stop::Finished);
            }
        }
        if let Some(since) = self.ending {
            // Its last messages and last words may still be in the pipes, or
            // already read and waiting here: give up on them only when
            // nothing is left to handle.
            let drained = self.exit.is_some() && self.stdout_closed && self.stderr_closed;
            if drained || (idle && now >= since + DRAIN) {
                return self.agent_gone();
            }
        }
        // Nobody can send a command and nobody reads the updates.
        if self.commands_closed && self.updates.is_closed() {
            self.close(None);
        }
    }

    // Outgoing.

    fn send(&mut self, update: Update) {
        // A user message the thread is shown is named once its chunks end.
        if !matches!(update, Update::UserChunk(_))
            && let Some(id) = self.showing.take()
        {
            let _ = self.updates.try_send(Update::PromptId(id));
        }
        // Unbounded, so this fails only when nobody listens any more.
        let _ = self.updates.try_send(update);
    }

    fn write(&mut self, line: String) {
        log::trace!("to {}: {line}", self.agent);
        if let Some(outbox) = &self.outbox
            && outbox.send(line).is_err()
        {
            self.outbox = None;
        }
    }

    fn request(&mut self, method: &str, params: &impl Serialize, pending: Pending) {
        let id = self.next_request;
        self.next_request += 1;
        self.pending.insert(id, pending);
        self.write(rpc::request(id, method, params));
    }

    fn respond(&mut self, id: &Value, result: Result<Value, RpcError>) {
        self.write(rpc::response(id, result));
    }

    // Handshake.

    fn initialize(&mut self) {
        let capabilities = acp::ClientCapabilities::new()
            .fs(acp::FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true))
            .terminal(true)
            // This client starts no login itself, but it can tell the user
            // the exact command of a terminal login, which is worth asking for.
            .auth(acp::AuthCapabilities::new().terminal(true))
            .session(acp::ClientSessionCapabilities::new().notices(acp::NoticeCapabilities::new()));
        // An agent asked to shows the commands it runs itself as terminals.
        let capabilities = if self.options.display_terminals {
            let mut meta = Map::new();
            meta.insert("terminal_output".to_owned(), json!(true));
            capabilities.meta(meta)
        } else {
            capabilities
        };
        let request = acp::InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(capabilities)
            .client_info(acp::Implementation::new(
                self.options.client_name.clone(),
                self.options.client_version.clone(),
            ));
        self.request(
            acp::AGENT_METHOD_NAMES.initialize,
            &request,
            Pending::Initialize,
        );
    }

    fn on_initialized(&mut self, result: Result<Value, RpcError>) {
        let response = match result.and_then(parse::<acp::InitializeResponse>) {
            Ok(response) => response,
            Err(error) => {
                let reason = format!("{} did not initialize: {}", self.agent, error.describe());
                return self.close(Some(reason));
            }
        };
        if let Some(info) = response.agent_info {
            let name = info.title.unwrap_or(info.name);
            if !name.trim().is_empty() {
                self.agent = name;
            }
        }
        if response.protocol_version != ProtocolVersion::V1 {
            let reason = format!(
                "{} speaks version {} of the Agent Client Protocol, and this client only version {}",
                self.agent,
                response.protocol_version,
                ProtocolVersion::V1
            );
            return self.close(Some(reason));
        }
        let capabilities = response.agent_capabilities;
        self.accepts = capabilities.prompt_capabilities;
        self.can_load = capabilities.load_session;
        self.can_resume = capabilities.session_capabilities.resume.is_some();
        self.mcp = capabilities.mcp_capabilities;
        self.takes_instructions = capabilities.session_capabilities.system_prompt.is_some();
        self.can_close = capabilities.session_capabilities.close.is_some();
        self.rewinds =
            capabilities.session_capabilities.fork.is_some() && self.options.fork_meta.is_some();
        // The steering extension is said beside the capabilities, in the
        // answer's own `_meta`.
        self.steers = response
            .meta
            .as_ref()
            .and_then(|meta| meta.get("steering"))
            .and_then(|steering| steering.get("supported"))
            .and_then(Value::as_bool)
            == Some(true);
        self.auth_methods = response.auth_methods;
        self.stage = Stage::Opening;

        let cwd = self.options.cwd.clone();
        let names = &acp::AGENT_METHOD_NAMES;
        match self.options.resume.clone() {
            // Loading replays the conversation, so it is preferred.
            Some(id) if self.can_load => {
                // The replay says what came before.
                self.before = Before::Nothing;
                self.session = Some(id.clone());
                let request = acp::LoadSessionRequest::new(id, cwd)
                    .mcp_servers(self.mcp_servers())
                    .system_prompt(self.instructions())
                    .meta(self.options.meta.clone());
                self.request(names.session_load, &request, Pending::Reopen);
            }
            Some(id) if self.can_resume => {
                // Nothing of the conversation is shown again.
                self.before = Before::Unknown;
                self.session = Some(id.clone());
                let request = acp::ResumeSessionRequest::new(id, cwd)
                    .mcp_servers(self.mcp_servers())
                    .system_prompt(self.instructions())
                    .meta(self.options.meta.clone());
                self.request(names.session_resume, &request, Pending::Reopen);
            }
            Some(_) => {
                self.send(Update::Notice(
                    Level::Warning,
                    format!(
                        "{} cannot resume sessions, so this is a new one",
                        self.agent
                    ),
                ));
                self.new_session();
            }
            None => self.new_session(),
        }
    }

    fn new_session(&mut self) {
        self.before = Before::Nothing;
        let request = self.new_session_request();
        self.request(
            acp::AGENT_METHOD_NAMES.session_new,
            &request,
            Pending::NewSession,
        );
    }

    fn new_session_request(&mut self) -> acp::NewSessionRequest {
        acp::NewSessionRequest::new(self.options.cwd.clone())
            .mcp_servers(self.mcp_servers())
            .system_prompt(self.instructions())
            .meta(self.options.meta.clone())
    }

    /// The configured MCP servers in the protocol's shape, without those over
    /// a transport the agent did not advertise. Stdio is taken by every agent.
    fn mcp_servers(&mut self) -> Vec<acp::McpServer> {
        let mut servers = Vec::new();
        let mut left_out = Vec::new();
        for server in &self.options.mcp_servers {
            match server {
                crate::McpServer::Stdio {
                    name,
                    command,
                    args,
                    env,
                } => servers.push(acp::McpServer::Stdio(
                    acp::McpServerStdio::new(name.clone(), command.clone())
                        .args(args.clone())
                        .env(
                            env.iter()
                                .map(|(name, value)| {
                                    acp::EnvVariable::new(name.clone(), value.clone())
                                })
                                .collect(),
                        ),
                )),
                crate::McpServer::Http { name, url, headers } if self.mcp.http => {
                    servers.push(acp::McpServer::Http(
                        acp::McpServerHttp::new(name.clone(), url.clone()).headers(
                            headers
                                .iter()
                                .map(|(name, value)| {
                                    acp::HttpHeader::new(name.clone(), value.clone())
                                })
                                .collect(),
                        ),
                    ))
                }
                crate::McpServer::Http { name, .. } => left_out.push(name.clone()),
            }
        }
        if !left_out.is_empty() && !self.left_out_servers {
            self.left_out_servers = true;
            self.send(Update::Notice(
                Level::Warning,
                format!(
                    "{} cannot reach MCP servers over HTTP, so this session goes without {}",
                    self.agent,
                    left_out.join(", ")
                ),
            ));
        }
        servers
    }

    /// The added instructions, for an agent that said it takes them. The
    /// protocol forbids sending them to one that did not.
    fn instructions(&self) -> Option<acp::SystemPrompt> {
        let text = self
            .options
            .instructions
            .as_ref()
            .filter(|text| !text.trim().is_empty())?;
        if !self.takes_instructions {
            log::debug!(
                "{} takes no added instructions; they are left out",
                self.agent
            );
            return None;
        }
        Some(acp::SystemPrompt::new(vec![acp::ContentBlock::from(
            text.clone(),
        )]))
    }

    fn on_session_opened(&mut self, reopened: bool, result: Result<Value, RpcError>) {
        let error = match result {
            Ok(value) if reopened => {
                // Both answers may be `null` or carry the same optional state.
                let state = parse::<acp::LoadSessionResponse>(value).unwrap_or_default();
                return self.ready(state.modes, state.config_options);
            }
            Ok(value) => match parse::<acp::NewSessionResponse>(value) {
                Ok(response) => {
                    self.session = Some(response.session_id.0.to_string());
                    // A session that begins here: an agent that could not
                    // be given instructions is told with its first prompt.
                    self.preface_due = !self.takes_instructions && self.options.preface.is_some();
                    return self.ready(response.modes, response.config_options);
                }
                Err(error) => error,
            },
            Err(error) => error,
        };
        if error.code == rpc::AUTH_REQUIRED {
            self.send(Update::Notice(Level::Error, self.login_hint()));
            return self.close(Some(format!("{} requires authentication", self.agent)));
        }
        if reopened {
            let id = self.session.take().unwrap_or_default();
            self.tools.clear();
            self.send(Update::Notice(
                Level::Warning,
                format!(
                    "Session {id} could not be resumed ({}), so this is a new one",
                    error.describe()
                ),
            ));
            return self.new_session();
        }
        let reason = format!(
            "{} did not start a session: {}",
            self.agent,
            error.describe()
        );
        self.close(Some(reason));
    }

    fn ready(
        &mut self,
        modes: Option<acp::SessionModeState>,
        options: Option<Vec<acp::SessionConfigOption>>,
    ) {
        self.announce(modes, options);
        if self.rewinds || self.steers {
            self.send(Update::Offers(self.offers()));
        }
        // A model chosen before the session existed, once.
        if let Some(wanted) = self.options.model.take()
            && self.selectors.current(Target::Model).as_deref() != Some(wanted.as_str())
        {
            let offered = self
                .selectors
                .get(Target::Model)
                .is_some_and(|selector| selector.choices.iter().any(|choice| choice.id == wanted));
            if offered {
                self.select(Target::Model, wanted);
            } else {
                self.send(Update::Notice(
                    Level::Warning,
                    format!(
                        "{} does not offer the model {wanted}, and keeps its own",
                        self.agent
                    ),
                ));
            }
        }
        self.take_early();
        self.next_prompt();
    }

    /// Tells the thread of the session in use, just opened or moved to: its
    /// id, what it takes, its modes and models.
    fn announce(
        &mut self,
        modes: Option<acp::SessionModeState>,
        options: Option<Vec<acp::SessionConfigOption>>,
    ) {
        self.selectors = Selectors::new(modes, options);
        self.stage = Stage::Ready;
        // Whatever a replay announced is history now.
        self.tools.clear();
        self.last_chunk = None;
        let selector = |target| {
            let selector = self.selectors.get(target);
            (
                selector.map(|selector| selector.choices.clone()),
                selector.and_then(|selector| selector.current.clone()),
            )
        };
        let (modes, mode) = selector(Target::Mode);
        let (models, model) = selector(Target::Model);
        self.send(Update::Ready {
            session: self.session.clone().unwrap_or_default(),
            capabilities: Capabilities {
                images: self.accepts.image,
                embedded_files: self.accepts.embedded_context,
                resume: self.can_load || self.can_resume,
            },
            modes: modes.unwrap_or_default(),
            mode,
            models: models.unwrap_or_default(),
            model,
        });
        // An agent that offers its models in groups: a picker shows the
        // groups first.
        if !self.selectors.providers.is_empty() {
            self.send(Update::Providers(self.selectors.providers.clone()));
        }
    }

    /// Handles the updates held for a session not yet known, now that it is:
    /// those for another session are dropped.
    fn take_early(&mut self) {
        for notification in std::mem::take(&mut self.early) {
            self.on_session_notification(notification);
        }
    }

    /// How the user logs in, from what the agent advertised.
    fn login_hint(&self) -> String {
        let known = self
            .options
            .login_hint
            .as_deref()
            .map(str::trim)
            .filter(|hint| !hint.is_empty());
        if self.auth_methods.is_empty() {
            return match known {
                Some(known) => format!(
                    "{} requires authentication. {known} Then open the session again.",
                    self.agent
                ),
                None => format!(
                    "{} requires authentication but does not say how to log in. Log in with its own command line, then open the session again.",
                    self.agent
                ),
            };
        }
        let mut hint = format!(
            "{} requires authentication. Log in outside this app, then open the session again:",
            self.agent
        );
        // The harness's own login command first: it is what the user knows.
        if let Some(known) = known {
            hint.push_str("\n- ");
            hint.push_str(known);
        }
        for method in &self.auth_methods {
            hint.push_str("\n- ");
            hint.push_str(method.name());
            if let acp::AuthMethod::Terminal(terminal) = method {
                // The same program this connection runs, with the method's
                // arguments and environment added.
                let mut words: Vec<String> = terminal
                    .env
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                words.sort();
                words.push(self.launch.program.display().to_string());
                words.extend(self.launch.args.iter().cloned());
                words.extend(terminal.args.iter().cloned());
                hint.push_str(&format!(": run `{}` in a terminal", words.join(" ")));
            }
            if let Some(description) = method.description().filter(|text| !text.is_empty()) {
                hint.push_str(&format!(" ({description})"));
            }
        }
        hint
    }

    // Commands from the interface.

    fn on_command(&mut self, command: Command) {
        if self.ending.is_some() && command != Command::Close {
            // The agent is gone; the reason is on its way.
            return;
        }
        match command {
            Command::Prompt(blocks) => {
                // One right after a stop of the agent's own turn was forced
                // in: the agent takes it, and is not told to stop.
                self.held_stop = None;
                if self.steers && self.holds() {
                    return self.prompt_into_turn(blocks);
                }
                self.hand_off();
                self.queue.push_back(Queued { blocks, id: None });
                self.next_prompt();
            }
            Command::Cancel => self.cancel(),
            Command::Permit { request, option } => self.permit(request, &option),
            Command::SetMode(mode) => self.select(Target::Mode, mode),
            Command::SetModel(model) => self.select(Target::Model, model),
            // Neither is offered here: an agent's questions come as plain
            // requests for leave, and it names no levels of effort.
            Command::Answer { .. } | Command::SetEffort(_) | Command::SetAccess(_) => {}
            Command::Close => self.close(None),
        }
    }

    /// Send the next queued prompt, if the agent is free to take one.
    fn next_prompt(&mut self) {
        while self.stage == Stage::Ready && self.turn.is_none() && self.moving.is_none() {
            let Some(session) = self.session.clone() else {
                return;
            };
            let Some(queued) = self.queue.pop_front() else {
                return;
            };
            let prompt = convert::prompt(&queued.blocks, &self.accepts);
            if !prompt.left_out.is_empty() {
                self.send(Update::Notice(
                    Level::Warning,
                    format!(
                        "{} does not accept {}, so the prompt went without",
                        self.agent,
                        prompt.left_out.join(" or ")
                    ),
                ));
            }
            if prompt.content.is_empty() {
                // Nothing the agent accepts is left; try the next one.
                if self.rewinds && queued.id.is_none() {
                    self.send(Update::PromptTaken(None));
                }
                continue;
            }
            if self.rewinds && queued.id.is_none() {
                let id = self.name();
                self.send(Update::PromptTaken(Some(id)));
            }
            // A message of the user's is no point a fork can end at.
            self.before = Before::Unknown;
            let mut content = prompt.content;
            if std::mem::take(&mut self.preface_due)
                && let Some(preface) = &self.options.preface
            {
                content.insert(0, acp::ContentBlock::from(convert::preface(preface)));
            }
            self.turns += 1;
            let request = acp::PromptRequest::new(session, content);
            self.request(
                acp::AGENT_METHOD_NAMES.session_prompt,
                &request,
                Pending::Prompt(self.turns),
            );
            // A turn the agent began by itself gives way to the user's.
            if self.own.take().is_some() {
                self.send(Update::TurnEnded(Stop::Finished));
            }
            // (What was started in the background of the turn before is not
            // this turn's to watch.)
            self.behind.clear();
            self.done_behind.clear();
            self.turn = Some(Turn {
                cancelled: false,
                echo: Some(Echo {
                    text: prompt.text,
                    attachments: prompt.attachments,
                }),
                serial: self.turns,
                answered: false,
                steered: false,
            });
            self.last_chunk = None;
            self.send(Update::TurnStarted);
            // What the agent asked in the turn that gave way to this one
            // it still asks: the end of that turn took the question off
            // the screen, and it would be refused at the end of this one
            // without the user having seen it. Shown again, to be answered.
            let asked: Vec<PermissionRequest> = self
                .permissions
                .iter()
                .map(|open| open.request.clone())
                .collect();
            for request in asked {
                self.send(Update::Permission(request));
            }
        }
    }

    /// Whether a prompt sent now is taken though a turn is open: the turn
    /// is the agent's own, or one it has answered and holds open.
    fn listens(&self) -> bool {
        self.own.is_some() || self.holds()
    }

    /// Whether a prompt's turn is open that the agent has answered: it holds
    /// the turn open only for what it left running.
    fn holds(&self) -> bool {
        self.turn
            .as_ref()
            .is_some_and(|turn| turn.answered && !turn.cancelled)
    }

    /// Sends a prompt into the turn the agent holds open, where the agent
    /// takes a message there: the turn takes it up, answers it, and stays
    /// open afterwards for what the agent left running, so that work goes on
    /// showing as work of a turn that runs. (Ended and begun anew, the turn
    /// would be over for the interface while what it started still ran, and
    /// nothing would say so.)
    fn prompt_into_turn(&mut self, blocks: Vec<Block>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let prompt = convert::prompt(&blocks, &self.accepts);
        if !prompt.left_out.is_empty() {
            self.send(Update::Notice(
                Level::Warning,
                format!(
                    "{} does not accept {}, so the prompt went without",
                    self.agent,
                    prompt.left_out.join(" or ")
                ),
            ));
        }
        if prompt.content.is_empty() {
            if self.rewinds {
                self.send(Update::PromptTaken(None));
            }
            return;
        }
        let id = self.rewinds.then(|| self.name());
        if let Some(id) = &id {
            self.send(Update::PromptTaken(Some(id.clone())));
        }
        // A message of the user's is no point a fork can end at.
        self.before = Before::Unknown;
        self.last_chunk = None;
        let request = json!({
            "sessionId": session,
            "prompt": prompt.content,
            "_meta": {"steering": {"idleBehavior": "promptRequired"}},
        });
        self.request(STEER_METHOD, &request, Pending::Into { blocks, id });
        self.at_work();
    }

    /// The agent answered a prompt sent into the turn it held open.
    fn on_into(&mut self, blocks: Vec<Block>, id: Option<String>, result: Result<Value, RpcError>) {
        match &result {
            Ok(value) if value.get("outcome").and_then(Value::as_str) != Some("promptRequired") => {
                return;
            }
            // The turn was over there by then.
            Ok(_) => {}
            Err(error) => log::warn!(
                "{} did not take a prompt into the turn it holds: {}",
                self.agent,
                error.describe()
            ),
        }
        // It goes as a prompt of its own: the held turn, if it is still
        // open, is over for the interface, as it is for the agent once the
        // prompt reaches it.
        if let Some(turn) = &mut self.turn {
            turn.answered = true;
        }
        self.hand_off();
        self.queue.push_front(Queued { blocks, id });
        self.next_prompt();
    }

    /// Ends, for the interface, a turn the agent has answered and holds open
    /// only for what it left running: the user is moving on. The agent does
    /// the same when the next prompt reaches it, and answers this turn's
    /// prompt then, which is no end of the turn after it.
    fn hand_off(&mut self) {
        if self.holds() {
            self.turn = None;
            self.last_chunk = None;
            self.send(Update::TurnEnded(Stop::Finished));
        }
    }

    /// The agent itself (no agent it started) says or does something: a
    /// turn it had answered is at work again.
    fn at_work(&mut self) {
        if let Some(turn) = &mut self.turn {
            turn.answered = false;
        }
        self.cycle_from.get_or_insert_with(Instant::now);
    }

    /// Whether a call made by the agent that call `launch` started is
    /// still open: that agent is at work, however long it says nothing.
    fn runs_a_call(&self, launch: &str) -> bool {
        self.tools.iter().any(|call| {
            call.parent.as_deref() == Some(launch)
                && matches!(call.status, ToolStatus::Pending | ToolStatus::Running)
        })
    }

    /// Something of the agent that call `launch` started arrived: it is at
    /// work. One that was taken to have ended is at work again.
    fn heard_of(&mut self, launch: &str) {
        let now = Instant::now();
        if let Some((_, heard)) = self.behind.iter_mut().find(|(id, _)| id == launch) {
            *heard = now;
        } else if let Some(at) = self.done_behind.iter().position(|id| id == launch) {
            self.done_behind.remove(at);
            self.behind.push((launch.to_owned(), now));
            self.send(Update::ToolPatch(ToolPatch {
                id: launch.to_owned(),
                ended: Some(false),
                ..ToolPatch::default()
            }));
        }
    }

    /// A call that started an agent in the background is over, and the
    /// agent runs on: from here it is watched for its end.
    fn watch_behind(&mut self, call: &ToolCall) {
        let started = call.agent.is_some()
            && call.work.background
            && !call.work.ended
            && call.status == ToolStatus::Done;
        let known = |id: &String| *id == call.id;
        if started
            && !self.behind.iter().any(|(id, _)| known(id))
            && !self.done_behind.iter().any(known)
        {
            self.behind.push((call.id.clone(), Instant::now()));
        }
    }

    /// The agent that call `launch` started in the background is taken to
    /// have ended.
    fn agent_behind_ended(&mut self, launch: &str) {
        self.behind.retain(|(id, _)| id != launch);
        self.done_behind.push(launch.to_owned());
        let patch = ToolPatch {
            id: launch.to_owned(),
            ended: Some(true),
            ..ToolPatch::default()
        };
        if self.tools.iter().any(|call| call.id == launch) {
            self.merge_tool(&patch);
        }
        self.send(Update::ToolPatch(patch));
    }

    /// A cycle of the agent's own is over that it ran because something in
    /// the background had ended. A command's end is known by its terminal;
    /// otherwise it was one of the agents: the one last heard of before the
    /// cycle began (its report is the last thing an agent says) that has no
    /// call running.
    fn something_behind_ended(&mut self) {
        if self.commands_ended > 0 {
            self.commands_ended -= 1;
            return;
        }
        let before = self.cycle_from.unwrap_or_else(Instant::now);
        let ended = self
            .behind
            .iter()
            .filter(|(id, heard)| *heard <= before && !self.runs_a_call(id))
            .max_by_key(|(_, heard)| *heard)
            .map(|(id, _)| id.clone());
        if let Some(id) = ended {
            self.agent_behind_ended(&id);
        }
    }

    fn on_prompt_answered(&mut self, serial: u64, result: Result<Value, RpcError>) {
        // The answer to the prompt of a turn handed off: that turn was over
        // when the prompt after it was sent.
        if self.turn.as_ref().is_none_or(|turn| turn.serial != serial) {
            return;
        }
        let Some(turn) = self.turn.take() else {
            return;
        };
        let stop = match result {
            Ok(value) => match parse::<acp::PromptResponse>(value.clone()) {
                Ok(response) => {
                    if let Some(usage) = response.usage {
                        let add = |total: Option<u64>, more: u64| Some(total.unwrap_or(0) + more);
                        self.usage.input_tokens = add(self.usage.input_tokens, usage.input_tokens);
                        self.usage.output_tokens =
                            add(self.usage.output_tokens, usage.output_tokens);
                        self.send(Update::Usage(self.usage));
                    }
                    convert::stop(response.stop_reason)
                }
                Err(_) => Stop::Failed(match value.get("stopReason").and_then(Value::as_str) {
                    Some(reason) => format!(
                        "{} stopped for a reason this client does not know: {reason}",
                        self.agent
                    ),
                    None => format!("{} ended the turn without saying why", self.agent),
                }),
            },
            // Agents that surface their aborted model request as an error
            // still stopped because the user asked them to.
            Err(error) if error.code == rpc::REQUEST_CANCELLED || turn.cancelled => {
                log::debug!("{} ended a cancelled turn with: {error:?}", self.agent);
                Stop::Cancelled
            }
            Err(error) if error.code == rpc::AUTH_REQUIRED => Stop::Failed(self.login_hint()),
            Err(error) => Stop::Failed(error.describe()),
        };
        // The turn is over; nothing can be permitted any more.
        self.cancel_permissions();
        self.send(Update::TurnEnded(stop));
        self.last_chunk = None;
        let (terminals, shown) = (&self.terminals, &self.shown);
        self.tools
            .retain(|call| shows_live_terminal(call, terminals, shown));
        self.next_prompt();
    }

    /// The agent works with no prompt of the user's open: a turn of its own
    /// begins, unless one is open already. Claude Code does this when
    /// something it left running in the background ends: it reads what came
    /// of it and goes on. The thread shows it at work, and it can be
    /// stopped, as in any turn.
    fn own_turn_starts(&mut self) {
        // (Nor while a stop of the last one is held back: what the agent
        // still says of that work opens no new turn.)
        if self.stage != Stage::Ready
            || self.turn.is_some()
            || self.moving.is_some()
            || self.held_stop.is_some()
        {
            return;
        }
        if self.own.replace(Instant::now()).is_none() {
            self.last_chunk = None;
            self.send(Update::TurnStarted);
        }
    }

    /// The turn the agent began by itself is over.
    fn own_turn_ends(&mut self, stop: Stop) {
        if self.own.take().is_none() {
            return;
        }
        // Nothing can be permitted any more.
        self.cancel_permissions();
        self.send(Update::TurnEnded(stop));
        self.last_chunk = None;
        let (terminals, shown) = (&self.terminals, &self.shown);
        self.tools
            .retain(|call| shows_live_terminal(call, terminals, shown));
        self.next_prompt();
    }

    fn cancel(&mut self) {
        if self.session.is_none() {
            return;
        }
        match &mut self.turn {
            Some(turn) if !turn.cancelled => turn.cancelled = true,
            // A turn of the agent's own is over for the interface at once:
            // no prompt's answer will end it. The agent is told a moment
            // later, unless a prompt follows (see `MAKE_WAY`).
            None if self.own.take().is_some() => {
                self.held_stop = Some(Instant::now() + MAKE_WAY);
                self.send(Update::TurnEnded(Stop::Cancelled));
                self.last_chunk = None;
                return;
            }
            _ => return,
        }
        self.stop_agent();
    }

    /// Tells the agent to stop what it does, answers what it asked, and says
    /// of the calls it was making that they will not finish.
    fn stop_agent(&mut self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let notification = acp::CancelNotification::new(session);
        self.write(rpc::notification(
            acp::AGENT_METHOD_NAMES.session_cancel,
            &notification,
        ));
        self.cancel_permissions();
        // The agent will not finish these; say so before it confirms.
        let mut patches = Vec::new();
        for call in &mut self.tools {
            if matches!(call.status, ToolStatus::Pending | ToolStatus::Running) {
                call.status = ToolStatus::Cancelled;
                patches.push(ToolPatch {
                    id: call.id.clone(),
                    status: Some(ToolStatus::Cancelled),
                    ..ToolPatch::default()
                });
            }
        }
        for patch in patches {
            self.send(Update::ToolPatch(patch));
        }
    }

    /// Answer every open permission request with the `cancelled` outcome.
    fn cancel_permissions(&mut self) {
        for open in std::mem::take(&mut self.permissions) {
            self.respond(&open.rpc, cancelled_outcome());
        }
    }

    fn permit(&mut self, request: u64, option: &str) {
        let Some(index) = self
            .permissions
            .iter()
            .position(|open| open.request.id == request)
        else {
            log::debug!("an answer to permission request {request}, which is not open");
            return;
        };
        if !self.permissions[index]
            .request
            .options
            .iter()
            .any(|offered| offered.id == option)
        {
            // Never pass on an answer the agent did not offer. The request
            // is still open, so the thread is told again that it blocks.
            log::warn!("permission request {request} has no option {option:?}");
            self.send(Update::Permission(self.permissions[index].request.clone()));
            return;
        }
        let open = self.permissions.remove(index);
        let outcome = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
            option.to_owned(),
        ));
        self.respond(
            &open.rpc,
            to_value(acp::RequestPermissionResponse::new(outcome)),
        );
    }

    fn select(&mut self, target: Target, value: String) {
        let (Stage::Ready, Some(session)) = (self.stage, self.session.clone()) else {
            log::debug!("a {} was chosen before the session exists", target.noun());
            return;
        };
        let Some(selector) = self.selectors.get(target) else {
            self.send(Update::Notice(
                Level::Warning,
                format!("{} offers no {} to choose", self.agent, target.noun()),
            ));
            return;
        };
        let names = &acp::AGENT_METHOD_NAMES;
        match selector.source.clone() {
            Source::Option(option) => {
                let request = acp::SetSessionConfigOptionRequest::new(
                    session,
                    option,
                    acp::SessionConfigOptionValue::value_id(value.clone()),
                );
                self.request(
                    names.session_set_config_option,
                    &request,
                    Pending::SetOption { target, value },
                );
            }
            Source::Modes => {
                let request = acp::SetSessionModeRequest::new(session, value.clone());
                self.request(names.session_set_mode, &request, Pending::SetMode(value));
            }
        }
    }

    fn on_selected(&mut self, target: Target, value: &str, result: Result<Value, RpcError>) {
        match result {
            Err(error) => self.send(Update::Notice(
                Level::Warning,
                format!(
                    "{} did not switch to {} {value}: {}",
                    self.agent,
                    target.noun(),
                    error.describe()
                ),
            )),
            // The answer to a config option is the whole new configuration,
            // because one choice can move another.
            Ok(result) => match parse::<acp::SetSessionConfigOptionResponse>(result) {
                Ok(response) if !response.config_options.is_empty() => {
                    self.apply_options(response.config_options)
                }
                _ => self.note_selected(target, value),
            },
        }
    }

    fn note_selected(&mut self, target: Target, value: &str) {
        if self.selectors.set(target, value) {
            self.send(match target {
                Target::Mode => Update::Mode(value.to_owned()),
                Target::Model => Update::Model(value.to_owned()),
            });
        }
    }

    fn apply_options(&mut self, options: Vec<acp::SessionConfigOption>) {
        let changed = self.selectors.replace_options(options);
        if let Some(mode) = changed.mode {
            self.send(Update::Mode(mode));
        }
        if let Some(model) = changed.model {
            self.send(Update::Model(model));
        }
        if let Some(providers) = changed.providers {
            self.send(Update::Providers(providers));
        }
    }

    // Edits.

    fn on_request_of_the_interface(&mut self, request: Request) {
        if self.ending.is_some() {
            // The agent is gone; the reason is on its way.
            return;
        }
        match request {
            Request::Rewind { prompt, blocks } => {
                if let Err(reason) = self.rewind(prompt, blocks) {
                    self.send(Update::Rewound(Err(reason)));
                }
            }
            Request::Steer(blocks) => self.steer(blocks),
        }
    }

    /// What this backend answers beyond the commands, as it stands now.
    fn offers(&self) -> Offers {
        Offers {
            steer: self.steers,
            rewind: self.rewinds,
            listening: self.listening,
            ..Offers::default()
        }
    }

    /// Hands a message to the turn that runs, for an agent that takes one
    /// there. One the turn ended before is not sent: the interface, which
    /// still holds it, sends it as a prompt.
    fn steer(&mut self, blocks: Vec<Block>) {
        if !self.steers {
            log::debug!("a message to steer by, which {} does not take", self.agent);
            return;
        }
        let session = match (self.stage, self.session.clone()) {
            (Stage::Ready, Some(session)) if self.turn.is_some() => session,
            _ => return,
        };
        let prompt = convert::prompt(&blocks, &self.accepts);
        if prompt.content.is_empty() {
            return;
        }
        let request = json!({
            "sessionId": session,
            "prompt": prompt.content,
            // With no turn running the agent is to leave the message alone,
            // not start a turn that no prompt of this client's stands for.
            "_meta": {"steering": {"idleBehavior": "promptRequired"}},
        });
        self.request(STEER_METHOD, &request, Pending::Steer(blocks));
        if let Some(turn) = &mut self.turn {
            turn.steered = true;
        }
    }

    /// The agent answered a message handed to its turn.
    fn on_steered(&mut self, blocks: Vec<Block>, result: Result<Value, RpcError>) {
        let outcome = match &result {
            Ok(value) => value.get("outcome").and_then(Value::as_str),
            Err(error) => {
                // It stays with the interface, which sends it when the turn ends.
                self.send(Update::Notice(
                    Level::Warning,
                    format!(
                        "{} did not take the message into its turn: {}. It goes when the turn ends.",
                        self.agent,
                        error.describe()
                    ),
                ));
                return;
            }
        };
        // No turn ran there any more: the interface sends it as a prompt.
        if outcome == Some("promptRequired") {
            return;
        }
        // The agent has it, and says nothing of it again: it is shown where
        // it came into the conversation, and can be edited like a prompt.
        self.last_chunk = None;
        if self.rewinds && self.showing.is_none() {
            self.showing = Some(self.name());
            self.before = Before::Unknown;
        }
        for block in blocks {
            self.send(Update::UserChunk(block));
        }
        self.at_work();
    }

    /// The ID of a prompt or message of the user's the thread is shown,
    /// and what came before it.
    fn name(&mut self) -> String {
        self.named += 1;
        let id = format!("prompt-{}", self.named);
        self.prompts.insert(id.clone(), self.before.clone());
        id
    }

    /// Starts an edit of `prompt`: the session it runs in is asked for, a
    /// fork up to the agent's last message before that prompt or a new one.
    /// An error is why nothing changed.
    fn rewind(&mut self, prompt: Option<String>, blocks: Vec<Block>) -> Result<(), String> {
        if !self.rewinds {
            return Err(format!(
                "{} cannot go back to an earlier prompt.",
                self.agent
            ));
        }
        if self.moving.is_some() {
            return Err("The last edit is still on its way.".into());
        }
        let session = match (self.stage, self.session.clone()) {
            (Stage::Ready, Some(session)) => session,
            _ => return Err("The agent is starting: edit once it is ready.".into()),
        };
        if self.turn.is_some() || !self.queue.is_empty() {
            return Err("The agent is working: stop it or let it finish, then edit.".into());
        }
        if convert::prompt(&blocks, &self.accepts).content.is_empty() {
            return Err(format!(
                "{} accepts nothing the edited prompt holds.",
                self.agent
            ));
        }
        let Some(prompt) = prompt else {
            // Nothing the session holds is replaced: the prompt simply runs.
            let id = self.name();
            self.send(Update::Rewound(Ok(id.clone())));
            self.queue.push_front(Queued {
                blocks,
                id: Some(id),
            });
            self.next_prompt();
            return Ok(());
        };
        let before = self.prompts.get(&prompt).cloned().ok_or_else(|| {
            "That prompt was not sent or shown on this connection to the agent.".to_owned()
        })?;
        let names = &acp::AGENT_METHOD_NAMES;
        match &before {
            Before::Unknown => {
                return Err(format!(
                    "{} cannot go back to before this prompt: what came before it was not seen here, or has no id to go back to.",
                    self.agent
                ));
            }
            Before::Nothing => {
                let request = self.new_session_request();
                self.request(names.session_new, &request, Pending::Move { fork: false });
            }
            Before::Message(message) => {
                let request = acp::ForkSessionRequest::new(session, self.options.cwd.clone())
                    .mcp_servers(self.mcp_servers())
                    .system_prompt(self.instructions())
                    .meta(self.fork_meta(message));
                self.request(names.session_fork, &request, Pending::Move { fork: true });
            }
        }
        self.moving = Some(Move { blocks, before });
        Ok(())
    }

    /// The session's `_meta` with the launch's way of naming the point a
    /// fork ends at, at the agent's message `message`.
    fn fork_meta(&self, message: &str) -> Option<Map<String, Value>> {
        let template = Value::Object(self.options.fork_meta.clone()?);
        let Value::Object(point) = fill(&template, message) else {
            return None;
        };
        let mut meta = self.options.meta.clone().unwrap_or_default();
        merge(&mut meta, point);
        Some(meta)
    }

    /// The agent answered the request for the session an edit runs in: the
    /// conversation goes on there, or, when it failed, where it was.
    fn on_moved(&mut self, fork: bool, result: Result<Value, RpcError>) {
        let Some(moving) = self.moving.take() else {
            return;
        };
        let opened = result.and_then(|value| {
            if fork {
                parse::<acp::ForkSessionResponse>(value)
                    .map(|opened| (opened.session_id, opened.modes, opened.config_options))
            } else {
                parse::<acp::NewSessionResponse>(value)
                    .map(|opened| (opened.session_id, opened.modes, opened.config_options))
            }
        });
        let (id, modes, options) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                // What the session that was not made said is dropped.
                self.take_early();
                let what = if fork {
                    "fork the session"
                } else {
                    "start a new session"
                };
                self.send(Update::Rewound(Err(format!(
                    "{} could not {what}: {}",
                    self.agent,
                    error.describe()
                ))));
                // Prompts sent meanwhile go to the session as it was.
                self.next_prompt();
                return;
            }
        };
        let id = id.0.to_string();
        let left = self.session.replace(id.clone()).filter(|left| *left != id);
        let kept =
            [Target::Mode, Target::Model].map(|target| (target, self.selectors.current(target)));
        self.announce(modes, options);
        self.take_early();
        // A session that starts over has heard nothing yet, the app's note
        // included; a fork holds what was said before it.
        if !fork {
            self.preface_due = !self.takes_instructions && self.options.preface.is_some();
        }
        // The session moved away from stays the agent's, as it was; this
        // connection lets go of it.
        if self.can_close
            && let Some(left) = left
        {
            let request = acp::CloseSessionRequest::new(left);
            self.request(
                acp::AGENT_METHOD_NAMES.session_close,
                &request,
                Pending::Close,
            );
        }
        // The mode and the model in force go on in force.
        for (target, value) in kept {
            let Some(value) = value else {
                continue;
            };
            let offered = self
                .selectors
                .get(target)
                .is_some_and(|selector| selector.choices.iter().any(|choice| choice.id == value));
            if offered && self.selectors.current(target).as_deref() != Some(value.as_str()) {
                self.select(target, value);
            }
        }
        self.before = moving.before;
        let id = self.name();
        self.send(Update::Rewound(Ok(id.clone())));
        self.queue.push_front(Queued {
            blocks: moving.blocks,
            id: Some(id),
        });
        self.next_prompt();
    }

    // Answers from the agent.

    fn on_response(&mut self, id: &Value, result: Result<Value, RpcError>) {
        let Some(pending) = id.as_u64().and_then(|id| self.pending.remove(&id)) else {
            log::debug!("{} answered a request that is not open: {id}", self.agent);
            return;
        };
        match pending {
            Pending::Initialize => self.on_initialized(result),
            Pending::NewSession => self.on_session_opened(false, result),
            Pending::Reopen => self.on_session_opened(true, result),
            Pending::Prompt(serial) => self.on_prompt_answered(serial, result),
            Pending::SetMode(mode) => match result {
                Ok(_) => self.note_selected(Target::Mode, &mode),
                Err(error) => self.on_selected(Target::Mode, &mode, Err(error)),
            },
            Pending::SetOption { target, value } => self.on_selected(target, &value, result),
            Pending::Move { fork } => self.on_moved(fork, result),
            Pending::Steer(blocks) => self.on_steered(blocks, result),
            Pending::Into { blocks, id } => self.on_into(blocks, id, result),
            Pending::Close => {
                if let Err(error) = result {
                    log::debug!(
                        "{} did not let go of a session left by an edit: {}",
                        self.agent,
                        error.describe()
                    );
                }
            }
        }
    }

    // Notifications from the agent.

    fn on_notification(&mut self, method: &str, params: Value) {
        if method == acp::CLIENT_METHOD_NAMES.session_update {
            let kind = params["update"]["sessionUpdate"]
                .as_str()
                .map(str::to_owned);
            match parse::<acp::SessionNotification>(params) {
                Ok(notification) => self.on_session_notification(notification),
                Err(error) => match kind.as_deref() {
                    // A kind this client handles, so the update itself is broken.
                    Some(kind) if KNOWN_UPDATES.contains(&kind) => log::warn!(
                        "{} sent a {kind} update this client cannot read: {}",
                        self.agent,
                        error.message
                    ),
                    // Most likely a kind of update newer than this client.
                    kind => log::debug!(
                        "{} sent a session update of an unknown kind: {kind:?}",
                        self.agent
                    ),
                },
            }
        } else if method == acp::PROTOCOL_LEVEL_METHOD_NAMES.cancel_request {
            self.on_request_cancelled(&params);
        } else {
            log::debug!("{} sent an unknown notification: {method}", self.agent);
        }
    }

    fn on_session_notification(&mut self, notification: acp::SessionNotification) {
        match &self.session {
            Some(session) if **session == *notification.session_id.0 => {
                self.on_update(notification.update)
            }
            // The session an edit moves to may speak before it is answered.
            Some(_) if self.moving.is_some() => self.early.push(notification),
            Some(_) => log::debug!(
                "{} sent an update for another session, {}",
                self.agent,
                notification.session_id
            ),
            // Before `session/new` is answered there is no telling yet.
            None => self.early.push(notification),
        }
    }

    fn on_update(&mut self, update: acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::UserMessageChunk(chunk) => {
                // What a subagent was given is its call's to show, not a
                // prompt of the user's.
                if convert::said_by_subagent(&chunk.meta).is_some() {
                    return;
                }
                // The app's own note before a first prompt is not the
                // user's, where an agent shows the prompt back or replays it.
                let Some(content) = convert::without_preface(chunk.content) else {
                    return;
                };
                if self.is_echo(&content) {
                    return;
                }
                self.last_chunk = None;
                // A message the thread shows apart, as a prompt of a replay
                // or one the turn took: it can be edited like one sent here.
                if self.rewinds && self.showing.is_none() {
                    self.showing = Some(self.name());
                    self.before = Before::Unknown;
                }
                self.send(Update::UserChunk(convert::user_block(content)));
            }
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                self.own_turn_starts();
                // A subagent's words go to the call that started it: they
                // are no part of the agent's answer.
                if let Some(call) = convert::said_by_subagent(&chunk.meta) {
                    self.agent_spoke();
                    self.heard_of(&call);
                    let mut text = convert::markdown(&chunk.content);
                    let id = chunk.message_id.as_ref().map(|id| id.0.to_string());
                    // Another message of the same subagent is another paragraph.
                    if let Some((last_call, last_id)) = &self.last_said
                        && *last_call == call
                        && *last_id != id
                        && !text.is_empty()
                    {
                        text.insert_str(0, "\n\n");
                    }
                    self.last_said = Some((call.clone(), id));
                    if !text.is_empty() {
                        self.send(Update::AgentSaid { call, text });
                    }
                    return;
                }
                self.at_work();
                let text = self.chunk_text(Chunk::Message, &chunk);
                if !text.is_empty() {
                    self.send(Update::AssistantChunk(text));
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                self.own_turn_starts();
                // A subagent's reasoning is not the agent's own, and is not shown.
                if let Some(call) = convert::said_by_subagent(&chunk.meta) {
                    self.agent_spoke();
                    self.heard_of(&call);
                    return;
                }
                self.at_work();
                let text = self.chunk_text(Chunk::Thought, &chunk);
                if !text.is_empty() {
                    self.send(Update::ThoughtChunk(text));
                }
            }
            acp::SessionUpdate::ToolCall(call) => {
                self.own_turn_starts();
                self.agent_spoke();
                self.last_chunk = None;
                if convert::said_by_subagent(&call.meta).is_none() {
                    self.at_work();
                }
                let told = convert::delegation(&call.meta, call.raw_input.as_ref());
                self.note_display(&convert::display(&call.meta));
                let call = ToolCall {
                    id: call.tool_call_id.0.to_string(),
                    title: call.title,
                    kind: convert::tool_kind(call.kind),
                    status: convert::tool_status(call.status),
                    content: self.tool_content(call.content),
                    locations: convert::locations(call.locations),
                    raw_input: call.raw_input,
                    raw_output: call.raw_output,
                    agent: told.agent,
                    parent: told.parent,
                    work: kou_thread::AgentWork {
                        background: told.background.unwrap_or_default(),
                        tokens: told.tokens,
                        ..kou_thread::AgentWork::default()
                    },
                };
                match self.tools.iter_mut().find(|known| known.id == call.id) {
                    Some(known) => *known = call.clone(),
                    None => self.tools.push(call.clone()),
                }
                if let Some(launch) = &call.parent {
                    self.heard_of(launch);
                }
                self.watch_behind(&call);
                self.send(Update::Tool(call));
                self.forget_settled_tools();
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                self.agent_spoke();
                let display = convert::display(&update.meta);
                let touched = self.note_display(&display);
                let mut patch = self.tool_patch(update);
                // What its terminal wrote or how it ended is the call's
                // content, whether or not this report gives content anew.
                if touched && patch.content.is_none() {
                    patch.content = self
                        .tools
                        .iter()
                        .find(|call| call.id == patch.id)
                        .map(|call| self.with_shown(call.content.clone()));
                }
                let call = self.merge_tool(&patch);
                // A command whose call is over while its terminal has not
                // exited was left to run in the background; the exit that
                // comes later is its end.
                let open = call.content.iter().any(|content| {
                    matches!(content, ToolContent::Terminal { id, .. }
                        if self.shown.get(id).is_some_and(|shown| shown.exit.is_none()))
                });
                if call.status == ToolStatus::Done && open && !call.work.background {
                    patch.background = Some(true);
                    self.merge_tool(&patch);
                }
                if call.work.background && !call.work.ended && display.exit.is_some() && !open {
                    patch.ended = Some(true);
                    self.merge_tool(&patch);
                    // (The agent is told of it, as of an agent that ended.)
                    self.commands_ended += 1;
                }
                if let Some(launch) = &call.parent {
                    self.heard_of(launch);
                }
                self.watch_behind(&call);
                self.send(Update::ToolPatch(patch));
                self.forget_settled_tools();
                // What has exited and is shown is kept by its call from here.
                self.shown.retain(|_, shown| shown.exit.is_none());
            }
            acp::SessionUpdate::Plan(plan) => {
                self.agent_spoke();
                self.send(Update::Plan(convert::plan(plan)));
            }
            acp::SessionUpdate::AvailableCommandsUpdate(update) => {
                self.send(Update::Commands(convert::commands(
                    update.available_commands,
                )));
            }
            acp::SessionUpdate::CurrentModeUpdate(update) => {
                self.note_selected(Target::Mode, &update.current_mode_id.0);
            }
            acp::SessionUpdate::ConfigOptionUpdate(update) => {
                self.apply_options(update.config_options)
            }
            acp::SessionUpdate::SessionInfoUpdate(update) => match update.title {
                MaybeUndefined::Value(title) => self.send(Update::Title(title)),
                // `null` clears the title; the thread has only a title to set.
                MaybeUndefined::Null => self.send(Update::Title(String::new())),
                MaybeUndefined::Undefined => {}
            },
            acp::SessionUpdate::UsageUpdate(update) => {
                self.usage.context_used = Some(update.used);
                self.usage.context_limit = Some(update.size);
                if let Some(cost) = &update.cost
                    && cost.currency.eq_ignore_ascii_case("USD")
                {
                    self.usage.cost_usd = Some(cost.amount);
                }
                // What a cycle of the model's cost is said at its end: that
                // is where a turn the agent began by itself is over. (What
                // is said in the middle of one says no cost.)
                let ended = update.cost.is_some();
                self.send(Update::Usage(self.usage));
                // A cycle the agent ran because something in the background
                // ended: the only word there is of an agent there being done.
                if ended {
                    if convert::cycle_origin(&update.meta).as_deref() == Some("task-notification") {
                        self.something_behind_ended();
                    }
                    self.cycle_from = None;
                    self.commands_ended = 0;
                }
                // In a prompt's turn it is where the agent has given its
                // answer: one that holds the turn open from here on does so
                // only for what it left running.
                if ended
                    && self.options.holds_turns
                    && let Some(turn) = &mut self.turn
                {
                    turn.answered = !std::mem::take(&mut turn.steered);
                }
                if ended {
                    self.own_turn_ends(Stop::Finished);
                } else if let Some(since) = &mut self.own {
                    *since = Instant::now();
                }
            }
            acp::SessionUpdate::Notice(notice) => {
                let (level, text) = convert::notice(notice);
                self.last_chunk = None;
                self.send(Update::Notice(level, text));
            }
            acp::SessionUpdate::CompactionUpdate(update) => self.on_compaction(update),
            other => log::debug!(
                "{} sent an update with no place here: {other:?}",
                self.agent
            ),
        }
    }

    /// The thread has no entry for a compaction, only a notice for its outcome.
    fn on_compaction(&mut self, update: acp::CompactionUpdate) {
        let id = update.compaction_id.0.to_string();
        let notice = match update.status {
            acp::CompactionStatus::Completed => {
                (Level::Info, "The conversation was compacted".to_owned())
            }
            acp::CompactionStatus::Failed => (
                Level::Warning,
                match update.error {
                    MaybeUndefined::Value(error) if !error.is_empty() => {
                        format!("Compacting the conversation failed: {error}")
                    }
                    _ => "Compacting the conversation failed".to_owned(),
                },
            ),
            _ => return,
        };
        // Later updates of the same compaction only patch its summary.
        if self.compacted.contains(&id) {
            return;
        }
        self.compacted.push(id);
        self.last_chunk = None;
        self.send(Update::Notice(notice.0, notice.1));
    }

    /// Whether this user content is the running prompt coming back. The
    /// interface already shows what the user sent, so an echo would double it.
    fn is_echo(&mut self, content: &acp::ContentBlock) -> bool {
        let Some(turn) = &mut self.turn else {
            return false;
        };
        let Some(echo) = &mut turn.echo else {
            return false;
        };
        let matched = match content {
            acp::ContentBlock::Text(text) => match echo.text.strip_prefix(text.text.as_str()) {
                Some(rest) => {
                    echo.text = rest.to_owned();
                    true
                }
                None => false,
            },
            _ if echo.attachments > 0 => {
                echo.attachments -= 1;
                true
            }
            _ => false,
        };
        if !matched {
            turn.echo = None;
        }
        matched
    }

    /// The agent produced something of its own, so an echo is no longer due.
    fn agent_spoke(&mut self) {
        if let Some(turn) = &mut self.turn {
            turn.echo = None;
        }
    }

    /// A chunk's markdown, set apart from the previous message of its kind
    /// when the agent marks it as the start of another one.
    fn chunk_text(&mut self, kind: Chunk, chunk: &acp::ContentChunk) -> String {
        self.agent_spoke();
        let mut text = convert::markdown(&chunk.content);
        let id = chunk.message_id.as_ref().map(|id| id.0.to_string());
        // The agent's last message is where a fork before the next prompt ends.
        self.before = id.clone().map_or(Before::Unknown, Before::Message);
        if let (Some((last_kind, last_id)), Some(id)) = (&self.last_chunk, &id)
            && *last_kind == kind
            && last_id != id
            && !text.is_empty()
        {
            text.insert_str(0, "\n\n");
        }
        self.last_chunk = id.map(|id| (kind, id));
        text
    }

    // Tool calls.

    fn tool_content(&self, content: Vec<acp::ToolCallContent>) -> Vec<ToolContent> {
        self.with_shown(convert::tool_content(content, |id| {
            self.terminals.snapshot(id)
        }))
    }

    /// `content` with the terminals the agent shows its commands in as
    /// they stand now.
    fn with_shown(&self, mut content: Vec<ToolContent>) -> Vec<ToolContent> {
        for part in &mut content {
            if let ToolContent::Terminal { id, output, exit } = part
                && let Some(shown) = self.shown.get(id)
            {
                *output = shown.output.clone();
                *exit = shown.exit.flatten();
            }
        }
        content
    }

    /// Takes in what a report says of the terminal its command is shown
    /// in. Whether it changed what such a terminal holds.
    fn note_display(&mut self, display: &convert::Display) -> bool {
        // (A terminal of this client's own is not the agent's to describe.)
        let mut touched = false;
        if let Some(id) = &display.info
            && self.terminals.snapshot(id).is_none()
        {
            self.shown.entry(id.clone()).or_default();
        }
        if let Some((id, text)) = &display.output
            && let Some(shown) = self.shown.get_mut(id)
        {
            shown.output.push_str(text);
            touched = true;
        }
        if let Some((id, code)) = &display.exit
            && let Some(shown) = self.shown.get_mut(id)
        {
            shown.exit = Some(*code);
            touched = true;
        }
        if touched && let Some(since) = &mut self.own {
            *since = Instant::now();
        }
        touched
    }

    fn tool_patch(&self, update: acp::ToolCallUpdate) -> ToolPatch {
        let fields = update.fields;
        let told = convert::delegation(&update.meta, fields.raw_input.as_ref());
        ToolPatch {
            id: update.tool_call_id.0.to_string(),
            title: fields.title,
            kind: fields.kind.map(convert::tool_kind),
            status: fields.status.map(convert::tool_status),
            content: fields.content.map(|content| self.tool_content(content)),
            locations: fields.locations.map(convert::locations),
            raw_input: fields.raw_input,
            raw_output: fields.raw_output,
            agent: told.agent,
            parent: told.parent,
            background: told.background,
            ended: None,
            tokens: told.tokens,
        }
    }

    /// Fold a patch into what is known of the call, and return the whole call.
    fn merge_tool(&mut self, patch: &ToolPatch) -> ToolCall {
        let index = match self.tools.iter().position(|call| call.id == patch.id) {
            Some(index) => index,
            None => {
                self.tools.push(ToolCall {
                    id: patch.id.clone(),
                    ..ToolCall::default()
                });
                self.tools.len() - 1
            }
        };
        let call = &mut self.tools[index];
        if let Some(title) = &patch.title {
            call.title = title.clone();
        }
        if let Some(kind) = patch.kind {
            call.kind = kind;
        }
        if let Some(status) = patch.status {
            call.status = status;
        }
        if let Some(content) = &patch.content {
            call.content = content.clone();
        }
        if let Some(locations) = &patch.locations {
            call.locations = locations.clone();
        }
        if patch.raw_input.is_some() {
            call.raw_input = patch.raw_input.clone();
        }
        match &patch.agent {
            Some(kind) if !kind.is_empty() || call.agent.is_none() => {
                call.agent = Some(kind.clone());
            }
            _ => {}
        }
        if patch.parent.is_some() {
            call.parent = patch.parent.clone();
        }
        if let Some(background) = patch.background {
            call.work.background = background;
        }
        if patch.tokens.is_some() {
            call.work.tokens = patch.tokens;
        }
        if let Some(ended) = patch.ended {
            call.work.ended = ended;
        }
        if patch.raw_output.is_some() {
            call.raw_output = patch.raw_output.clone();
        }
        call.clone()
    }

    /// Outside a turn nothing asks about a finished call again, unless it
    /// shows a terminal that still runs.
    fn forget_settled_tools(&mut self) {
        if self.turn.is_some() || self.stage != Stage::Ready {
            return;
        }
        let (terminals, shown) = (&self.terminals, &self.shown);
        self.tools.retain(|call| {
            matches!(call.status, ToolStatus::Pending | ToolStatus::Running)
                || shows_live_terminal(call, terminals, shown)
        });
    }

    // Requests from the agent.

    fn on_request(&mut self, id: Value, method: &str, params: Value) {
        let names = &acp::CLIENT_METHOD_NAMES;
        let result = if method == names.session_request_permission {
            return self.on_permission_request(id, params);
        } else if method == names.terminal_wait_for_exit {
            return self.on_wait_for_exit(id, params);
        } else if method == names.fs_read_text_file {
            parse::<acp::ReadTextFileRequest>(params).and_then(|request| {
                let content = fs::read_text_file(&request.path, request.line, request.limit)?;
                to_value(acp::ReadTextFileResponse::new(content))
            })
        } else if method == names.fs_write_text_file {
            parse::<acp::WriteTextFileRequest>(params).and_then(|request| {
                fs::write_text_file(&request.path, &request.content)?;
                Ok(json!({}))
            })
        } else if method == names.terminal_create {
            parse(params).and_then(|request| self.create_terminal(request))
        } else if method == names.terminal_output {
            parse::<acp::TerminalOutputRequest>(params).and_then(|request| {
                let snapshot = self.live_snapshot(&request.terminal_id.0)?;
                to_value(
                    acp::TerminalOutputResponse::new(snapshot.output, snapshot.truncated)
                        .exit_status(snapshot.exit.map(exit_status)),
                )
            })
        } else if method == names.terminal_kill {
            parse::<acp::KillTerminalRequest>(params).and_then(|request| {
                self.terminals.kill(&request.terminal_id.0)?;
                Ok(json!({}))
            })
        } else if method == names.terminal_release {
            parse::<acp::ReleaseTerminalRequest>(params)
                .and_then(|request| self.release_terminal(&request.terminal_id.0))
        } else {
            log::debug!("{} called an unknown method: {method}", self.agent);
            Err(RpcError::new(rpc::METHOD_NOT_FOUND, "Method not found"))
        };
        self.respond(&id, result);
    }

    fn on_permission_request(&mut self, id: Value, params: Value) {
        let request = match parse::<acp::RequestPermissionRequest>(params) {
            Ok(request) => request,
            Err(error) => return self.respond(&id, Err(error)),
        };
        if self.turn.as_ref().is_some_and(|turn| turn.cancelled) {
            // The user already said stop; there is nothing left to ask.
            return self.respond(&id, cancelled_outcome());
        }
        if self.turn.is_none() {
            // The agent's own turn was stopped a moment ago and the stop
            // has not gone out yet: this was on its way already.
            if self.held_stop.is_some() {
                return self.respond(&id, cancelled_outcome());
            }
            // Asked with no turn open: it opens one of the agent's own, so
            // that there is a turn the answer goes on in and that can end.
            // (Without one the conversation would wait on the question and
            // then "work" with nothing to end that.)
            self.own_turn_starts();
        }
        self.agent_spoke();
        self.last_chunk = None;
        let patch = self.tool_patch(request.tool_call);
        let request = PermissionRequest {
            questions: Vec::new(),
            id: self.next_permission,
            tool: self.merge_tool(&patch),
            options: convert::permission_options(request.options),
        };
        self.next_permission += 1;
        self.permissions.push(OpenPermission {
            rpc: id,
            request: request.clone(),
        });
        self.send(Update::Permission(request));
    }

    /// `$/cancel_request`: the agent takes back a request it made.
    fn on_request_cancelled(&mut self, params: &Value) {
        let Some(id) = params.get("requestId") else {
            return;
        };
        if let Some(index) = self.permissions.iter().position(|open| open.rpc == *id) {
            let open = self.permissions.remove(index);
            self.respond(&open.rpc, cancelled_outcome());
        } else if let Some(index) = self.exit_waiters.iter().position(|(_, rpc)| rpc == id) {
            let (_, rpc) = self.exit_waiters.remove(index);
            self.respond(
                &rpc,
                Err(RpcError::new(rpc::REQUEST_CANCELLED, "Request cancelled")),
            );
        }
    }

    // Terminals.

    fn create_terminal(&mut self, request: acp::CreateTerminalRequest) -> Result<Value, RpcError> {
        let id = self.terminals.create(Spawn {
            command: request.command,
            args: request.args,
            env: request
                .env
                .into_iter()
                .map(|variable| (variable.name, variable.value))
                .collect(),
            cwd: request.cwd.unwrap_or_else(|| self.options.cwd.clone()),
            limit: request.output_byte_limit,
        })?;
        to_value(acp::CreateTerminalResponse::new(id))
    }

    /// The state of a terminal that was not released.
    fn live_snapshot(&self, id: &str) -> Result<Snapshot, RpcError> {
        self.terminals
            .snapshot(id)
            .filter(|_| self.terminals.is_live(id))
            .ok_or_else(|| no_terminal(id))
    }

    fn on_wait_for_exit(&mut self, id: Value, params: Value) {
        let terminal = match parse::<acp::WaitForTerminalExitRequest>(params) {
            Ok(request) => request.terminal_id.0.to_string(),
            Err(error) => return self.respond(&id, Err(error)),
        };
        if !self.terminals.is_live(&terminal) {
            return self.respond(&id, Err(no_terminal(&terminal)));
        }
        match self.terminals.exit(&terminal) {
            None => self.exit_waiters.push((terminal, id)),
            Some(exit) => {
                // The thread learns of the exit before the agent can act on it.
                self.mirror(&terminal);
                self.respond(&id, exited(exit));
            }
        }
    }

    fn release_terminal(&mut self, id: &str) -> Result<Value, RpcError> {
        let last = self.terminals.release(id)?;
        self.mirrored.remove(id);
        self.mirror_due.remove(id);
        let exit = last.exit.unwrap_or(Exit::Unknown);
        // A tool call keeps showing what its terminal printed.
        self.show_terminal(id, last);
        self.answer_exit_waiters(id, exit);
        self.forget_settled_tools();
        Ok(json!({}))
    }

    fn on_terminal_output(&mut self, id: String) {
        match self.mirrored.get(&id) {
            // Every update carries the whole output, so they are spaced out.
            Some(last) if last.elapsed() < MIRROR_INTERVAL => {
                self.mirror_due.insert(id, *last + MIRROR_INTERVAL);
            }
            _ => self.mirror(&id),
        }
    }

    fn on_terminal_exit(&mut self, id: &str) {
        self.mirror(id);
        if let Some(exit) = self.terminals.exit(id) {
            self.answer_exit_waiters(id, exit);
        }
    }

    fn answer_exit_waiters(&mut self, terminal: &str, exit: Exit) {
        let (answer, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.exit_waiters)
            .into_iter()
            .partition(|(waited, _)| waited == terminal);
        self.exit_waiters = keep;
        for (_, rpc) in answer {
            self.respond(&rpc, exited(exit));
        }
    }

    /// Bring the tool calls that show a live terminal up to date.
    fn mirror(&mut self, id: &str) {
        self.mirror_due.remove(id);
        if !self.tools.iter().any(|call| shows_terminal(call, id)) {
            // Nobody shows it; a later tool call reads it when it comes.
            self.terminals.mark_seen(id);
            return;
        }
        if let Some(snapshot) = self.terminals.take_snapshot(id) {
            self.mirrored.insert(id.to_owned(), Instant::now());
            self.show_terminal(id, snapshot);
        }
    }

    fn show_terminal(&mut self, id: &str, snapshot: Snapshot) {
        let fresh = convert::terminal_content(id.to_owned(), snapshot);
        let mut patches = Vec::new();
        for call in &mut self.tools {
            let mut changed = false;
            for content in &mut call.content {
                if matches!(content, ToolContent::Terminal { id: shown, .. } if shown == id)
                    && *content != fresh
                {
                    *content = fresh.clone();
                    changed = true;
                }
            }
            if changed {
                patches.push(ToolPatch {
                    id: call.id.clone(),
                    content: Some(call.content.clone()),
                    ..ToolPatch::default()
                });
            }
        }
        for patch in patches {
            self.send(Update::ToolPatch(patch));
        }
    }

    // The end.

    /// The agent went away without being asked to.
    fn agent_gone(&mut self) {
        let program = self
            .launch
            .program
            .file_name()
            .unwrap_or(self.launch.program.as_os_str())
            .to_string_lossy()
            .into_owned();
        let mut reason = match self.exit {
            Some(exit) => format!("{program} exited with {}", exit.describe()),
            None => format!("{program} closed its output"),
        };
        let last_words: Vec<String> = self
            .stderr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect();
        if !last_words.is_empty() {
            reason.push_str(":\n");
            reason.push_str(&last_words.join("\n"));
        }
        self.close(Some(reason));
    }

    /// End the agent and everything it started, and say so. `reason` is why,
    /// unless the interface asked for it.
    fn close(&mut self, reason: Option<String>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(turn) = self.turn.take() {
            if reason.is_none()
                && !turn.cancelled
                && let Some(session) = self.session.clone()
            {
                // A last chance for the agent to stop its work in good order.
                let notification = acp::CancelNotification::new(session);
                self.write(rpc::notification(
                    acp::AGENT_METHOD_NAMES.session_cancel,
                    &notification,
                ));
            }
            self.send(Update::TurnEnded(match &reason {
                Some(reason) => Stop::Failed(reason.clone()),
                None => Stop::Cancelled,
            }));
        }
        self.pending.clear();
        self.moving = None;
        self.held_stop = None;
        // Open questions are answered rather than dropped, as for a cancelled turn.
        self.cancel_permissions();
        self.exit_waiters.clear();
        self.queue.clear();
        self.terminals.release_all();
        // Dropping the sender lets the writer flush and close the agent's
        // input. An agent that leaves by itself at that point, as most do,
        // gets to save what it has; the others are told to go.
        self.outbox = None;
        self.process.wait_timeout(LEAVE);
        self.process.stop(GRACE);
        self.send(Update::Closed(reason));
        self.updates.close();
        self.commands.close();
        self.requests.close();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Only reached with the session unfinished if its thread panicked.
        if !self.finished {
            self.process.kill();
            self.send(Update::Closed(Some(
                "the connection to the agent failed inside this app".into(),
            )));
        }
    }
}

fn shows_terminal(call: &ToolCall, terminal: &str) -> bool {
    call.content
        .iter()
        .any(|content| matches!(content, ToolContent::Terminal { id, .. } if id == terminal))
}

/// A terminal the agent shows a command of its own in.
#[derive(Debug, Default)]
struct Shown {
    output: String,
    /// `Some` once its command ended: its status, when the agent knows it.
    exit: Option<Option<i32>>,
}

fn shows_live_terminal(
    call: &ToolCall,
    terminals: &Terminals,
    shown: &HashMap<String, Shown>,
) -> bool {
    call.content.iter().any(|content| {
        matches!(content, ToolContent::Terminal { id, .. }
            if terminals.is_live(id) || shown.get(id).is_some_and(|shown| shown.exit.is_none()))
    })
}

/// `template` with `message` wherever [`crate::FORK_MESSAGE_ID`] stands in it.
fn fill(template: &Value, message: &str) -> Value {
    match template {
        Value::String(text) if text == crate::FORK_MESSAGE_ID => Value::String(message.to_owned()),
        Value::Array(items) => Value::Array(items.iter().map(|item| fill(item, message)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), fill(value, message)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `more` added to `base`, objects under the same key merged key by key.
fn merge(base: &mut Map<String, Value>, more: Map<String, Value>) {
    for (key, value) in more {
        match (base.get_mut(&key), value) {
            (Some(Value::Object(inner)), Value::Object(value)) => merge(inner, value),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn no_terminal(id: &str) -> RpcError {
    RpcError::new(rpc::RESOURCE_NOT_FOUND, format!("no terminal {id}"))
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    serde_json::from_value(value)
        .map_err(|error| RpcError::new(rpc::INVALID_PARAMS, error.to_string()))
}

fn to_value(value: impl Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(rpc::INTERNAL_ERROR, error.to_string()))
}

fn cancelled_outcome() -> Result<Value, RpcError> {
    to_value(acp::RequestPermissionResponse::new(
        acp::RequestPermissionOutcome::Cancelled,
    ))
}

fn exit_status(exit: Exit) -> acp::TerminalExitStatus {
    let code = match exit {
        Exit::Code(code) => Some(code as u32),
        _ => None,
    };
    acp::TerminalExitStatus::new()
        .exit_code(code)
        .signal(exit.signal_name())
}

fn exited(exit: Exit) -> Result<Value, RpcError> {
    to_value(acp::WaitForTerminalExitResponse::new(exit_status(exit)))
}
