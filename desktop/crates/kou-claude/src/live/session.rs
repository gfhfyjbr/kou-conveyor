//! The thread that owns a session's state.
//!
//! Everything that happens to it arrives as an [`Event`]: what Claude Code
//! wrote, its process ending, the interface's commands and requests. They
//! are handled one at a time on this one thread, which keeps the updates in
//! the order things happened.

mod asks;
mod calls;
mod choices;
mod edit;
mod found;

use super::{
    link::{self, Event, Link},
    stream::{Key, Kind, Open, Stream},
    wire::{self, Model, text_of},
};
use crate::{Options, records};
use kou_thread::{
    Block, Capabilities, Choice, Command, Level, Offers, PermissionRequest, Request, Stop, Update,
    Usage,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long Claude Code gets to leave by itself once its input has ended.
const LEAVE: Duration = Duration::from_millis(250);
/// How long it gets to stop after being asked to, before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// How long the rest of its output is awaited once it has exited or stopped
/// talking: a child it left behind can hold the pipes open.
const DRAIN: Duration = Duration::from_millis(750);
/// How often a session without a command channel checks whether anyone
/// still reads its updates.
const ORPHAN_POLL: Duration = Duration::from_millis(250);

/// What a call refused because the user stopped the turn is told.
const CANCELLED: &str = "The user cancelled this prompt.";

/// What [`super::connect`] hands to the session's thread.
pub(crate) struct Wiring {
    pub events: mpsc::Receiver<Event>,
    /// For the links the session starts later.
    pub sender: mpsc::Sender<Event>,
    pub updates: async_channel::Sender<Update>,
    pub commands: async_channel::Receiver<Command>,
    pub requests: async_channel::Receiver<Request>,
    pub link: Link,
    /// How things stand in the host the link was found in, when it was
    /// found greeted: the session takes it up from there.
    pub found: Option<crate::host::Hello>,
    /// The number of the host an edit starts next, for a kept session.
    pub next_host: u32,
}

/// A request of ours that Claude Code has not answered.
#[derive(Debug)]
enum Pending {
    Initialize,
    Interrupt,
    SetMode(String),
    SetModel(String),
    SetEffort(String),
    /// `get_settings`, for the level of effort in force; after the level
    /// was set to this one, to see whether it holds.
    Settings(Option<String>),
    /// `initialize` of the process an edit is to run in.
    Move,
}

/// What a prompt or message written to Claude Code was, until its echo.
#[derive(Debug)]
enum Sent {
    /// A prompt, which the interface shows already.
    Prompt,
    /// A message into the turn at work, shown when Claude Code takes it.
    Steer(Vec<Block>),
}

/// What came before a prompt: where a copy of the conversation that the
/// prompt's edit runs in ends.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Before {
    /// Nothing: the prompt began the conversation.
    Nothing,
    /// The entry `entry` of the conversation of session `session` (the
    /// session in use, while its id is not known yet).
    At {
        session: Option<String>,
        entry: String,
    },
    /// Something not seen here.
    Unknown,
}

/// A prompt waiting for the session to take prompts.
#[derive(Debug)]
struct Queued {
    blocks: Vec<Block>,
    /// The name an edit already gave it.
    uuid: Option<String>,
}

/// An edit on its way: the process it runs in is starting. It is taken up
/// once it takes the edited prompt (the prompt's echo); until then the
/// process before it stays, and goes on if the new one dies.
struct Move {
    link: Link,
    /// The edited prompt, and its name.
    blocks: Vec<Block>,
    uuid: String,
    /// What came before it, in the conversation it runs in.
    before: Before,
    /// Its answer to `initialize`, once given: the edited prompt is
    /// written to it then.
    greeting: Option<Value>,
    /// What the new process said that is not handled yet.
    early: Vec<Value>,
}

/// The conversation's turn.
#[derive(Debug, Default)]
struct Turn {
    /// Claude Code said it was at work since the turn began.
    ran: bool,
    /// Why the last cycle of the model's in it stopped, once one has.
    stop: Option<Stop>,
    /// A cycle ended and no other began since: the turn is open only for
    /// what runs in the background, and a prompt is taken at once.
    answered: bool,
    /// The user stopped it.
    cancelled: bool,
}

/// A request of Claude Code's the user has not answered.
struct OpenPermission {
    /// Claude Code's id for it.
    request_id: String,
    asked: Asked,
    /// What the interface was told.
    request: PermissionRequest,
}

/// What an open request asks, which decides how it is answered.
enum Asked {
    /// Leave to run call `tool_use` with `input` (`can_use_tool`);
    /// `suggestions` are the rules Claude Code offers to remember.
    Leave {
        tool_use: String,
        input: Value,
        suggestions: Vec<Value>,
    },
    /// The questions of the `AskUserQuestion` call `tool_use`, whose leave
    /// to run is asked with them.
    Questions { tool_use: String, input: Value },
    /// An MCP server's form (`elicitation`), shown as the call `call`.
    Form {
        call: String,
        fields: Vec<super::ask::Field>,
    },
}

/// A tool call of Claude Code's or of a subagent's, as far as it is known.
#[derive(Debug, Default)]
struct Call {
    name: String,
    input: Value,
    parent: Option<String>,
    /// Its input as it streams.
    json: super::partial::Partial,
    /// Its input is whole.
    complete: bool,
    /// It was announced ([`Update::Tool`]).
    shown: bool,
    /// It is the to-do tool's, shown as the plan.
    plan: bool,
    background: bool,
    ended: bool,
}

pub(crate) struct Session {
    events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    updates: async_channel::Sender<Update>,
    commands: async_channel::Receiver<Command>,
    requests: async_channel::Receiver<Request>,
    options: Options,
    /// The program's name, for sentences.
    program: String,
    /// Claude Code's home, where the conversation's records are.
    home: Option<PathBuf>,
    link: Link,
    next_link: u64,
    /// See [`Wiring::found`].
    found: Option<crate::host::Hello>,
    /// See [`Wiring::next_host`].
    next_host: u32,

    /// `initialize` was answered: the session takes prompts.
    ready: bool,
    pending: HashMap<String, (u64, Pending)>,
    next_request: u64,
    models: Vec<Model>,
    model: Option<String>,
    mode: Option<String>,
    /// The modes the interface was last told of.
    modes_said: Vec<Choice>,
    /// The level of effort in force, as far as Claude Code said.
    effort: Option<String>,
    /// The level the user chose, which a process started for an edit is
    /// given too.
    pinned_effort: Option<String>,
    /// The levels of effort and the one in force the interface was last
    /// told of.
    efforts_said: Option<(Vec<Choice>, Option<String>)>,
    arguments: Vec<(String, String)>,
    /// What Claude Code says it can do (`capabilities` of `init`).
    capabilities: Vec<String>,
    /// The session's id, once known.
    session: Option<String>,
    /// Every id the conversation has had on this connection, oldest first.
    sessions: Vec<String>,
    /// Something has been said in the conversation, so Claude Code keeps a
    /// record of it. Until then its id names nothing that can be gone on
    /// with, and the interface is not told it.
    said: bool,
    /// The conversation asked for that has no record: said once, at the
    /// start.
    lost: Option<String>,
    /// The conversation began on this connection: nothing came before its
    /// first prompt.
    fresh: bool,
    /// The conversation's name, as the interface was last told it.
    title: Option<String>,
    usage: Usage,
    /// The model the latest answer came from, whose context window counts.
    answering_model: Option<String>,

    /// Claude Code says `session_state_changed`.
    states: bool,
    turn: Option<Turn>,
    /// A turn was ended here before Claude Code ended it: what is left of it
    /// is dropped until Claude Code is idle or takes the next prompt.
    draining: bool,
    /// What the interface was last told of [`Session::listens`].
    listening: bool,

    queue: VecDeque<Queued>,
    /// Prompts and messages written whose echo has not come, by uuid.
    sent: HashMap<String, Sent>,
    /// What came before each prompt shown, by its uuid.
    prompts: HashMap<String, Before>,
    /// The last entry of the conversation seen.
    last_entry: Option<String>,
    preface_due: bool,

    permissions: Vec<OpenPermission>,
    next_permission: u64,
    calls: HashMap<String, Call>,
    /// The call that started each task Claude Code runs, by the task's id.
    tasks: HashMap<String, String>,
    /// The conversation's task list, as the task tools change it.
    tasks_list: records::tools::Tasks,
    /// Claude Code's answer as it streams, and each subagent's, by the call
    /// that started the subagent.
    streams: HashMap<Option<String>, Stream>,
    /// The kind and block of the last text said, to set blocks apart.
    last_chunk: Option<(Kind, Key)>,
    /// The subagent that spoke last, by its call, and the block it spoke in.
    last_said: Option<(String, Key)>,

    moving: Option<Move>,
    commands_closed: bool,
    finished: bool,
}

impl Session {
    /// `lost`: the conversation that was to be gone on with and has no
    /// record, in whose place this one begins.
    pub fn new(options: Options, lost: Option<String>, wiring: Wiring) -> Self {
        let program = link::program_name(&options);
        let home = options
            .home
            .clone()
            .or_else(records::transcript::default_home);
        let fresh = options.resume.is_none();
        let preface_due = fresh && options.preface.is_some();
        let next_link = wiring.link.id + 1;
        Self {
            events: wiring.events,
            sender: wiring.sender,
            updates: wiring.updates,
            commands: wiring.commands,
            requests: wiring.requests,
            options,
            program,
            home,
            link: wiring.link,
            next_link,
            found: wiring.found,
            next_host: wiring.next_host,
            ready: false,
            pending: HashMap::new(),
            next_request: 1,
            models: Vec::new(),
            model: None,
            mode: None,
            modes_said: Vec::new(),
            effort: None,
            pinned_effort: None,
            efforts_said: None,
            arguments: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            sessions: Vec::new(),
            said: false,
            lost,
            fresh,
            title: None,
            usage: Usage::default(),
            answering_model: None,
            states: false,
            turn: None,
            draining: false,
            listening: false,
            queue: VecDeque::new(),
            sent: HashMap::new(),
            prompts: HashMap::new(),
            last_entry: None,
            preface_due,
            permissions: Vec::new(),
            next_permission: 1,
            calls: HashMap::new(),
            tasks: HashMap::new(),
            tasks_list: records::tools::Tasks::default(),
            streams: HashMap::new(),
            last_chunk: None,
            last_said: None,
            moving: None,
            commands_closed: false,
            finished: false,
        }
    }

    /// Drive the session until it is closed.
    pub fn run(mut self) {
        self.start();
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

    /// The greeting, and the conversation gone on with replayed meanwhile.
    fn start(&mut self) {
        if let Some(hello) = self.found.take() {
            return self.take_up(hello);
        }
        let id = self.request_id();
        let link = self.link.id;
        self.pending.insert(id.clone(), (link, Pending::Initialize));
        self.link
            .write(&wire::control_request(&id, wire::initialize()));
        if let Some(session) = self.lost.take() {
            self.send(Update::Notice(
                Level::Warning,
                format!("Session {session} has no record here, so this is a new conversation."),
            ));
        }
        let Some(session) = self.options.resume.clone() else {
            return;
        };
        self.said = true;
        self.session = Some(session.clone());
        self.sessions.push(session.clone());
        let record = self
            .home
            .as_deref()
            .and_then(|home| records::transcript::record(home, &self.options.cwd, &session));
        match record {
            Some(record) => {
                for update in records::transcript::replay(&record) {
                    if let Update::Title(title) = &update {
                        self.title = Some(title.clone());
                    }
                    self.send(update);
                }
            }
            None => self.send(Update::Notice(
                Level::Warning,
                format!(
                    "The record of session {session} was not found here, so what was said in it is not shown."
                ),
            )),
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Message(link, message) => {
                if link == self.link.id {
                    self.on_message(message);
                } else if self
                    .moving
                    .as_ref()
                    .is_some_and(|moving| moving.link.id == link)
                {
                    self.on_moving_message(message);
                }
            }
            Event::StdoutClosed(link) => self.with_link(link, |link| {
                link.stdout_closed = true;
                link.ending.get_or_insert_with(Instant::now);
            }),
            Event::StderrClosed(link) => self.with_link(link, |link| link.stderr_closed = true),
            Event::KeeperGone(link) => self.with_link(link, |link| {
                link.vanished = true;
                link.ending.get_or_insert_with(Instant::now);
            }),
            Event::Replaced(link) => self.with_link(link, |link| {
                link.replaced = true;
                link.ending.get_or_insert_with(Instant::now);
            }),
            Event::Exited(link, exit) => self.with_link(link, |link| {
                link.exit = Some(exit);
                link.ending.get_or_insert_with(Instant::now);
            }),
            Event::Command(command) => self.on_command(command),
            Event::Request(request) => self.on_interface_request(request),
            Event::CommandsClosed => self.commands_closed = true,
        }
    }

    /// Change the link of number `id`, the one in use or the one an edit
    /// starts; a link let go of is no concern any more.
    fn with_link(&mut self, id: u64, change: impl FnOnce(&mut Link)) {
        if self.link.id == id {
            change(&mut self.link);
        } else if let Some(moving) = &mut self.moving
            && moving.link.id == id
        {
            change(&mut moving.link);
        }
    }

    /// When the loop must wake up even if nothing happens.
    fn next_deadline(&self) -> Option<Instant> {
        let mut deadlines = Vec::new();
        deadlines.extend(self.link.ending.map(|since| since + DRAIN));
        if let Some(moving) = &self.moving {
            deadlines.extend(moving.link.ending.map(|since| since + DRAIN));
        }
        if self.commands_closed {
            deadlines.push(Instant::now() + ORPHAN_POLL);
        }
        deadlines.into_iter().min()
    }

    /// What is due after an event, or after a wait in which none came.
    fn tick(&mut self, idle: bool) {
        if self.finished {
            return;
        }
        let now = Instant::now();
        let listening = self.listens();
        if listening != self.listening {
            self.listening = listening;
            self.send(Update::Offers(self.offers()));
        }
        if let Some(moving) = &self.moving
            && let Some(since) = moving.link.ending
            && (moving.link.drained() || (idle && now >= since + DRAIN))
        {
            let reason = describe_end(&self.program, &moving.link);
            self.move_failed(reason);
        }
        if let Some(since) = self.link.ending {
            // Its last messages and last words may still be in the pipes, or
            // already read and waiting here: give up on them only when
            // nothing is left to handle.
            if self.link.drained() || (idle && now >= since + DRAIN) {
                if self.link.replaced {
                    return self.taken_elsewhere();
                }
                let reason = describe_end(&self.program, &self.link);
                if self.link.vanished
                    && let Some(socket) = self.link.keeper_socket()
                {
                    crate::host::note(socket, "app", &format!("the session ends: {reason}"));
                }
                return self.close(Some(reason));
            }
        }
        // Nobody can send a command and nobody reads the updates.
        if self.commands_closed && self.updates.is_closed() {
            if self.link.is_kept() {
                self.leave();
            } else {
                self.close(None);
            }
        }
    }

    // Outgoing.

    fn send(&mut self, update: Update) {
        // Unbounded, so this fails only when nobody listens any more.
        let _ = self.updates.try_send(update);
    }

    fn request_id(&mut self) -> String {
        let id = format!("kou-{}", self.next_request);
        self.next_request += 1;
        id
    }

    /// Send a control request of ours on the link in use.
    fn request(&mut self, request: Value, pending: Pending) {
        let id = self.request_id();
        self.pending.insert(id.clone(), (self.link.id, pending));
        self.link.write(&wire::control_request(&id, request));
    }

    fn offers(&self) -> Offers {
        Offers {
            steer: true,
            rewind: true,
            listening: self.listening,
            arguments: self.arguments.clone(),
        }
    }

    /// Whether a prompt sent now goes at once though a turn is open: the
    /// model has given its answer and only what runs in the background
    /// keeps the turn open.
    fn listens(&self) -> bool {
        self.turn
            .as_ref()
            .is_some_and(|turn| turn.answered && !turn.cancelled)
    }

    /// The session as it stands now, for the interface.
    fn announce(&mut self) {
        self.modes_said = self.modes();
        self.send(Update::Ready {
            // (Named once there is something of it to go on with.)
            session: self
                .session
                .clone()
                .filter(|_| self.said)
                .unwrap_or_default(),
            capabilities: Capabilities {
                images: true,
                embedded_files: true,
                resume: true,
            },
            modes: self.modes_said.clone(),
            mode: self.mode.clone(),
            models: self
                .models
                .iter()
                .map(|model| model.choice.clone())
                .collect(),
            model: self.model.clone(),
        });
    }

    // From Claude Code.

    fn on_message(&mut self, message: Value) {
        if let Some(session) = text_of(&message, "session_id") {
            let session = session.to_owned();
            self.note_session(&session);
        }
        match text_of(&message, "type") {
            Some("control_response") => self.on_control_response(&message),
            Some("control_request") => self.on_control_request(&message),
            Some("control_cancel_request") => self.on_control_cancel(&message),
            Some("system") => self.on_system(&message),
            Some("stream_event") => self.on_stream_event(&message),
            Some("assistant") => self.on_assistant(&message),
            Some("user") => self.on_user(&message),
            Some("result") => self.on_result(&message),
            kind => log::debug!(
                "{} wrote a message of an unknown type: {kind:?}",
                self.program
            ),
        }
    }

    /// The session's id, as Claude Code's messages carry it: a new one is
    /// said to the interface.
    fn note_session(&mut self, session: &str) {
        if self.session.as_deref() == Some(session) {
            return;
        }
        self.session = Some(session.to_owned());
        if !self.sessions.iter().any(|known| known == session) {
            self.sessions.push(session.to_owned());
        }
        for before in self.prompts.values_mut() {
            if let Before::At {
                session: unknown @ None,
                ..
            } = before
            {
                *unknown = Some(session.to_owned());
            }
        }
        if self.ready && self.said {
            self.announce();
        }
    }

    fn on_control_response(&mut self, message: &Value) {
        let response = message.get("response").unwrap_or(&Value::Null);
        let Some(id) = text_of(response, "request_id") else {
            log::debug!("{} answered no request it names", self.program);
            return;
        };
        let answer = match text_of(response, "subtype") {
            Some("error") => Err(text_of(response, "error")
                .unwrap_or("an error it did not describe")
                .to_owned()),
            _ => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
        };
        let Some((link, pending)) = self.pending.remove(id) else {
            log::debug!("{} answered a request that is not open: {id}", self.program);
            return;
        };
        if link != self.link.id {
            return;
        }
        match pending {
            Pending::Initialize => self.on_initialized(answer),
            Pending::Interrupt => {
                if let Err(error) = answer {
                    log::warn!("{} did not take the stop: {error}", self.program);
                }
            }
            Pending::SetMode(mode) => match answer {
                Ok(_) => self.note_mode(&mode),
                Err(error) => self.send(Update::Notice(
                    Level::Warning,
                    format!("Claude Code did not switch to the mode {mode}: {error}"),
                )),
            },
            Pending::SetModel(model) => match answer {
                Ok(_) => self.note_model(&model),
                Err(error) => self.send(Update::Notice(
                    Level::Warning,
                    format!("Claude Code did not switch to the model {model}: {error}"),
                )),
            },
            Pending::SetEffort(level) => self.on_effort_set(level, answer),
            Pending::Settings(asked) => self.on_settings(answer, asked),
            Pending::Move => {}
        }
    }

    fn on_initialized(&mut self, answer: Result<Value, String>) {
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) => {
                return self.close(Some(format!("{} did not start: {error}", self.program)));
            }
        };
        self.models = wire::models(&answer);
        if self.model.is_none() {
            self.model = wire::current_model(&self.models, self.options.model.as_deref());
        }
        // The mode it starts in, which the user's settings may have set.
        // (An older Claude Code does not say it when greeted: then it is
        // taken to be its usual one until its first turn says otherwise.)
        if let Some(mode) = text_of(&answer, "current_permission_mode") {
            self.mode = Some(mode.to_owned());
        }
        self.mode.get_or_insert_with(|| "default".to_owned());
        self.ready = true;
        self.announce();
        if self.options.resume.is_some() {
            self.note_title();
        }
        let (commands, arguments) = wire::commands(answer.get("commands"));
        self.arguments = arguments;
        self.send(Update::Commands(commands));
        self.send(Update::Offers(self.offers()));
        // The level of effort in force is not in the greeting: asked before
        // any prompt goes.
        self.request(wire::get_settings(), Pending::Settings(None));
        self.next_prompt();
    }

    fn on_control_request(&mut self, message: &Value) {
        let Some(id) = text_of(message, "request_id").map(str::to_owned) else {
            log::debug!(
                "{} asked something without naming the request",
                self.program
            );
            return;
        };
        let request = message.get("request").unwrap_or(&Value::Null);
        match text_of(request, "subtype") {
            Some("can_use_tool") => self.on_permission_request(id, request),
            Some("elicitation") => self.on_elicitation(id, request),
            Some("request_user_dialog") => {
                // Only sent for a kind the client declared, and none is: as
                // the Python SDK answers what it has no handler for. (An
                // error leaves the dialog to its own deadline.)
                let kind = text_of(request, "dialog_kind").unwrap_or("none");
                self.link.write(&wire::failure(
                    &id,
                    &format!("This client shows no dialog of kind {kind}"),
                ));
            }
            subtype => {
                // Answered, not left hanging: Claude Code goes on without it.
                let subtype = subtype.unwrap_or("none");
                log::debug!("{} asked something unknown here: {subtype}", self.program);
                self.link.write(&wire::failure(
                    &id,
                    &format!("Unsupported control request subtype: {subtype}"),
                ));
            }
        }
    }

    /// Claude Code takes back a question of its own: it was answered
    /// elsewhere, or the work it was for stopped.
    fn on_control_cancel(&mut self, message: &Value) {
        if let Some(id) = text_of(message, "request_id") {
            self.withdraw(id);
        }
    }

    fn on_system(&mut self, message: &Value) {
        match text_of(message, "subtype") {
            Some("init") => {
                if let Some(mode) = text_of(message, "permissionMode") {
                    self.note_mode(mode);
                }
                if let Some(model) = text_of(message, "model") {
                    self.note_running_model(model);
                }
                if let Some(effort) = wire::effort_field(message) {
                    self.note_effort(effort);
                }
                if let Some(capabilities) = message.get("capabilities").and_then(Value::as_array) {
                    self.capabilities = capabilities
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                }
            }
            Some("session_state_changed") => self.on_state(text_of(message, "state")),
            Some("status") => {
                if let Some(mode) = text_of(message, "permissionMode") {
                    self.note_mode(mode);
                }
                if text_of(message, "status") == Some("requesting") && !self.draining {
                    self.at_work();
                }
                if text_of(message, "compact_result") == Some("failed") {
                    let text = match text_of(message, "compact_error") {
                        Some(error) => format!("Compacting the conversation failed: {error}"),
                        None => "Compacting the conversation failed".to_owned(),
                    };
                    self.notice(Level::Warning, text);
                }
            }
            Some("compact_boundary") => {
                self.note_entry(message);
                self.notice(Level::Info, "The conversation was compacted".to_owned());
            }
            Some("commands_changed") => {
                let (commands, arguments) = wire::commands(message.get("commands"));
                self.send(Update::Commands(commands));
                if arguments != self.arguments {
                    self.arguments = arguments;
                    self.send(Update::Offers(self.offers()));
                }
            }
            Some("local_command_output") if !self.draining => {
                // What a command of Claude Code's own (`/cost`, `/usage`)
                // answers, shown as its words.
                if let Some(text) = text_of(message, "content") {
                    let key = (text_of(message, "uuid").unwrap_or_default().to_owned(), 0);
                    self.say(&None, Kind::Text, key, text.to_owned());
                }
            }
            Some("informational") => {
                let level = match text_of(message, "level") {
                    Some("warning") => Level::Warning,
                    Some("notice" | "suggestion") => Level::Info,
                    // Shown only in Claude Code's transcript mode.
                    _ => return,
                };
                if let Some(text) = text_of(message, "content") {
                    self.notice(level, text.to_owned());
                }
            }
            Some("task_started") => self.on_task_started(message),
            Some("task_updated") => self.on_task_updated(message),
            Some("task_notification") => self.on_task_notification(message),
            subtype => log::trace!("{} said {subtype:?}, which has no place here", self.program),
        }
    }

    fn on_state(&mut self, state: Option<&str>) {
        self.states = true;
        match state {
            Some("idle") => {
                // A turn begun before Claude Code said it was at work may
                // hear the idle of before, or that of the work a second stop
                // let go of: only its own ends it.
                let drained = std::mem::take(&mut self.draining);
                let over = self
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.stop.is_some() || (turn.ran && !drained));
                if over {
                    self.end_turn();
                }
            }
            // (What a stop let go of still runs, for no turn of the user's.)
            Some(_) if self.draining => {}
            Some(_) => match &mut self.turn {
                Some(turn) => turn.ran = true,
                None => self.own_turn_starts(),
            },
            None => {}
        }
    }

    fn on_result(&mut self, message: &Value) {
        self.note_usage(message);
        if self.draining {
            if !self.states {
                self.draining = false;
            }
            return;
        }
        let Some(turn) = &mut self.turn else {
            return;
        };
        turn.stop = Some(wire::stop(message, turn.cancelled));
        turn.answered = true;
        // Everything of the answer has come whole by now. (A subagent left
        // running in the background goes on streaming.)
        if let Some(stream) = self.streams.get_mut(&None) {
            stream.clear();
        }
        // A Claude Code that says no states ends its turn here, unless a
        // message handed to it is still to be taken: it runs that next, and
        // the interface, told the turn is over, would send it again.
        let steered = self
            .sent
            .values()
            .any(|sent| matches!(sent, Sent::Steer(_)));
        if !self.states && !steered {
            self.end_turn();
        }
    }

    /// The figures a `result` carries: what the session cost so far, and the
    /// context window of the model that answered.
    fn note_usage(&mut self, result: &Value) {
        if let Some(cost) = result.get("total_cost_usd").and_then(Value::as_f64) {
            self.usage.cost_usd = Some(cost);
        }
        if let Some(models) = result.get("modelUsage").and_then(Value::as_object) {
            let sum = |key: &str| {
                models
                    .values()
                    .filter_map(|usage| usage.get(key).and_then(Value::as_u64))
                    .sum::<u64>()
            };
            if !models.is_empty() {
                self.usage.input_tokens = Some(sum("inputTokens"));
                self.usage.output_tokens = Some(sum("outputTokens"));
            }
            let window = |usage: &Value| usage.get("contextWindow").and_then(Value::as_u64);
            let answering = self
                .answering_model
                .as_deref()
                .and_then(|model| models.get(model))
                .and_then(window);
            if let Some(limit) = answering.or_else(|| models.values().filter_map(window).max()) {
                self.usage.context_limit = Some(limit);
            }
        }
        self.send(Update::Usage(self.usage));
    }

    fn on_stream_event(&mut self, message: &Value) {
        let parent = text_of(message, "parent_tool_use_id").map(str::to_owned);
        if self.draining && parent.is_none() {
            return;
        }
        let event = message.get("event").unwrap_or(&Value::Null);
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match text_of(event, "type") {
            Some("message_start") => {
                if parent.is_none() {
                    self.own_turn_starts();
                    self.at_work();
                }
                let id = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.streams.entry(parent).or_default().start_message(id);
            }
            Some("content_block_start") => {
                let block = event.get("content_block").unwrap_or(&Value::Null);
                match text_of(block, "type") {
                    // Its message began before the session was there (one
                    // found again): what streamed of it is not known, so
                    // it is shown whole when its whole message comes.
                    Some("text" | "thinking") if !self.stream(&parent).started() => {
                        self.stream(&parent).start_block(index, Open::Other);
                    }
                    Some("text") => {
                        self.stream(&parent)
                            .start_block(index, Open::Text(Kind::Text));
                        if let Some(text) = text_of(block, "text") {
                            self.streamed(&parent, index, Kind::Text, text);
                        }
                    }
                    Some("thinking") => {
                        self.stream(&parent)
                            .start_block(index, Open::Text(Kind::Thinking));
                        if let Some(text) = text_of(block, "thinking") {
                            self.streamed(&parent, index, Kind::Thinking, text);
                        }
                    }
                    // (A tool the API runs itself streams its input alike.)
                    Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                        let (Some(id), Some(name)) = (text_of(block, "id"), text_of(block, "name"))
                        else {
                            return;
                        };
                        let id = id.to_owned();
                        self.stream(&parent)
                            .start_block(index, Open::Tool(id.clone()));
                        self.call_begins(&id, name, parent);
                    }
                    _ => self.stream(&parent).start_block(index, Open::Other),
                }
            }
            Some("content_block_delta") => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                let open = self.stream(&parent).block(index).cloned();
                match (text_of(delta, "type"), open) {
                    (Some("text_delta"), Some(Open::Text(Kind::Text))) => {
                        if let Some(text) = text_of(delta, "text") {
                            self.streamed(&parent, index, Kind::Text, text);
                        }
                    }
                    (Some("thinking_delta"), Some(Open::Text(Kind::Thinking))) => {
                        if let Some(text) = text_of(delta, "thinking") {
                            self.streamed(&parent, index, Kind::Thinking, text);
                        }
                    }
                    (Some("input_json_delta"), Some(Open::Tool(id))) => {
                        let grown = match (text_of(delta, "partial_json"), self.calls.get_mut(&id))
                        {
                            (Some(part), Some(call)) => call.json.push(part),
                            _ => None,
                        };
                        if let Some(input) = grown {
                            self.call_grows(&id, &input);
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                if let Some(Open::Tool(id)) = self.stream(&parent).stop_block(index) {
                    let input = match self.calls.get(&id) {
                        Some(call) if !call.complete => {
                            serde_json::from_str(call.json.text()).unwrap_or_else(|_| json!({}))
                        }
                        _ => return,
                    };
                    self.call_complete(&id, input);
                }
            }
            _ => {}
        }
    }

    fn stream(&mut self, parent: &Option<String>) -> &mut Stream {
        self.streams.entry(parent.clone()).or_default()
    }

    /// Text streamed into block `index` of the answer of `parent` (Claude
    /// Code's own when `None`).
    fn streamed(&mut self, parent: &Option<String>, index: u64, kind: Kind, text: &str) {
        if text.is_empty() {
            return;
        }
        let key = self.stream(parent).streamed(index, kind, text);
        self.say(parent, kind, key, text.to_owned());
    }

    /// Text of an answer, said: Claude Code's own as its answer or thought,
    /// a subagent's as what it said (a subagent's thinking is not shown).
    fn say(&mut self, parent: &Option<String>, kind: Kind, key: Key, mut text: String) {
        if text.is_empty() {
            return;
        }
        match parent {
            None => {
                // Another block of the same kind right after one is another
                // paragraph.
                if let Some((last_kind, last_key)) = &self.last_chunk
                    && *last_kind == kind
                    && *last_key != key
                {
                    text.insert_str(0, "\n\n");
                }
                self.last_chunk = Some((kind, key));
                self.send(match kind {
                    Kind::Text => Update::AssistantChunk(text),
                    Kind::Thinking => Update::ThoughtChunk(text),
                });
            }
            Some(call) if kind == Kind::Text => {
                if let Some((last_call, last_key)) = &self.last_said
                    && last_call == call
                    && *last_key != key
                {
                    text.insert_str(0, "\n\n");
                }
                self.last_said = Some((call.clone(), key));
                self.send(Update::AgentSaid {
                    call: call.clone(),
                    text,
                });
            }
            Some(_) => {}
        }
    }

    fn on_assistant(&mut self, message: &Value) {
        let parent = text_of(message, "parent_tool_use_id").map(str::to_owned);
        if self.draining && parent.is_none() {
            return;
        }
        let body = message.get("message").unwrap_or(&Value::Null);
        if parent.is_none() {
            self.own_turn_starts();
            self.at_work();
            self.note_entry(message);
            self.note_context(body);
        }
        let id = text_of(body, "id").unwrap_or_default().to_owned();
        let uuid = text_of(message, "uuid").unwrap_or_default().to_owned();
        let Some(blocks) = body.get("content").and_then(Value::as_array) else {
            return;
        };
        for (position, block) in blocks.iter().enumerate() {
            let (kind, field) = match text_of(block, "type") {
                Some("text") => (Kind::Text, "text"),
                Some("thinking") => (Kind::Thinking, "thinking"),
                Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                    let (Some(call), Some(name)) = (text_of(block, "id"), text_of(block, "name"))
                    else {
                        continue;
                    };
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    self.whole_call(call, name, input, parent.clone());
                    continue;
                }
                // What a tool the API ran itself gave (`web_search_tool_result`
                // and its kin), which comes in the answer.
                Some(kind) if kind.ends_with("_tool_result") => {
                    self.server_result(block);
                    continue;
                }
                _ => continue,
            };
            let Some(text) = text_of(block, field) else {
                continue;
            };
            let (rest, key) = self.stream(&parent).remainder(&id, kind, text);
            let key = key.unwrap_or_else(|| (uuid.clone(), position as u64));
            self.say(&parent, kind, key, rest);
        }
    }

    /// The context an answer of Claude Code's own was written in, as its
    /// usage counts it.
    fn note_context(&mut self, body: &Value) {
        let model = text_of(body, "model");
        if model == Some("<synthetic>") {
            return;
        }
        let Some(usage) = body.get("usage") else {
            return;
        };
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let used = count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens")
            + count("output_tokens");
        self.answering_model = model.map(str::to_owned).or(self.answering_model.take());
        if used > 0 && self.usage.context_used != Some(used) {
            self.usage.context_used = Some(used);
            self.send(Update::Usage(self.usage));
        }
    }

    fn on_user(&mut self, message: &Value) {
        if message.get("isReplay").and_then(Value::as_bool) == Some(true) {
            return self.on_echo(message);
        }
        let parent = text_of(message, "parent_tool_use_id").map(str::to_owned);
        if self.draining && parent.is_none() {
            return;
        }
        if parent.is_none() {
            self.note_entry(message);
        }
        let Some(blocks) = message
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return;
        };
        let results: Vec<&Value> = blocks
            .iter()
            .filter(|block| text_of(block, "type") == Some("tool_result"))
            .collect();
        // What Claude Code keeps of a result beside the block is the
        // message's, so it goes with a message's only result.
        let structured = message
            .get("tool_use_result")
            .filter(|_| results.len() == 1);
        for block in results {
            let Some(id) = text_of(block, "tool_use_id") else {
                continue;
            };
            let content = block.get("content").cloned().unwrap_or_else(|| json!(""));
            let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
            let (name, input, plan) = match self.calls.get(id) {
                Some(call) => (call.name.clone(), call.input.clone(), call.plan),
                None => (String::new(), json!({}), false),
            };
            if plan {
                let changed = self
                    .tasks_list
                    .apply(&name, &input, &content, structured, failed);
                if let Some(steps) = changed {
                    self.plan(steps);
                }
                continue;
            }
            let mut patch = records::tools::result(id, &name, &input, &content, structured, failed);
            if let Some(title) = patch.title.take() {
                patch.title = Some(records::tools::relative(&self.options.cwd, &title));
            }
            // An agent the result says was started in the background.
            if patch.background == Some(true)
                && let Some(call) = self.calls.get_mut(id)
            {
                call.background = true;
            }
            self.send(Update::ToolPatch(patch));
        }
    }

    /// A message of the user's, written back as Claude Code takes it.
    fn on_echo(&mut self, message: &Value) {
        let uuid = text_of(message, "uuid").map(str::to_owned);
        let sent = uuid.as_deref().and_then(|uuid| self.sent.remove(uuid));
        match sent {
            Some(Sent::Prompt) => {
                // It is what runs now.
                self.draining = false;
                self.at_work();
            }
            Some(Sent::Steer(blocks)) => {
                self.at_work();
                self.last_chunk = None;
                for block in blocks {
                    self.send(Update::UserChunk(block));
                }
                if let Some(uuid) = &uuid {
                    self.send(Update::PromptId(uuid.clone()));
                }
            }
            None => {
                // Typed elsewhere (another client of the session): shown as
                // the user's. Claude Code's own notes are not.
                let human = message.pointer("/origin/kind").and_then(Value::as_str)
                    == Some("human")
                    && message.get("isSynthetic").and_then(Value::as_bool) != Some(true)
                    && text_of(message, "parent_tool_use_id").is_none();
                if !human || self.draining {
                    return;
                }
                let blocks = user_blocks(message.pointer("/message/content"));
                if blocks.is_empty() {
                    return;
                }
                self.at_work();
                self.last_chunk = None;
                for block in blocks {
                    self.send(Update::UserChunk(block));
                }
                if let Some(uuid) = &uuid {
                    self.send(Update::PromptId(uuid.clone()));
                }
            }
        }
        if let Some(uuid) = uuid {
            let before = match &self.last_entry {
                Some(entry) => Before::At {
                    session: self.session.clone(),
                    entry: entry.clone(),
                },
                None if self.fresh => Before::Nothing,
                None => Before::Unknown,
            };
            self.prompts.entry(uuid.clone()).or_insert(before);
            self.last_entry = Some(uuid);
        }
    }

    /// An entry of the conversation of Claude Code's own (no subagent's).
    fn note_entry(&mut self, message: &Value) {
        if let Some(uuid) = text_of(message, "uuid") {
            self.last_entry = Some(uuid.to_owned());
        }
    }

    /// Claude Code (no subagent of its) says or does something: a turn it
    /// had answered is at work again.
    fn at_work(&mut self) {
        if let Some(turn) = &mut self.turn {
            turn.answered = false;
        }
    }

    /// Claude Code is at work with no turn open: it goes on by itself (a
    /// task it left running has ended), in a turn of its own.
    fn own_turn_starts(&mut self) {
        if self.turn.is_some() || self.draining || !self.ready || self.moving.is_some() {
            return;
        }
        self.turn = Some(Turn {
            ran: true,
            ..Turn::default()
        });
        self.last_chunk = None;
        self.send(Update::TurnStarted);
    }

    /// The turn is over.
    fn end_turn(&mut self) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        let stop = if turn.cancelled {
            Stop::Cancelled
        } else {
            turn.stop.unwrap_or(Stop::Finished)
        };
        // Nothing can be permitted any more.
        self.answer_open(CANCELLED);
        // Only what runs on in the background is still heard of.
        let running: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, call)| call.background && !call.ended)
            .map(|(id, _)| id.clone())
            .collect();
        self.calls.retain(|id, call| {
            running.contains(id)
                || call
                    .parent
                    .as_ref()
                    .is_some_and(|parent| running.contains(parent))
        });
        self.last_chunk = None;
        self.last_said = None;
        if let Some(stream) = self.streams.get_mut(&None) {
            stream.clear();
        }
        self.send(Update::TurnEnded(stop));
        self.note_title();
    }

    /// The conversation's name, as its record has it: said when it is not
    /// what was said last. (Claude Code names a conversation by itself
    /// after its first exchanges, and `/rename` names it.)
    fn note_title(&mut self) {
        let (Some(home), Some(session)) = (&self.home, &self.session) else {
            return;
        };
        if !self.said {
            return;
        }
        let title = records::transcript::record(home, &self.options.cwd, session)
            .and_then(|record| records::transcript::title(&record));
        if let Some(title) = title
            && self.title.as_ref() != Some(&title)
        {
            self.title = Some(title.clone());
            self.send(Update::Title(title));
        }
    }

    fn notice(&mut self, level: Level, text: String) {
        self.last_chunk = None;
        self.send(Update::Notice(level, text));
    }

    // From the interface.

    fn on_command(&mut self, command: Command) {
        if self.link.ending.is_some() && command != Command::Close {
            // Claude Code is gone; the reason is on its way.
            return;
        }
        match command {
            Command::Prompt(blocks) => {
                if self.ready && self.moving.is_none() {
                    self.write_prompt(blocks, None);
                } else {
                    self.queue.push_back(Queued { blocks, uuid: None });
                }
            }
            Command::Cancel => self.cancel(),
            Command::Permit { request, option } => self.permit(request, &option),
            Command::SetMode(mode) => self.set_mode(mode),
            Command::SetModel(model) => {
                self.request(
                    json!({"subtype": "set_model", "model": model}),
                    Pending::SetModel(model),
                );
            }
            Command::SetEffort(level) => self.set_effort(level),
            Command::SetAccess(_) => {}
            Command::Answer { request, answers } => self.answer(request, answers),
            Command::Close => self.close(None),
        }
    }

    /// Send the prompts that waited for the session.
    fn next_prompt(&mut self) {
        while self.ready && self.moving.is_none() {
            let Some(queued) = self.queue.pop_front() else {
                return;
            };
            self.write_prompt(queued.blocks, queued.uuid);
        }
    }

    /// Write a prompt. To an idle session it begins a turn; into a turn
    /// that is open it is taken in, as one typed into Claude Code's own
    /// interface is. `named` is the name an edit gave it.
    fn write_prompt(&mut self, blocks: Vec<Block>, named: Option<String>) {
        if wire::prompt_content(&blocks).is_empty() {
            if named.is_none() {
                self.send(Update::PromptTaken(None));
            }
            return;
        }
        let uuid = named
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let note = if std::mem::take(&mut self.preface_due) {
            self.options.preface.clone()
        } else {
            None
        };
        let session = self.session.clone().unwrap_or_default();
        self.link.write(&wire::user_message(
            &blocks,
            note.as_deref(),
            &uuid,
            &session,
        ));
        self.prompt_written(uuid, named.is_none());
    }

    /// A prompt named `uuid` was written to Claude Code: its echo is
    /// awaited, and a turn begins unless one is open. `taken`: the
    /// interface is told the prompt's name.
    fn prompt_written(&mut self, uuid: String, taken: bool) {
        self.sent.insert(uuid.clone(), Sent::Prompt);
        let first = !std::mem::replace(&mut self.said, true);
        if taken {
            self.send(Update::PromptTaken(Some(uuid)));
        }
        // The thread shows the prompt where it stands: what is said after
        // it begins anew.
        self.last_chunk = None;
        match &mut self.turn {
            Some(turn) => turn.answered = false,
            None => {
                self.turn = Some(Turn::default());
                self.send(Update::TurnStarted);
            }
        }
        // From here Claude Code keeps the conversation: its id, if it has
        // said it already, is the interface's to go on with.
        if first && self.ready && self.session.is_some() {
            self.announce();
        }
    }

    fn on_interface_request(&mut self, request: Request) {
        if self.link.ending.is_some() {
            return;
        }
        match request {
            Request::Steer(blocks) => self.steer(blocks),
            Request::Rewind { prompt, blocks } => {
                if let Err(reason) = self.rewind(prompt, blocks) {
                    self.send(Update::Rewound(Err(reason)));
                }
            }
        }
    }

    /// Hand a message to the turn at work: written as a prompt is, and
    /// shown once Claude Code takes it. One the turn ended before is not
    /// sent: the interface, which still holds it, sends it as a prompt.
    fn steer(&mut self, blocks: Vec<Block>) {
        if self.turn.is_none() || !self.ready || self.moving.is_some() {
            return;
        }
        if wire::prompt_content(&blocks).is_empty() {
            return;
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let session = self.session.clone().unwrap_or_default();
        self.link
            .write(&wire::user_message(&blocks, None, &uuid, &session));
        self.sent.insert(uuid, Sent::Steer(blocks));
    }

    fn cancel(&mut self) {
        let Some(turn) = &mut self.turn else {
            return;
        };
        if turn.cancelled {
            // Asked twice: the turn is over for the interface now, and
            // whatever Claude Code still says of it is dropped.
            self.end_turn();
            self.draining = true;
            return;
        }
        turn.cancelled = true;
        // Stop means everything: the messages handed to the turn that it
        // has not taken yet are dropped too (the interface still holds
        // them), as is what runs in the background.
        self.request(
            json!({"subtype": "interrupt", "cancel_queued": true}),
            Pending::Interrupt,
        );
        self.sent.retain(|_, sent| !matches!(sent, Sent::Steer(_)));
        self.answer_open(CANCELLED);
    }

    // The end.

    /// End Claude Code and everything it started, and say so. `reason` is
    /// why, unless the interface asked for it.
    fn close(&mut self, reason: Option<String>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.turn.take().is_some() {
            self.send(Update::TurnEnded(match &reason {
                Some(reason) => Stop::Failed(reason.clone()),
                None => Stop::Cancelled,
            }));
        }
        // Open questions are answered rather than dropped.
        self.answer_open(CANCELLED);
        self.pending.clear();
        self.queue.clear();
        if let Some(moving) = self.moving.take() {
            moving.link.stop(Duration::ZERO);
        }
        // Its input closed, a Claude Code that leaves by itself gets to
        // save what it has; one that does not is told to go. (A kept one's
        // host is told to stop it, and itself.)
        self.link.end(LEAVE, GRACE);
        self.send(Update::Closed(reason));
        self.updates.close();
        self.commands.close();
        self.requests.close();
    }

    /// Another connection has taken Claude Code up (this chat opened again,
    /// or another window of the app): it works on there. Nothing of it is
    /// this session's to answer or end any more; the interface is told why
    /// the session is over.
    fn taken_elsewhere(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(socket) = self.link.keeper_socket() {
            crate::host::note(socket, "app", "the session ends: taken up elsewhere");
        }
        if self.turn.take().is_some() {
            self.send(Update::TurnEnded(Stop::Cancelled));
        }
        // What Claude Code asked is the new connection's to answer.
        self.permissions.clear();
        self.pending.clear();
        self.queue.clear();
        if let Some(moving) = self.moving.take() {
            moving.link.stop(Duration::ZERO);
        }
        self.link.close_input();
        self.send(Update::Notice(
            Level::Warning,
            "Claude Code was taken up elsewhere (this chat was opened again, or another window of the app took it): it goes on there.".into(),
        ));
        self.send(Update::Closed(None));
        self.updates.close();
        self.commands.close();
        self.requests.close();
    }

    /// Nobody holds the session any more, and nobody closed it (the app
    /// quit, or its window was made anew): a kept Claude Code works on, as
    /// it is, for the session to be found again. Nothing is answered for
    /// the user and nothing is stopped, except the process of an edit not
    /// taken up yet.
    fn leave(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(moving) = self.moving.take() {
            moving.link.stop(Duration::ZERO);
        }
        self.link.close_input();
        self.updates.close();
        self.commands.close();
        self.requests.close();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Only reached with the session unfinished if its thread panicked.
        if !self.finished {
            // (A kept one is left to be found again.)
            if !self.link.is_kept() {
                self.link.runner().kill();
            }
            if let Some(moving) = &self.moving {
                moving.link.runner().kill();
            }
            self.send(Update::Closed(Some(
                "the connection to Claude Code failed inside this app".into(),
            )));
        }
    }
}

/// Why a process that was not asked to went away, with what it last said
/// on its standard error.
fn describe_end(program: &str, link: &Link) -> String {
    // A keeper that ended left word of why, unless another has started at
    // its socket since or the app swept it away.
    let last = link
        .vanished
        .then(|| link.keeper_socket().and_then(crate::host::last))
        .flatten();
    let mut reason = match (link.exit, &last) {
        (Some(exit), _) => format!("{program} exited with {}", exit.describe()),
        (None, Some(last)) => format!(
            "the keeper of {program} ended ({}): {program} {}",
            last.reason, last.exit
        ),
        (None, None) if link.replaced => format!("{program} was taken up elsewhere"),
        (None, None) if link.vanished => {
            format!("the connection to the keeper of {program} was lost")
        }
        (None, None) => format!("{program} closed its output"),
    };
    let mut last_words = link.last_words();
    if last_words.is_empty()
        && let Some(last) = last
    {
        last_words = last.stderr;
    }
    if !last_words.is_empty() {
        reason.push_str(":\n");
        reason.push_str(&last_words.join("\n"));
    }
    reason
}

/// The blocks of a user message as the conversation shows them: its text
/// and its pictures.
fn user_blocks(content: Option<&Value>) -> Vec<Block> {
    use base64::Engine as _;
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![Block::Text(text.clone())],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| match text_of(block, "type") {
                Some("text") => text_of(block, "text").map(|text| Block::Text(text.to_owned())),
                Some("image") => {
                    let source = block.get("source")?;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(text_of(source, "data")?)
                        .ok()?;
                    Some(Block::Image {
                        mime: text_of(source, "media_type")
                            .unwrap_or("image/png")
                            .to_owned(),
                        data: data.into(),
                    })
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_message_shows_its_text_and_pictures() {
        let content = json!([
            {"type": "text", "text": "Look"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/gif", "data": "R0lG"}},
            {"type": "tool_result", "tool_use_id": "x"},
        ]);
        assert_eq!(
            user_blocks(Some(&content)),
            [
                Block::Text("Look".into()),
                Block::Image {
                    mime: "image/gif".into(),
                    data: b"GIF".to_vec().into(),
                },
            ]
        );
        assert_eq!(user_blocks(Some(&json!("Hi"))), [Block::Text("Hi".into())]);
        assert!(user_blocks(None).is_empty());
    }
}
