//! One kou session behind a pair of channels: a thread that replays the
//! stored conversation, starts a runner for each prompt, hands what the
//! runner says to the fold, and stops the runner when asked.
//!
//! Runs are attached, as the terminal cockpit's are: the runner is a child
//! of this process and is stopped when the session closes. A run a cockpit
//! started apart from itself and left running is taken up and followed
//! instead, and let go of again on close.

use crate::{
    Options,
    connection::{self, Catalog, DEFAULT_EFFORT, EFFORTS, Launch, Meta, Preferences, Setup},
    fold::Fold,
    rewind::{self, PromptGone},
    runner::{
        self, AdoptError, Detached, Features, LockError, Output, Process, SessionLock, Start, Tail,
        session_busy, session_dir, session_path,
    },
    text::{clean, format_time, headline},
    transcript::{Body, Event, Transcript},
};
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use kou_thread::{
    Backend, BackendEnd, Block, Capabilities, Choice, Command, Level, Offers, Request, Stop,
    Update,
    files::{FileLink, file_links, link_label},
};
use serde_json::json;
use std::{
    collections::VecDeque,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

/// What the agent is asked to pick up an interrupted run.
const CONTINUE_PROMPT: &str = "Continue where you left off.";
/// Why the session records a run of this app's was stopped.
const STOPPED_REASON: &str = "the user stopped the run from kou desktop";
const CLOSED_REASON: &str = "the user closed the session in kou desktop";
/// The image formats every provider takes.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_IMAGES: usize = 20;
const MAX_IMAGE_BYTES: usize = 20 << 20;
/// How often the files of a run this process does not own are read.
const FOLLOW_POLL: Duration = Duration::from_millis(100);
/// How long a detached runner that said how it ended may take to exit.
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// Opens a session and returns its channels at once; a thread does the rest.
pub(crate) fn connect(options: Options, setup: Setup) -> Result<Backend> {
    if !options.runner.is_file() {
        bail!(
            "the runner {} does not exist; build it with `make build`",
            options.runner.display()
        );
    }
    if !options.cwd.is_dir() {
        bail!(
            "the project folder {} does not exist",
            options.cwd.display()
        );
    }
    if let Some(id) = &options.session
        && !runner::valid_session_id(id)
    {
        bail!("invalid session ID {id:?}");
    }
    if let Some(model) = &options.model
        && !connection::valid_model(model)
    {
        bail!("a model ID has no spaces");
    }
    let (backend, end) = kou_thread::channel();
    std::thread::Builder::new()
        .name("kou-native-session".into())
        .spawn(move || Session::run(options, setup, end))?;
    Ok(backend)
}

enum Message {
    Command(Command),
    /// A request beyond the commands: a message for the running agent.
    Request(Request),
    /// The interface dropped its end.
    Gone,
    /// Something the runner of run `generation` said.
    Runner {
        generation: u64,
        output: Output,
    },
    /// The connection's models, listed in the background.
    Models(Catalog),
}

/// A prompt on its way to the runner.
struct Outgoing {
    message_id: String,
    text: String,
    images: Vec<serde_json::Value>,
    files: Vec<FileLink>,
    /// A compaction instead of a prompt, with what its summary should keep
    /// in view.
    compact: Option<String>,
}

/// What a run is started with once its session is held: the launch, the
/// session's lock and the model.
struct Claim {
    launch: Launch,
    lock: SessionLock,
    model: String,
}

enum Run {
    /// A runner this session started, attached.
    Own { process: Process, generation: u64 },
    /// A run a cockpit left running apart from itself, taken up here.
    Detached {
        run: Detached,
        output: Tail,
        errors: Tail,
        /// What the runner's last line said, and when it came.
        exit: Option<(i32, Instant)>,
    },
    /// A run of another window or terminal, followed in the session's file.
    External { file: Tail },
}

struct Session {
    runner: PathBuf,
    cwd: PathBuf,
    sessions: PathBuf,
    setup: Setup,
    features: Features,
    id: String,
    /// The runner has not created the session's file yet.
    fresh: bool,
    titled: bool,
    updates: async_channel::Sender<Update>,
    /// The interface is gone: nothing more can be told.
    deaf: bool,
    messages: mpsc::Sender<Message>,
    fold: Fold,
    catalog: Catalog,
    effort: String,
    run: Option<Run>,
    generation: u64,
    queue: VecDeque<Outgoing>,
}

impl Session {
    fn run(options: Options, setup: Setup, end: BackendEnd) {
        let (messages, inbox) = mpsc::channel();
        let commands = end.commands.clone();
        {
            let (messages, commands) = (messages.clone(), end.commands);
            std::thread::spawn(move || {
                while let Ok(command) = commands.recv_blocking() {
                    if messages.send(Message::Command(command)).is_err() {
                        return;
                    }
                }
                let _ = messages.send(Message::Gone);
            });
        }
        let requests = end.requests.clone();
        {
            // Requests come while a run goes on, long after the prompt that
            // started it: their own thread keeps their order among themselves.
            let (messages, requests) = (messages.clone(), end.requests);
            std::thread::spawn(move || {
                while let Ok(request) = requests.recv_blocking() {
                    if messages.send(Message::Request(request)).is_err() {
                        return;
                    }
                }
            });
        }
        {
            // Listing may take seconds; the conversation replays meanwhile.
            let (messages, setup, cwd) = (messages.clone(), setup.clone(), options.cwd.clone());
            std::thread::spawn(move || {
                let _ = messages.send(Message::Models(connection::list_models(&setup, &cwd)));
            });
        }
        let mut session = Session {
            features: runner::features_of(&options.runner),
            runner: options.runner,
            sessions: session_dir(&options.cwd),
            fold: Fold::new(&options.cwd, options.model),
            cwd: options.cwd,
            setup,
            id: options
                .session
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            fresh: true,
            titled: false,
            updates: end.updates,
            deaf: false,
            messages,
            catalog: Catalog::default(),
            effort: String::new(),
            run: None,
            generation: 0,
            queue: VecDeque::new(),
        };
        let reason = session
            .serve(&inbox)
            .err()
            .map(|error| format!("{error:#}"));
        session.send(Update::Closed(reason));
        // Nothing reads commands any more: the threads that wait for them go.
        commands.close();
        requests.close();
    }

    fn serve(&mut self, inbox: &mpsc::Receiver<Message>) -> Result<()> {
        self.open()?;
        // Commands that arrive before the models are listed wait their turn.
        let mut early = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Message::Models(catalog)) => {
                    self.catalog = catalog;
                    break;
                }
                Ok(Message::Gone) => return Ok(()),
                Ok(other) => early.push(other),
                Err(_) => {
                    self.catalog.error = Some("the connection did not answer in time".into());
                    break;
                }
            }
        }
        self.ready();
        for message in early {
            if !self.handle(message) {
                self.shut_down(inbox);
                return Ok(());
            }
        }
        loop {
            let following = matches!(self.run, Some(Run::Detached { .. } | Run::External { .. }));
            let message = if following {
                match inbox.recv_timeout(FOLLOW_POLL) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => Some(Message::Gone),
                }
            } else {
                Some(inbox.recv().unwrap_or(Message::Gone))
            };
            let alive = match message {
                Some(message) => self.handle(message),
                None => {
                    self.follow();
                    true
                }
            };
            if !alive || self.deaf {
                self.shut_down(inbox);
                return Ok(());
            }
        }
    }

    fn send(&mut self, update: Update) {
        if self.updates.send_blocking(update).is_err() {
            self.deaf = true;
        }
    }

    /// Runs `work` with a sink that sends updates to the interface.
    fn with_sink<T>(&mut self, work: impl FnOnce(&mut Self, &mut dyn FnMut(Update)) -> T) -> T {
        let updates = self.updates.clone();
        let mut deaf = false;
        let result = work(self, &mut |update| {
            deaf |= updates.send_blocking(update).is_err();
        });
        self.deaf |= deaf;
        result
    }

    fn notice(&mut self, level: Level, text: &str) {
        self.with_sink(|session, sink| session.fold.emitter.notice(level, text, sink));
    }

    /// Replays the stored conversation and takes up a run that goes on.
    fn open(&mut self) -> Result<()> {
        let path = session_path(&self.sessions, &self.id);
        let meta = Meta::load(&self.sessions, &self.id);
        let preferences = Preferences::load(&self.setup);
        self.effort = [
            meta.effort.as_str(),
            preferences.effort.as_str(),
            DEFAULT_EFFORT,
        ]
        .into_iter()
        .find(|effort| !effort.is_empty())
        .unwrap_or(DEFAULT_EFFORT)
        .to_owned();
        if !path.exists() {
            if self.fold.chosen.is_none() {
                // A new session starts with the model last chosen in a composer.
                self.fold.chosen = Some(preferences.last_model).filter(|model| !model.is_empty());
            }
            return Ok(());
        }
        self.fresh = false;
        let mut external = false;
        if session_busy(&self.sessions, &self.id) {
            match runner::adopt(&self.sessions, &self.id) {
                Ok(run) => {
                    // The run's output carries on from the session as the
                    // runner found it.
                    let prefix = std::fs::File::open(&path)?.take(run.info.session_size);
                    for line in BufReader::new(prefix).split(b'\n') {
                        self.fold.transcript.apply(&String::from_utf8_lossy(&line?));
                    }
                    let (mut output, errors) = (run.output(), run.errors());
                    let mut exit = None;
                    let transcript = &mut self.fold.transcript;
                    output.read(|line| {
                        for event in transcript.apply(line) {
                            if let Event::Exit(code) = event {
                                exit = Some((code, Instant::now()));
                            }
                        }
                    })?;
                    // Nobody watched what streamed so far: the recorded
                    // texts are all the thread gets of it.
                    self.fold.transcript.forget_streamed();
                    self.fold.join();
                    self.run = Some(Run::Detached {
                        run,
                        output,
                        errors,
                        exit,
                    });
                }
                Err(AdoptError::Followed | AdoptError::None) => external = true,
            }
        }
        if self.run.is_none() {
            let mut file = Tail::new(path, 0);
            let transcript = &mut self.fold.transcript;
            file.read(|line| {
                transcript.apply(line);
            })?;
            if external {
                self.run = Some(Run::External { file });
            }
        }
        let transcript = &mut self.fold.transcript;
        if transcript.skipped > 0 {
            let text = format!("{} session records could not be read", transcript.skipped);
            transcript.note(Level::Warning, text);
            transcript.skipped = 0;
        }
        let live = self.run.is_some();
        let interrupted = self.with_sink(|session, sink| {
            let fold = &mut session.fold;
            fold.emitter.replay(&mut fold.transcript, live, sink)
        });
        let title = meta
            .shown_title()
            .map(str::to_owned)
            .or_else(|| self.fold.transcript.title());
        if let Some(title) = title {
            self.titled = true;
            self.send(Update::Title(title));
        }
        if external {
            self.notice(
                Level::Warning,
                "This session is running in another window or terminal. Following it.",
            );
        } else if !live && interrupted {
            self.notice(
                Level::Warning,
                "The last run did not finish. Send /continue to pick it up.",
            );
        }
        Ok(())
    }

    /// Says the session can take a prompt, with what it offers.
    fn ready(&mut self) {
        self.fold.emitter.context_limit = self.catalog.context_window;
        let model = self.model_in_force();
        self.fold.announced.clone_from(&model);
        self.send(Update::Ready {
            session: self.id.clone(),
            capabilities: Capabilities {
                images: true,
                // The runner reads linked files itself, as the prompt runs.
                embedded_files: false,
                resume: true,
            },
            modes: EFFORTS
                .iter()
                .map(|level| Choice {
                    id: (*level).to_owned(),
                    name: format!("Effort: {level}"),
                    description: Some("How hard the model thinks before it answers".to_owned()),
                })
                .collect(),
            mode: Some(self.effort.clone()),
            models: self.catalog.models.clone(),
            model,
        });
        if !self.catalog.providers.is_empty() {
            self.send(Update::Providers(self.catalog.providers.clone()));
        }
        self.send(Update::Commands(vec![
            Choice {
                id: "compact".into(),
                name: "compact".into(),
                description: Some(
                    "Summarize the conversation to free context; the next prompt starts from the summary"
                        .into(),
                ),
            },
            Choice {
                id: "continue".into(),
                name: "continue".into(),
                description: Some("Pick up a run that was stopped or did not finish".into()),
            },
        ]));
        self.send(Update::Offers(Offers {
            listening: false,
            steer: self.features.steer,
            // An edited prompt rewinds the session, as the cockpits do.
            rewind: true,
            arguments: vec![("compact".into(), "[focus]".into())],
        }));
        if !self.fresh {
            self.with_sink(|session, sink| {
                let fold = &mut session.fold;
                fold.emitter.tell_usage(&fold.transcript, sink);
            });
        }
        if let Some(error) = self.catalog.error.clone() {
            self.notice(
                Level::Warning,
                &format!("Cannot list the connection's models: {error}"),
            );
        }
    }

    /// The model the next prompt runs with: the one chosen here, else the one
    /// the session's last prompt ran with, else the connection's.
    fn model_in_force(&self) -> Option<String> {
        self.fold
            .chosen
            .clone()
            .or_else(|| self.fold.transcript.last_model.clone())
            .or_else(|| self.catalog.named.clone())
            .or_else(|| self.catalog.first().map(str::to_owned))
            .or_else(|| self.catalog.fallback.clone())
    }

    /// Handles one message; `false` ends the session.
    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Gone | Message::Command(Command::Close) => return false,
            Message::Models(_) => {}
            Message::Command(Command::Prompt(blocks)) => self.prompt(&blocks),
            Message::Request(Request::Steer(blocks)) => self.steer(&blocks),
            Message::Request(Request::Rewind { prompt, blocks }) => {
                if let Err(error) = self.rewind(prompt.as_deref(), &blocks) {
                    self.send(Update::Rewound(Err(format!("{error:#}"))));
                }
            }
            Message::Command(Command::Cancel) => self.cancel(STOPPED_REASON),
            Message::Command(Command::SetModel(model)) => self.set_model(&model),
            Message::Command(Command::SetMode(mode)) => self.set_effort(&mode),
            // The runner asks no permission: there is nothing to answer.
            // (Its efforts are offered as its modes.)
            Message::Command(
                Command::Permit { .. }
                | Command::Answer { .. }
                | Command::SetEffort(_)
                | Command::SetAccess(_),
            ) => {}
            Message::Runner { generation, output } => {
                let current = matches!(&self.run, Some(Run::Own { generation: own, .. }) if *own == generation);
                if current {
                    match output {
                        Output::Line { text, stderr } => {
                            self.line(&text, stderr);
                        }
                        Output::Exited(code) => self.finish(code),
                    }
                }
            }
        }
        true
    }

    fn prompt(&mut self, blocks: &[Block]) {
        let outgoing = match self.compose(blocks) {
            Ok(Some(outgoing)) => outgoing,
            Ok(None) => {
                self.send(Update::PromptTaken(None));
                return;
            }
            Err(error) => {
                self.send(Update::PromptTaken(None));
                self.notice(Level::Warning, &format!("{error:#}"));
                return;
            }
        };
        // The interface knows it by its ID from now on; a compaction is no prompt.
        if outgoing.compact.is_none() {
            self.fold.emitter.taken(&outgoing.message_id);
            self.send(Update::PromptTaken(Some(outgoing.message_id.clone())));
        } else {
            self.send(Update::PromptTaken(None));
        }
        self.prompt_outgoing(outgoing);
    }

    /// Runs a prompt, or queues it behind the run that goes on.
    fn prompt_outgoing(&mut self, outgoing: Outgoing) {
        let external = matches!(self.run, Some(Run::External { .. }));
        if external {
            self.notice(
                Level::Warning,
                "This session is running in another window or terminal. The message was not sent.",
            );
        } else if self.run.is_none() {
            self.start(outgoing);
        } else if outgoing.compact.is_some() {
            self.notice(
                Level::Warning,
                "A run is in progress. Compact once it has finished.",
            );
        } else {
            // While the agent works, a prompt waits for the run to end.
            self.fold.emitter.sent(&outgoing.message_id);
            self.queue.push_back(outgoing);
        }
    }

    /// Hands a message to the running agent, which reads it after the tool
    /// calls it is making. It is not marked as sent from here: the runner
    /// records it when the agent takes it, and the thread shows it then, in
    /// its place. Without a run that takes messages it waits as a prompt.
    fn steer(&mut self, blocks: &[Block]) {
        let outgoing = match self.compose(blocks) {
            Ok(Some(outgoing)) => outgoing,
            Ok(None) => return,
            Err(error) => {
                self.notice(Level::Warning, &format!("{error:#}"));
                return;
            }
        };
        if outgoing.compact.is_some() {
            self.notice(
                Level::Warning,
                "A compaction cannot be forced into a run. Send it once the run has finished.",
            );
            return;
        }
        let mut line = serde_json::Map::new();
        line.insert("content".into(), json!(outgoing.text));
        line.insert("message_id".into(), json!(outgoing.message_id));
        if !outgoing.images.is_empty() {
            line.insert("images".into(), json!(outgoing.images));
        }
        if !outgoing.files.is_empty() {
            line.insert("files".into(), json!(outgoing.files));
        }
        let line = serde_json::Value::Object(line).to_string();
        let sent = match &mut self.run {
            Some(Run::Own { process, .. }) if self.features.steer => process.send(line.as_bytes()),
            Some(Run::Detached { run, .. }) => run.steer(&line).is_ok(),
            _ => false,
        };
        if sent {
            return;
        }
        let running = self.run.is_some();
        if running {
            self.notice(
                Level::Info,
                "The running agent takes no messages: this one runs when it finishes.",
            );
        }
        self.prompt_outgoing(outgoing);
    }

    /// Turns the blocks of a prompt into what the runner takes: text that
    /// refers to its images by label and to its files by `$` reference.
    fn compose(&mut self, blocks: &[Block]) -> Result<Option<Outgoing>> {
        let (trimmed, images) = self.assemble(blocks)?;
        if clean(&trimmed).trim().is_empty() {
            self.notice(Level::Warning, "The prompt has no printable text.");
            return Ok(None);
        }
        let mut compact = None;
        let mut text = trimmed.clone();
        if let Some(command) = trimmed.strip_prefix('/') {
            let (name, argument) = match command.split_once(char::is_whitespace) {
                Some((name, argument)) => (name, argument.trim()),
                None => (command, ""),
            };
            match name {
                "compact" => {
                    if self.fresh {
                        self.notice(
                            Level::Info,
                            "Nothing to compact yet: the session starts with its first prompt.",
                        );
                        return Ok(None);
                    }
                    if !self.fold.transcript.compactable && self.run.is_none() {
                        self.notice(
                            Level::Info,
                            "Nothing to compact: the agent has not answered since the last compaction.",
                        );
                        return Ok(None);
                    }
                    compact = Some(argument.to_owned());
                }
                "continue" => {
                    if self.run.is_none() && !self.fold.transcript.interrupted() {
                        self.notice(
                            Level::Info,
                            "The last run finished. There is nothing to continue.",
                        );
                        return Ok(None);
                    }
                    CONTINUE_PROMPT.clone_into(&mut text);
                }
                "model" | "models" if !argument.is_empty() => {
                    self.set_model(argument);
                    return Ok(None);
                }
                "effort" | "think" if !argument.is_empty() => {
                    self.set_effort(argument);
                    return Ok(None);
                }
                // Anything else is the user's text: a path may start a prompt.
                _ => {}
            }
        }
        Ok(Some(self.outgoing(text, images, compact)))
    }

    /// A prompt of `text` and `images` on its way, with the files it links.
    fn outgoing(
        &self,
        text: String,
        images: Vec<serde_json::Value>,
        compact: Option<String>,
    ) -> Outgoing {
        let files = if self.features.links && compact.is_none() {
            file_links(&text, &self.cwd)
        } else {
            Vec::new()
        };
        Outgoing {
            message_id: uuid::Uuid::new_v4().to_string(),
            text,
            images,
            files,
            compact,
        }
    }

    /// The text of a prompt's blocks, trimmed, with a label where each image
    /// is, and the images.
    fn assemble(&self, blocks: &[Block]) -> Result<(String, Vec<serde_json::Value>)> {
        let typed: String = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut next_image = next_image_number(&typed);
        let mut text = String::new();
        let mut images = Vec::new();
        // A label sits in the line it was put in; two pieces of text are
        // lines of their own.
        let join = |text: &mut String, piece: &str, separator: char| {
            // No space before what closes a clause: `[Image 1], and`.
            if !text.is_empty()
                && !text.ends_with(char::is_whitespace)
                && !piece
                    .starts_with(|ch: char| ch.is_whitespace() || ".,;:!?)]}»…'\"".contains(ch))
            {
                text.push(separator);
            }
            text.push_str(piece);
        };
        let mut after_label = false;
        for block in blocks {
            match block {
                Block::Text(piece) => {
                    join(&mut text, piece, if after_label { ' ' } else { '\n' });
                }
                Block::Image { mime, data } => {
                    let mime = match mime.trim().to_ascii_lowercase().as_str() {
                        "image/jpg" => "image/jpeg".to_owned(),
                        other => other.to_owned(),
                    };
                    if !IMAGE_TYPES.contains(&mime.as_str()) {
                        bail!(
                            "Models take PNG, JPEG, GIF and WebP images, not {mime}. The prompt was not sent."
                        );
                    }
                    if data.len() > MAX_IMAGE_BYTES {
                        bail!("An image is larger than 20 MB. The prompt was not sent.");
                    }
                    if images.len() == MAX_IMAGES {
                        bail!(
                            "A prompt takes at most {MAX_IMAGES} images. The prompt was not sent."
                        );
                    }
                    let label = format!("[Image {next_image}]");
                    next_image += 1;
                    join(&mut text, &label, ' ');
                    images.push(json!({
                        "label": label,
                        "media_type": mime,
                        "data": base64::engine::general_purpose::STANDARD.encode(data),
                    }));
                }
                Block::File { path, .. } => {
                    let name = path
                        .strip_prefix(&self.cwd)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned();
                    join(&mut text, &link_label(&name), ' ');
                }
            }
            after_label = !matches!(block, Block::Text(_));
        }
        Ok((text.trim().to_owned(), images))
    }

    /// Starts a run for a prompt, or says why it could not start.
    fn start(&mut self, outgoing: Outgoing) {
        self.start_claimed(outgoing, None);
    }

    /// Starts a run for a prompt with the session held already, or holds it
    /// first.
    fn start_claimed(&mut self, outgoing: Outgoing, claim: Option<Claim>) {
        self.fold.emitter.sent(&outgoing.message_id);
        let compaction = outgoing.compact.is_some();
        let title = headline(&outgoing.text, 80);
        self.with_sink(|session, sink| session.fold.begin(compaction, sink));
        let launched = match claim {
            Some(claim) => self.spawn(&outgoing, claim),
            None => self.launch(&outgoing),
        };
        match launched {
            Ok(process) => {
                self.run = Some(Run::Own {
                    process,
                    generation: self.generation,
                });
                if !self.titled && !compaction && !title.is_empty() {
                    self.titled = true;
                    self.send(Update::Title(title));
                }
            }
            Err(error) => {
                // The run never began: the turn ends with why.
                let line = json!({"type": "error", "message": format!("{error:#}")}).to_string();
                self.with_sink(|session, sink| {
                    session.fold.line(&line, false, sink);
                    session.fold.finish(Some(1), sink);
                });
                self.drop_queue();
            }
        }
    }

    /// Edits a prompt already sent: the session goes back to before it, as
    /// the cockpits rewind one, and the edited prompt runs from there.
    /// `prompt` names the first prompt the interface let go of, `None` when
    /// none of them reached the session. Says how it went with
    /// [`Update::Rewound`]: an error returned changed nothing.
    fn rewind(&mut self, prompt: Option<&str>, blocks: &[Block]) -> Result<()> {
        match &self.run {
            Some(Run::External { .. }) => {
                bail!("This session is running in another window or terminal.")
            }
            Some(_) => bail!("The agent is working: stop it or let it finish, then edit."),
            None => {}
        }
        let (text, images) = self.assemble(blocks)?;
        if clean(&text).trim().is_empty() {
            bail!("The edited prompt has no printable text.");
        }
        let outgoing = self.outgoing(text, images, None);
        // The session goes back before the first of those prompts it holds:
        // one that never reached it exists only in the interface.
        let target = prompt.and_then(|prompt| self.held_from(prompt));
        let claim = self.claim(false)?;
        match &target {
            Some(target) => {
                rewind::rewind_session(&self.sessions, &self.id, target, &outgoing.message_id)
                    .map_err(|error| match error.downcast_ref::<PromptGone>() {
                        Some(_) => anyhow!(
                            "The session no longer holds that prompt: it was changed elsewhere."
                        ),
                        None => error,
                    })?;
                self.reload(prompt);
            }
            None => self.fold.emitter.let_go(prompt),
        }
        self.fold.emitter.taken(&outgoing.message_id);
        self.send(Update::Rewound(Ok(outgoing.message_id.clone())));
        self.start_claimed(outgoing, Some(claim));
        Ok(())
    }

    /// The first prompt from `prompt` on, in the order the interface was told
    /// of them, that the session holds; `prompt` itself when the interface
    /// was not told of it here.
    fn held_from(&self, prompt: &str) -> Option<String> {
        let held =
            |id: &str| {
                self.fold.transcript.entries.iter().any(
                    |entry| matches!(&entry.body, Body::User(message) if message.message_id == id),
                )
            };
        let named = &self.fold.emitter.named;
        match named.iter().position(|id| id == prompt) {
            Some(at) => named[at..].iter().find(|id| held(id)).cloned(),
            None => Some(prompt.to_owned()),
        }
    }

    /// Reads the session again after a rewind: what remains is what the
    /// interface keeps of the conversation, and the totals, the model and
    /// the title are those of what remains.
    fn reload(&mut self, prompt: Option<&str>) {
        let mut transcript = Transcript::new();
        let mut file = Tail::new(session_path(&self.sessions, &self.id), 0);
        if let Err(error) = file.read(|line| {
            transcript.apply(line);
        }) {
            log::warn!("the rewound session could not be read: {error}");
        }
        // What a removed run left open is no more.
        transcript.settle_abandoned(true);
        transcript.skipped = 0;
        self.fold.emitter.rewound(&mut transcript, prompt);
        self.fold.transcript = transcript;
        self.fresh = !session_path(&self.sessions, &self.id).exists();
        // A title that was the first prompt follows the edited one.
        self.titled = Meta::load(&self.sessions, &self.id).shown_title().is_some()
            || self.fold.transcript.title().is_some();
        self.with_sink(|session, sink| {
            let fold = &mut session.fold;
            fold.emitter.tell_usage(&fold.transcript, sink);
        });
        let model = self.model_in_force();
        if model.is_some() && model != self.fold.announced {
            self.fold.announced.clone_from(&model);
            self.send(Update::Model(model.unwrap_or_default()));
        }
    }

    fn launch(&mut self, outgoing: &Outgoing) -> Result<Process> {
        let claim = self.claim(outgoing.compact.is_some())?;
        self.spawn(outgoing, claim)
    }

    /// Holds the session for a run: its launch is made, the runner can run
    /// it, and no other front-end runs it.
    fn claim(&mut self, compaction: bool) -> Result<Claim> {
        let model = self.model_in_force().unwrap_or_default();
        let launch = connection::launch(&self.setup, &self.cwd, &model)?;
        runner::check_provider(&self.runner, &launch.provider)?;
        let lock = match runner::lock_session(&self.sessions, &self.id) {
            Ok(lock) => lock,
            Err(LockError::Busy) => {
                bail!("This session is running in another window or terminal.")
            }
            Err(LockError::Failed(error)) => return Err(error),
        };
        // Checked under the lock, which deleting a session holds too.
        let exists = session_path(&self.sessions, &self.id).exists();
        if !exists && (!self.fresh || compaction) {
            bail!("This session was deleted elsewhere.");
        }
        Ok(Claim {
            launch,
            lock,
            model,
        })
    }

    /// Starts the runner for a prompt in the session `claim` holds.
    fn spawn(&mut self, outgoing: &Outgoing, claim: Claim) -> Result<Process> {
        let Claim {
            launch,
            lock,
            model,
        } = claim;
        if outgoing.compact.is_none() {
            self.snapshot_memory(&outgoing.message_id);
        }
        let mut request = serde_json::Map::new();
        match &outgoing.compact {
            Some(focus) => {
                request.insert("compact".into(), json!(true));
                if !focus.is_empty() {
                    request.insert("compact_instructions".into(), json!(focus));
                }
            }
            None => {
                let mut message = serde_json::Map::new();
                message.insert("content".into(), json!(outgoing.text));
                message.insert("message_id".into(), json!(outgoing.message_id));
                if !outgoing.images.is_empty() {
                    message.insert("images".into(), json!(outgoing.images));
                }
                if !outgoing.files.is_empty() {
                    message.insert("files".into(), json!(outgoing.files));
                }
                request.insert("messages".into(), json!([message]));
            }
        }
        request.insert("session_id".into(), json!(self.id));
        if !model.is_empty() {
            request.insert("model".into(), json!(model));
        }
        request.insert("thinking_level".into(), json!(self.effort));
        let request = serde_json::to_vec(&request)?;

        let mut args = vec![
            "-workspace".to_owned(),
            self.cwd.to_string_lossy().into_owned(),
            "-session-directory".to_owned(),
            self.sessions.to_string_lossy().into_owned(),
            "-tool-heartbeat-interval".to_owned(),
            "10m0s".to_owned(),
        ];
        // A compaction takes no messages: what is sent during it runs after it.
        let steers = self.features.steer && outgoing.compact.is_none();
        if steers {
            args.push("-steer".to_owned());
        }
        if self.features.stream {
            args.push("-stream".to_owned());
        }
        self.generation += 1;
        let (generation, messages) = (self.generation, self.messages.clone());
        let process = Process::start(
            Start {
                runner: &self.runner,
                workspace: &self.cwd,
                args,
                env: launch.env,
                request: &request,
                keep_input: steers,
                stops: steers && self.features.stop,
                lock,
                lease: launch.lease,
            },
            move |output| {
                let _ = messages.send(Message::Runner { generation, output });
            },
        )?;
        if outgoing.compact.is_none()
            && let Err(error) = Meta::save_effort(&self.sessions, &self.id, &self.effort)
        {
            log::warn!("the session's effort was not kept: {error:#}");
        }
        Ok(process)
    }

    /// Records the notes and the length of the summaries a prompt's run
    /// starts from, so that a cockpit that rewinds to the prompt can restore
    /// them. Best effort, as in the cockpits.
    fn snapshot_memory(&self, message_id: &str) {
        let file = |suffix: &str| self.sessions.join(format!("{}{suffix}", self.id));
        let notes = std::fs::read_to_string(file(".notes.md")).unwrap_or_default();
        let summaries =
            std::fs::metadata(file(".summaries.md")).map_or(0, |metadata| metadata.len());
        let mut record = serde_json::Map::new();
        record.insert("message".into(), json!(message_id));
        if !notes.is_empty() {
            record.insert("notes".into(), json!(notes));
        }
        if summaries > 0 {
            record.insert("summaries".into(), json!(summaries));
        }
        record.insert("at".into(), json!(format_time(SystemTime::now())));
        let written = connection::create_private_dir(&self.sessions).and_then(|()| {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut memory = options.open(file(".memory.jsonl"))?;
            memory.write_all(format!("{}\n", serde_json::Value::Object(record)).as_bytes())?;
            Ok(())
        });
        if let Err(error) = written {
            log::debug!("the session's memory was not recorded: {error:#}");
        }
    }

    /// Folds one line of a run's output in and tells the interface.
    fn line(&mut self, text: &str, stderr: bool) -> Option<i32> {
        self.with_sink(|session, sink| session.fold.line(text, stderr, sink))
    }

    /// Ends the run: closes its turn, and goes on with what was queued
    /// meanwhile if it ended well.
    fn finish(&mut self, code: Option<i32>) {
        self.run = None;
        let stop = self.with_sink(|session, sink| session.fold.finish(code, sink));
        self.fresh = !session_path(&self.sessions, &self.id).exists();
        if !matches!(stop, Stop::Finished | Stop::Limit) {
            // A run that is stopped or fails holds what waited for it.
            self.drop_queue();
        } else if let Some(next) = self.queue.pop_front() {
            self.start(next);
        }
    }

    /// Tells the user of the prompts that waited for a run that did not end
    /// well: they are not sent on their own.
    fn drop_queue(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let first = headline(&self.queue[0].text, 60);
        let text = match self.queue.len() {
            1 => format!("The queued message was not sent: \"{first}\". Send it again to run it."),
            count => format!(
                "{count} queued messages were not sent, the first: \"{first}\". Send them again to run them."
            ),
        };
        self.queue.clear();
        self.notice(Level::Warning, &text);
    }

    fn cancel(&mut self, reason: &str) {
        let result = match &mut self.run {
            Some(Run::Own { process, .. }) => {
                if self.fold.cancel() {
                    process.stop(reason);
                }
                Ok(())
            }
            Some(Run::Detached { run, .. }) => {
                if self.fold.cancel() {
                    run.stop(reason, &self.sessions)
                } else {
                    Ok(())
                }
            }
            Some(Run::External { .. }) => Err(anyhow!(
                "This run belongs to another window or terminal. Stop it there."
            )),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.notice(Level::Warning, &format!("{error:#}"));
        }
    }

    fn set_model(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty() || !connection::valid_model(model) {
            self.notice(Level::Warning, "A model ID has no spaces.");
            return;
        }
        self.fold.chosen = Some(model.to_owned());
        self.fold.announced = Some(model.to_owned());
        self.send(Update::Model(model.to_owned()));
        // New sessions of either cockpit start with the model last chosen.
        if self.setup.settings_file.is_some()
            && let Err(error) = Preferences::save(&self.setup, "last_model", model)
        {
            self.notice(
                Level::Warning,
                &format!("The model is chosen for this session only: {error:#}"),
            );
        }
        let listed = self.catalog.models.iter().any(|known| known.id == model);
        if !self.catalog.models.is_empty() && !listed {
            self.notice(
                Level::Warning,
                &format!("The connection does not list {model}. Runs try it anyway."),
            );
        }
        if matches!(self.run, Some(Run::Own { .. } | Run::Detached { .. })) {
            self.notice(
                Level::Info,
                &format!("The running agent keeps its model. {model} answers the next prompt."),
            );
        }
    }

    fn set_effort(&mut self, level: &str) {
        let level = level.trim();
        if !EFFORTS.contains(&level) {
            self.notice(
                Level::Warning,
                &format!("Effort levels: {}.", EFFORTS.join(", ")),
            );
            return;
        }
        level.clone_into(&mut self.effort);
        self.send(Update::Mode(level.to_owned()));
        if self.setup.settings_file.is_some()
            && let Err(error) = Preferences::save(&self.setup, "effort", level)
        {
            log::warn!("the effort was not kept for new sessions: {error:#}");
        }
    }

    /// Reads what a run this process does not own wrote since the last look.
    fn follow(&mut self) {
        let mut lines: Vec<(String, bool)> = Vec::new();
        let mut ended = false;
        match &mut self.run {
            Some(Run::Detached {
                run,
                output,
                errors,
                exit,
            }) => {
                // Whether the runner is gone is asked before the read, so
                // that what it wrote last is read.
                ended = run.gone(&self.sessions)
                    || exit.is_some_and(|(_, seen)| seen.elapsed() > EXIT_WAIT);
                let read = output
                    .read(|line| lines.push((line.to_owned(), false)))
                    .and_then(|()| errors.read(|line| lines.push((line.to_owned(), true))));
                if let Err(error) = read {
                    log::warn!("the run's output could not be read: {error}");
                }
            }
            Some(Run::External { file }) => {
                ended = !session_busy(&self.sessions, &self.id);
                if let Err(error) = file.read(|line| lines.push((line.to_owned(), false))) {
                    log::warn!("the session could not be read: {error}");
                }
            }
            _ => {}
        }
        for (line, stderr) in lines {
            if let Some(code) = self.line(&line, stderr)
                && let Some(Run::Detached { exit, .. }) = &mut self.run
            {
                exit.get_or_insert((code, Instant::now()));
            }
        }
        if !ended {
            return;
        }
        match self.run.take() {
            Some(Run::Detached { exit, .. }) => self.finish(exit.map(|(code, _)| code)),
            Some(Run::External { .. }) => {
                // The other window's run is over: its session says how it went.
                let interrupted = self.with_sink(|session, sink| session.fold.finish_other(sink));
                if interrupted {
                    self.notice(
                        Level::Warning,
                        "The other run did not finish. Send /continue to pick it up.",
                    );
                } else {
                    self.notice(Level::Info, "The other run has finished.");
                }
            }
            other => self.run = other,
        }
    }

    /// Closes the session as the terminal cockpit does: a run of its own is
    /// stopped and waited for; a run of another front-end goes on.
    fn shut_down(&mut self, inbox: &mpsc::Receiver<Message>) {
        self.queue.clear();
        if !matches!(self.run, Some(Run::Own { .. })) {
            self.run = None;
            return;
        }
        self.cancel(CLOSED_REASON);
        let deadline = Instant::now() + runner::STOP_TIMEOUT + Duration::from_secs(5);
        while matches!(self.run, Some(Run::Own { .. })) {
            match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(message @ Message::Runner { .. }) => {
                    self.handle(message);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if let Some(Run::Own { process, .. }) = &self.run
            && !process.exited()
        {
            log::warn!("the runner outlasted its stop and was left to the kill");
        }
        self.run = None;
    }
}

/// The number of the next image a text takes: one past any label it uses.
fn next_image_number(text: &str) -> usize {
    let mut next = 1;
    let mut rest = text;
    while let Some(at) = rest.find("[Image ") {
        rest = &rest[at + "[Image ".len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=6).contains(&digits)
            && rest[digits..].starts_with(']')
            && let Ok(number) = rest[..digits].parse::<usize>()
        {
            next = next.max(number + 1);
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_labels_number_on_from_the_ones_the_text_has() {
        assert_eq!(next_image_number("no images"), 1);
        assert_eq!(
            next_image_number("see [Image 2] and [Image 12], not [Image x]"),
            13
        );
    }
}
