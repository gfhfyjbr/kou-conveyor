//! One `claude` process: the command line and environment it is started
//! with, and the threads that carry its lines in and out. It is a child of
//! the app's, or the program of a host that keeps it apart from the app
//! (see `kept`); the session speaks to either alike.
//!
//! A session may run more than one in its life (an edit of a sent prompt
//! starts Claude Code anew), so everything a process says reaches the session
//! marked with the number of its link, and what an old one still says after
//! it was let go is known for what it is.

use super::{
    kept::{self, Kept, Over},
    process::{self, Exit, Process},
};
use crate::{Options, host::Attached};
use anyhow::Context as _;
use kou_thread::{Command, Request};
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

/// Something that happened to the session.
pub(crate) enum Event {
    /// The process of this link wrote a message.
    Message(u64, Value),
    /// The process of this link closed its standard output.
    StdoutClosed(u64),
    /// The process of this link closed its standard error.
    StderrClosed(u64),
    /// The process of this link ended.
    Exited(u64, Exit),
    /// The host that kept the process of this link went away without
    /// saying how the process ended.
    KeeperGone(u64),
    /// Another client took the place of this link's at its host: the
    /// process runs on, out of this session's reach.
    Replaced(u64),
    /// The interface asks for something.
    Command(Command),
    /// The interface asks for something beyond the commands.
    Request(Request),
    /// The interface dropped its sending end.
    CommandsClosed,
}

/// How many of the last lines of standard error are kept, for the reason
/// given when the process dies.
const LAST_WORDS: usize = 10;
/// The longest such line, in characters.
const LAST_WORD_LENGTH: usize = 400;

/// The variables Claude Code sets for what it starts, which would make it
/// take itself for a child of another Claude Code (an app started from a
/// terminal Claude Code runs in has them).
const NOT_PASSED_ON: [&str; 3] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
];

/// Asks Claude Code for `session_state_changed`, which says when it is idle:
/// the end of a turn here.
const READS_SESSION_STATE: &str = "CLAUDE_CODE_SDK_READS_SESSION_STATE";

/// Where a process starts in the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resume {
    /// A new conversation.
    New,
    /// The conversation of this session id, whole.
    Session(String),
    /// A copy of the conversation of `session` that ends with the entry
    /// `message`, under a new session id: where an edit of the prompt after
    /// that entry runs.
    Fork { session: String, message: String },
}

/// The arguments `claude` is started with. `model` is the one to start with,
/// if any; `lean` leaves out what only a conversation needs (MCP servers,
/// added instructions, plugins), for a process only asked its models.
pub(crate) fn arguments(
    options: &Options,
    resume: &Resume,
    model: Option<&str>,
    lean: bool,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        "--replay-user-messages",
        "--permission-prompt-tool",
        "stdio",
        // Without it Claude Code refuses to be switched to
        // `bypassPermissions`, one of the modes offered (the SDK's
        // `allowDangerouslySkipPermissions`). It switches nothing by itself.
        "--allow-dangerously-skip-permissions",
    ]
    .map(str::to_owned)
    .to_vec();
    // The equals form, so that no id can be read as a flag of its own.
    match resume {
        Resume::New => {}
        Resume::Session(session) => args.push(format!("--resume={session}")),
        Resume::Fork { session, message } => {
            args.push(format!("--resume={session}"));
            args.push(format!("--resume-session-at={message}"));
            args.push("--fork-session".into());
        }
    }
    if let Some(model) = model {
        args.extend(["--model".into(), model.to_owned()]);
    }
    if lean {
        return args;
    }
    if !options.mcp_servers.is_empty() {
        let servers: Map<String, Value> = options
            .mcp_servers
            .iter()
            .map(|server| {
                let env: Map<String, Value> = server
                    .env
                    .iter()
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect();
                let config = json!({
                    "command": server.command,
                    "args": server.args,
                    "env": env,
                });
                (server.name.clone(), config)
            })
            .collect();
        args.push("--mcp-config".into());
        args.push(json!({ "mcpServers": servers }).to_string());
    }
    if let Some(text) = options
        .instructions
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        args.extend(["--append-system-prompt".into(), text.to_owned()]);
    }
    for plugin in &options.plugins {
        args.push("--plugin-dir".into());
        args.push(plugin.to_string_lossy().into_owned());
    }
    args
}

/// The environment `claude` is started with: `inherited` (the app's own)
/// without the marks of a Claude Code session the app may run in, with
/// [`added_env`] over it.
pub(crate) fn environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    options: &Options,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = inherited
        .into_iter()
        .filter(|(name, _)| !NOT_PASSED_ON.iter().any(|left| name == left))
        .collect();
    for (name, value) in added_env(options) {
        match env.iter_mut().find(|(known, _)| *known == *name) {
            Some(known) => known.1 = value.into(),
            None => env.push((name.into(), value.into())),
        }
    }
    env
}

/// What is added to the app's environment for `claude`: [`Options::env`]
/// and the request for session states. (A host is given this, and leaves
/// out the same marks of Claude Code's as [`environment`] does.)
pub(crate) fn added_env(options: &Options) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = options
        .env
        .iter()
        .filter(|(name, _)| name != READS_SESSION_STATE)
        .cloned()
        .collect();
    env.push((READS_SESSION_STATE.into(), "1".into()));
    env
}

/// The session's folder must be there, and be said in full.
pub(crate) fn check_folder(options: &Options) -> anyhow::Result<()> {
    anyhow::ensure!(
        options.cwd.is_absolute(),
        "the session's folder must be an absolute path, got {}",
        options.cwd.display()
    );
    anyhow::ensure!(
        options.cwd.is_dir(),
        "the session's folder {} does not exist",
        options.cwd.display()
    );
    Ok(())
}

/// The program's name, for sentences.
pub(crate) fn program_name(options: &Options) -> String {
    options
        .program
        .file_name()
        .unwrap_or(options.program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// What runs a link's `claude`.
#[derive(Clone)]
pub(crate) enum Runner {
    /// A child of the app's, in a process group of its own.
    Child(Arc<Process>),
    /// A host's program, reached through the host's client.
    Kept(Arc<Kept>),
}

impl Runner {
    /// End it at once, and what it started.
    pub fn kill(&self) {
        match self {
            Runner::Child(process) => process.kill(),
            Runner::Kept(kept) => kept.stop(),
        }
    }
}

/// How long a host that is told to stop is waited for: it gives its
/// program two seconds after `SIGTERM`, then kills it.
const KEPT_PATIENCE: Duration = Duration::from_secs(5);

/// A running `claude` and the ends of its pipes.
pub(crate) struct Link {
    /// The number its events carry.
    pub id: u64,
    runner: Runner,
    /// Lines for its standard input; `None` once that is closed.
    outbox: Option<mpsc::Sender<String>>,
    /// The last lines it wrote to standard error.
    pub stderr: Arc<Mutex<VecDeque<String>>>,
    pub stdout_closed: bool,
    pub stderr_closed: bool,
    pub exit: Option<Exit>,
    /// The host that kept it went away without saying how it ended.
    pub vanished: bool,
    /// Another client took this one's place at its host.
    pub replaced: bool,
    /// When it was first seen to be going away on its own.
    pub ending: Option<Instant>,
}

impl Link {
    /// Start `claude` with `args` in the session's folder, its events going
    /// to `events` under number `id`.
    pub fn start(
        id: u64,
        options: &Options,
        args: &[String],
        events: &mpsc::Sender<Event>,
    ) -> anyhow::Result<Self> {
        check_folder(options)?;
        let mut command = std::process::Command::new(&options.program);
        command
            .args(args)
            .env_clear()
            .envs(environment(std::env::vars_os(), options))
            .current_dir(&options.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process::own_group(&mut command);
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start {}", options.program.display()))?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let exited = events.clone();
        let process = Process::watch(child, "kou-claude exit", move |exit| {
            let _ = exited.send(Event::Exited(id, exit));
        })
        .context("could not watch the claude process")?;
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            process.kill();
            anyhow::bail!("the claude process has no pipes");
        };
        let name = program_name(options);
        let (outbox, lines) = mpsc::channel::<String>();
        let last_words = Arc::new(Mutex::new(VecDeque::new()));
        let started = (|| -> anyhow::Result<()> {
            spawn("kou-claude input", move || write_lines(stdin, lines))?;
            let (output, label) = (events.clone(), name.clone());
            spawn("kou-claude output", move || {
                read_messages(stdout, id, &label, &output)
            })?;
            let (errors, kept) = (events.clone(), last_words.clone());
            spawn("kou-claude errors", move || {
                read_errors(stderr, id, &name, &kept, &errors)
            })?;
            Ok(())
        })();
        if let Err(error) = started {
            // Half a link is none: the process does not stay behind.
            process.kill();
            return Err(error);
        }
        Ok(Self {
            id,
            runner: Runner::Child(process),
            outbox: Some(outbox),
            stderr: last_words,
            stdout_closed: false,
            stderr_closed: false,
            exit: None,
            vanished: false,
            replaced: false,
            ending: None,
        })
    }

    /// The link to the program of the host at `socket`, attached to, its
    /// events going to `events` under number `id`.
    pub fn kept(
        id: u64,
        attached: Attached,
        events: &mpsc::Sender<Event>,
        name: &str,
        socket: PathBuf,
    ) -> anyhow::Result<Self> {
        let Attached {
            client, received, ..
        } = attached;
        let over = Arc::new(Over::default());
        let kept = Arc::new(Kept::new(client, over.clone(), socket));
        let (outbox, lines) = mpsc::channel::<String>();
        let last_words = Arc::new(Mutex::new(VecDeque::new()));
        let writer = kept.clone();
        spawn("kou-claude input", move || {
            for line in lines {
                if let Err(error) = writer.write(&line) {
                    log::debug!("the keeper stopped reading: {error}");
                    return;
                }
            }
        })?;
        let (heard, kept_words, label) = (events.clone(), last_words.clone(), name.to_owned());
        spawn("kou-claude keeper", move || {
            kept::hear(id, received, over, kept_words, heard, label)
        })?;
        Ok(Self {
            id,
            runner: Runner::Kept(kept),
            outbox: Some(outbox),
            stderr: last_words,
            stdout_closed: false,
            stderr_closed: false,
            exit: None,
            vanished: false,
            replaced: false,
            ending: None,
        })
    }

    /// What runs it, to end it from elsewhere.
    pub fn runner(&self) -> Runner {
        self.runner.clone()
    }

    /// It is a host's program, which outlives the app.
    pub fn is_kept(&self) -> bool {
        matches!(self.runner, Runner::Kept(_))
    }

    /// Where the host of a kept one listens.
    pub fn keeper_socket(&self) -> Option<&Path> {
        match &self.runner {
            Runner::Kept(kept) => Some(kept.socket()),
            Runner::Child(_) => None,
        }
    }

    /// Stop it: a child is terminated, and killed after `grace`; a host is
    /// told to stop, which it does by itself.
    pub fn stop(&self, grace: Duration) {
        match &self.runner {
            Runner::Child(process) => {
                process.stop(grace);
            }
            Runner::Kept(kept) => kept.stop(),
        }
    }

    /// End it for good, and wait for it: a child gets `leave` to go by
    /// itself once its input is closed, then is stopped; a host is told to
    /// stop, and its going awaited.
    pub fn end(&mut self, leave: Duration, grace: Duration) {
        match &self.runner {
            Runner::Child(process) => {
                let process = process.clone();
                self.close_input();
                process.wait_timeout(leave);
                process.stop(grace);
            }
            Runner::Kept(kept) => {
                kept.stop();
                if !kept.wait_over(KEPT_PATIENCE) {
                    log::warn!("the keeper did not stop in time");
                }
                self.close_input();
            }
        }
    }

    /// Let go of a link the session no longer uses, ending its program
    /// without waiting for it.
    pub fn let_go(mut self, leave: Duration, grace: Duration) {
        self.close_input();
        match self.runner {
            Runner::Child(process) => {
                let _ = thread::Builder::new()
                    .name("kou-claude let go".into())
                    .spawn(move || {
                        process.wait_timeout(leave);
                        process.stop(grace);
                    });
            }
            Runner::Kept(kept) => kept.stop(),
        }
    }

    /// Write one message. False once the process no longer reads.
    pub fn write(&mut self, message: &Value) -> bool {
        let line = message.to_string();
        log::trace!("to claude: {line}");
        match &self.outbox {
            Some(outbox) if outbox.send(line).is_ok() => true,
            _ => {
                self.outbox = None;
                false
            }
        }
    }

    /// Close its standard input, once what was written has gone.
    pub fn close_input(&mut self) {
        self.outbox = None;
    }

    /// Whether all of its output has been read and it has ended (or its
    /// keeper is gone).
    pub fn drained(&self) -> bool {
        (self.exit.is_some() || self.vanished || self.replaced)
            && self.stdout_closed
            && self.stderr_closed
    }

    /// What it last wrote to standard error, a line each.
    pub fn last_words(&self) -> Vec<String> {
        self.stderr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> anyhow::Result<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(body)
        .with_context(|| format!("could not start the {name} thread"))?;
    Ok(())
}

/// Claude Code's input: one message a line, flushed as it is written. Ends,
/// closing the pipe, when the link drops its sender or Claude Code stops
/// reading.
fn write_lines(mut stdin: impl Write, lines: mpsc::Receiver<String>) {
    for line in lines {
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            log::debug!("claude stopped reading: {error}");
            return;
        }
    }
}

/// Claude Code's output: one JSON object a line. A line that is not one is
/// logged and skipped, as the stream after it is still good.
fn read_messages(stdout: impl Read, id: u64, name: &str, events: &mpsc::Sender<Event>) {
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
        match serde_json::from_str::<Value>(text) {
            Ok(message @ Value::Object(_)) => {
                if events.send(Event::Message(id, message)).is_err() {
                    return;
                }
            }
            Ok(_) => log::warn!(
                "{name} wrote a line that is not a message: {}",
                clip(text, LAST_WORD_LENGTH)
            ),
            Err(error) => log::warn!(
                "{name} wrote a line that is not JSON ({error}): {}",
                clip(text, LAST_WORD_LENGTH)
            ),
        }
    }
    let _ = events.send(Event::StdoutClosed(id));
}

/// Claude Code's standard error is its log: it goes to ours, and its last
/// lines are kept to explain a death.
fn read_errors(
    stderr: impl Read,
    id: u64,
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
        keep_last_word(last_words, text);
    }
    let _ = events.send(Event::StderrClosed(id));
}

/// Keep `text`, a line of standard error, among the last ones.
pub(crate) fn keep_last_word(last_words: &Mutex<VecDeque<String>>, text: &str) {
    let mut kept = last_words
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if kept.len() == LAST_WORDS {
        kept.pop_front();
    }
    kept.push_back(clip(text, LAST_WORD_LENGTH));
}

/// At most `limit` characters of `text`.
pub(crate) fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::McpServer;

    fn options() -> Options {
        Options::new("/bin/claude", "/work")
    }

    #[test]
    fn the_command_line_is_the_sdk_s_with_what_the_options_add() {
        let mut options = options();
        options.mcp_servers.push(McpServer {
            name: "kou".into(),
            command: "/bin/kou".into(),
            args: vec!["mcp".into()],
            env: vec![("KOU_PANE".into(), "3".into())],
        });
        options.instructions = Some("Be brief.".into());
        options.plugins = vec!["/plugins/a".into(), "/plugins/b".into()];
        let base = [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
            "--allow-dangerously-skip-permissions",
        ];
        let mut expected: Vec<String> = base.map(str::to_owned).to_vec();
        expected.extend(
            [
                "--resume=s-1",
                "--model",
                "opus",
                "--mcp-config",
                r#"{"mcpServers":{"kou":{"command":"/bin/kou","args":["mcp"],"env":{"KOU_PANE":"3"}}}}"#,
                "--append-system-prompt",
                "Be brief.",
                "--plugin-dir",
                "/plugins/a",
                "--plugin-dir",
                "/plugins/b",
            ]
            .map(str::to_owned),
        );
        let resume = Resume::Session("s-1".into());
        assert_eq!(arguments(&options, &resume, Some("opus"), false), expected);

        // Only asked its models: nothing a conversation needs.
        let lean = arguments(&options, &Resume::New, Some("opus"), true);
        let mut wanted: Vec<String> = base.map(str::to_owned).to_vec();
        wanted.extend(["--model".to_owned(), "opus".to_owned()]);
        assert_eq!(lean, wanted);

        let fork = Resume::Fork {
            session: "s-1".into(),
            message: "m-9".into(),
        };
        let forked = arguments(&Options::new("/bin/claude", "/work"), &fork, None, false);
        assert_eq!(
            forked[base.len()..],
            ["--resume=s-1", "--resume-session-at=m-9", "--fork-session"]
        );
    }

    #[test]
    fn the_environment_loses_the_marks_of_a_claude_code_session_and_asks_for_states() {
        let mut options = options();
        options.env = vec![
            ("CLAUDE_CONFIG_DIR".into(), "/accounts/two".into()),
            ("HOME".into(), "/elsewhere".into()),
        ];
        let inherited = [
            ("HOME", "/Users/me"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_SSE_PORT", "4242"),
            ("PATH", "/bin"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let env = environment(inherited, &options);
        let pairs: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("HOME", "/elsewhere"),
                ("PATH", "/bin"),
                ("CLAUDE_CONFIG_DIR", "/accounts/two"),
                ("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1"),
            ]
        );
    }

    #[test]
    fn a_long_line_is_clipped_between_characters() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("ффффф!", 5), "ффффф...");
    }
}
