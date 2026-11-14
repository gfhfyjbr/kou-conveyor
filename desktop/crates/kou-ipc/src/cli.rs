//! The command line client: the same binary as the app, used by an agent in a
//! pane to reach other panes, and by harness hooks to report what the harness
//! is doing.

use crate::protocol::{Harness, Pane, Placement, Request, Response, Spawn, Until, env};
use crate::transport::{self, NotRunning};
use std::io::{IsTerminal as _, Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// What `--help` prints.
pub const USAGE: &str = "\
kou-desktop: a workspace for coding agents, and the command line to drive it

  kou-desktop                                 start the app
  kou-desktop ping
  kou-desktop harness list [--json]
  kou-desktop pane list [--all] [--json]
  kou-desktop pane info [<pane>]                       (default: $KOU_PANE)
  kou-desktop pane spawn <harness> [--cwd DIR] [--title T] [--prompt TEXT | --prompt-file F | -] [--model M] [--tab | --split | --split-down] [--share PERCENT] [--native]
  kou-desktop pane send <pane> [--no-submit] <text...> (text from stdin when \"-\")
  kou-desktop pane read <pane> [--lines N] [--scrollback]
  kou-desktop pane wait <pane>... [--exit] [--any] [--timeout SECONDS]
  kou-desktop pane close <pane>
  kou-desktop pane focus <pane>
  kou-desktop tab list [--all] [--json]
  kou-desktop tab rename [--tab ID] <name...>          (default: the tab $KOU_PANE is in)
  kou-desktop tab rename [--tab ID] --clear            (called after its pane again)
  kou-desktop tab icon [--tab ID] <icon>               (one of tab icons; auto: what runs in it)
  kou-desktop tab icons
  kou-desktop session list [--json]
  kou-desktop session rename [--session ID] <name...>  (default: $KOU_SESSION)
  kou-desktop space list [--json]
  kou-desktop space rename [--space ID] <name...>      (default: the space $KOU_PANE is in)
  kou-desktop hook <harness> <event>                   (payload JSON on stdin, may be empty)

Every command takes --json to print the app's raw answer.
Exit status: 0 success, 1 the app refused, 2 bad usage, 3 the app is not running,
4 a wait timed out.
";

/// Success.
pub const EXIT_OK: i32 = 0;
/// The app answered with an error.
pub const EXIT_ERROR: i32 = 1;
/// The arguments made no sense.
pub const EXIT_USAGE: i32 = 2;
/// Nothing listens on the socket.
pub const EXIT_NOT_RUNNING: i32 = 3;
/// A `pane wait` ran out of time.
pub const EXIT_TIMED_OUT: i32 = 4;

/// A hook is called on every tool call of a harness and must never slow it
/// down: after this long it gives up on the app and exits.
const HOOK_BUDGET: Duration = Duration::from_millis(150);

/// How much of that budget may go to waiting for a payload on stdin.
const STDIN_BUDGET: Duration = Duration::from_millis(60);

/// The budget of a hook that is not about a tool call: a prompt sent, a turn
/// ended, a question asked, a session begun or over. These come once in a
/// while, so waiting costs the harness nothing it would notice, and they are
/// what the app's marks and notifications hang on: one lost on a machine
/// busy with the agents' own work leaves a pane working for ever. Well
/// under the five seconds a harness gives a hook.
const PATIENT_HOOK_BUDGET: Duration = Duration::from_secs(2);

/// How much of that may go to waiting for a payload, which for some of
/// these events says what happened.
const PATIENT_STDIN_BUDGET: Duration = Duration::from_millis(500);

/// How long a hook for `event` may take in all, and how much of that
/// waiting for its payload. Every harness names its tool events with the
/// word (`PreToolUse`, `postToolUse`, `BeforeTool`, `AfterTool`).
fn hook_budget(event: &str) -> (Duration, Duration) {
    if event.to_ascii_lowercase().contains("tool") {
        (HOOK_BUDGET, STDIN_BUDGET)
    } else {
        (PATIENT_HOOK_BUDGET, PATIENT_STDIN_BUDGET)
    }
}

/// How long an ordinary request may take to be answered.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Run a client command. `args` are the arguments after the program name.
///
/// Returns `None` when they are not a client command and the app should
/// start, or `Some(status)` when a command ran. The caller must then end the
/// process with that status (`std::process::exit`) rather than return from
/// `main` and wait: a hook may leave a thread behind that is still blocked
/// reading a stdin its harness never closes.
pub fn run(args: &[String]) -> Option<i32> {
    if !is_client_command(args) {
        return None;
    }
    let env = Env::from_process();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    Some(run_with(
        args,
        &env,
        Input::Process,
        &mut stdout.lock(),
        &mut stderr.lock(),
    ))
}

fn is_client_command(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some(
            "ping"
                | "harness"
                | "pane"
                | "tab"
                | "session"
                | "space"
                | "hook"
                | "help"
                | "--help"
                | "-h"
        )
    )
}

/// The parts of the process environment the client reads.
#[derive(Clone, Debug, Default)]
struct Env {
    socket: PathBuf,
    pane: Option<String>,
    session: Option<String>,
    cwd: PathBuf,
}

impl Env {
    fn from_process() -> Self {
        let var = |name| {
            std::env::var(name)
                .ok()
                .filter(|value: &String| !value.is_empty())
        };
        Env {
            socket: transport::socket_path(),
            pane: var(env::PANE),
            session: var(env::SESSION),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// Where text read "from stdin" comes from.
enum Input {
    /// The process's standard input.
    Process,
    /// Fixed text, for tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Text(String),
}

impl Input {
    fn read(self) -> std::io::Result<String> {
        match self {
            Input::Text(text) => Ok(text),
            Input::Process => {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                Ok(text)
            }
        }
    }

    /// A terminal on stdin means no one piped a payload; reading would wait
    /// for the user to type one.
    fn is_interactive(&self) -> bool {
        matches!(self, Input::Process) && std::io::stdin().is_terminal()
    }
}

/// Text given inline, or to be read from somewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Text {
    Inline(String),
    File(PathBuf),
    Stdin,
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    Help,
    Ping,
    Harnesses,
    PaneList {
        all: bool,
    },
    PaneInfo {
        pane: Option<String>,
    },
    PaneSpawn {
        harness: String,
        cwd: Option<PathBuf>,
        title: Option<String>,
        prompt: Option<Text>,
        model: Option<String>,
        placement: Placement,
        below: bool,
        share: Option<u8>,
        native: bool,
    },
    PaneSend {
        pane: String,
        text: Text,
        submit: bool,
    },
    PaneRead {
        pane: String,
        lines: Option<u32>,
        scrollback: bool,
    },
    PaneWait {
        panes: Vec<String>,
        until: Until,
        any: bool,
        timeout: Option<Duration>,
    },
    PaneClose {
        pane: String,
    },
    PaneFocus {
        pane: String,
    },
    TabList {
        all: bool,
    },
    TabRename {
        tab: Option<String>,
        /// Empty with `--clear`: the tab is called after its pane again.
        name: String,
    },
    TabIcon {
        tab: Option<String>,
        icon: String,
    },
    TabIcons,
    SessionList,
    SessionRename {
        session: Option<String>,
        name: String,
    },
    SpaceList,
    SpaceRename {
        space: Option<String>,
        name: String,
    },
    Hook {
        harness: String,
        event: String,
        /// Codex's `notify` passes its payload as an argument, not on stdin.
        payload: Option<String>,
    },
}

/// A command and whether to print the raw answer.
#[derive(Clone, Debug, PartialEq)]
struct Parsed {
    command: Command,
    json: bool,
}

/// The arguments of one subcommand, consumed front to back.
struct Args<'a> {
    rest: std::slice::Iter<'a, String>,
    json: bool,
    /// After `--`, everything is positional.
    literal: bool,
}

enum Token<'a> {
    Flag(&'a str),
    Word(&'a str),
}

impl<'a> Args<'a> {
    fn new(args: &'a [String]) -> Self {
        Args {
            rest: args.iter(),
            json: false,
            literal: false,
        }
    }

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let arg = self.rest.next()?.as_str();
            if self.literal {
                return Some(Token::Word(arg));
            }
            match arg {
                "--" => self.literal = true,
                "--json" => self.json = true,
                // A lone dash is a word: it names stdin.
                _ if arg.starts_with('-') && arg != "-" => return Some(Token::Flag(arg)),
                _ => return Some(Token::Word(arg)),
            }
        }
    }

    fn value(&mut self, flag: &str) -> Result<&'a str, String> {
        self.rest
            .next()
            .map(String::as_str)
            .ok_or_else(|| format!("{flag} needs a value"))
    }

    /// No flags and no positional arguments beyond `--json`.
    fn finish(mut self, command: Command) -> Result<Parsed, String> {
        match self.next() {
            None => Ok(Parsed {
                command,
                json: self.json,
            }),
            Some(Token::Flag(flag)) => Err(unknown_flag(flag)),
            Some(Token::Word(word)) => Err(format!("unexpected argument {word:?}")),
        }
    }
}

fn unknown_flag(flag: &str) -> String {
    format!("unknown option {flag}")
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let (first, rest) = args.split_first().ok_or("no command")?;
    if args.iter().any(|arg| arg == "--help" || arg == "-h") || first == "help" {
        // A hook's event or payload may be anything, `--help` included.
        if first != "hook" {
            return Ok(Parsed {
                command: Command::Help,
                json: false,
            });
        }
    }
    match first.as_str() {
        "ping" => Args::new(rest).finish(Command::Ping),
        "harness" => match rest.split_first() {
            Some((sub, rest)) if sub == "list" => Args::new(rest).finish(Command::Harnesses),
            Some((sub, _)) => Err(format!("unknown command: harness {sub}")),
            None => Err("harness needs a command: list".into()),
        },
        "pane" => match rest.split_first() {
            Some((sub, rest)) => parse_pane(sub, rest),
            None => Err(
                "pane needs a command: list, info, spawn, send, read, wait, close, focus".into(),
            ),
        },
        "tab" => match rest.split_first() {
            Some((sub, rest)) => parse_tab(sub, rest),
            None => Err("tab needs a command: list, rename, icon, icons".into()),
        },
        "session" => match rest.split_first() {
            Some((sub, rest)) => parse_session(sub, rest),
            None => Err("session needs a command: list, rename".into()),
        },
        "space" => match rest.split_first() {
            Some((sub, rest)) => parse_space(sub, rest),
            None => Err("space needs a command: list, rename".into()),
        },
        "hook" => parse_hook(rest),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_hook(rest: &[String]) -> Result<Parsed, String> {
    // No option parsing here: every argument is the harness's own.
    match rest {
        [harness, event] => Ok(Command::Hook {
            harness: harness.clone(),
            event: event.clone(),
            payload: None,
        }),
        [harness, event, payload] => Ok(Command::Hook {
            harness: harness.clone(),
            event: event.clone(),
            payload: Some(payload.clone()),
        }),
        _ => Err("usage: hook <harness> <event>".into()),
    }
    .map(|command| Parsed {
        command,
        json: false,
    })
}

fn parse_pane(sub: &str, rest: &[String]) -> Result<Parsed, String> {
    let mut args = Args::new(rest);
    let command = match sub {
        "list" => Command::PaneList {
            all: parse_all(&mut args)?,
        },
        "info" => {
            let mut pane = None;
            while let Some(token) = args.next() {
                match token {
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) if pane.is_none() => pane = Some(word.to_owned()),
                    Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
                }
            }
            Command::PaneInfo { pane }
        }
        "spawn" => parse_spawn(&mut args)?,
        "send" => {
            let mut pane = None;
            let mut submit = true;
            let mut words: Vec<&str> = Vec::new();
            while let Some(token) = args.next() {
                match token {
                    Token::Flag("--no-submit") => submit = false,
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) if pane.is_none() => pane = Some(word.to_owned()),
                    Token::Word(word) => words.push(word),
                }
            }
            let pane = pane.ok_or("usage: pane send <pane> [--no-submit] <text...>")?;
            let text = match words.as_slice() {
                [] => return Err("pane send needs text, or - to read it from stdin".into()),
                ["-"] => Text::Stdin,
                words => Text::Inline(words.join(" ")),
            };
            Command::PaneSend { pane, text, submit }
        }
        "read" => {
            let mut pane = None;
            let mut lines = None;
            let mut scrollback = false;
            while let Some(token) = args.next() {
                match token {
                    Token::Flag("--scrollback") => scrollback = true,
                    Token::Flag("--lines") => {
                        let value = args.value("--lines")?;
                        lines =
                            Some(value.parse::<u32>().map_err(|_| {
                                format!("--lines takes a whole number, not {value:?}")
                            })?);
                    }
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) if pane.is_none() => pane = Some(word.to_owned()),
                    Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
                }
            }
            Command::PaneRead {
                pane: pane.ok_or("usage: pane read <pane> [--lines N] [--scrollback]")?,
                lines,
                scrollback,
            }
        }
        "wait" => {
            let mut panes = Vec::new();
            let mut until = Until::Idle;
            let mut any = false;
            let mut timeout = None;
            while let Some(token) = args.next() {
                match token {
                    Token::Flag("--exit") => until = Until::Exit,
                    Token::Flag("--any") => any = true,
                    Token::Flag("--timeout") => {
                        let value = args.value("--timeout")?;
                        let seconds = value
                            .parse::<f64>()
                            .ok()
                            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                            .ok_or_else(|| {
                                format!("--timeout takes a number of seconds, not {value:?}")
                            })?;
                        timeout = Some(Duration::from_secs_f64(seconds));
                    }
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) => panes.push(word.to_owned()),
                }
            }
            if panes.is_empty() {
                return Err(
                    "usage: pane wait <pane>... [--exit] [--any] [--timeout SECONDS]".into(),
                );
            }
            Command::PaneWait {
                panes,
                until,
                any,
                timeout,
            }
        }
        "close" | "focus" => {
            let mut pane = None;
            while let Some(token) = args.next() {
                match token {
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) if pane.is_none() => pane = Some(word.to_owned()),
                    Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
                }
            }
            let pane = pane.ok_or_else(|| format!("usage: pane {sub} <pane>"))?;
            if sub == "close" {
                Command::PaneClose { pane }
            } else {
                Command::PaneFocus { pane }
            }
        }
        other => return Err(format!("unknown command: pane {other}")),
    };
    Ok(Parsed {
        command,
        json: args.json,
    })
}

/// The options of a list that defaults to the caller's session: `--all`, and
/// nothing else.
fn parse_all(args: &mut Args<'_>) -> Result<bool, String> {
    let mut all = false;
    while let Some(token) = args.next() {
        match token {
            Token::Flag("--all") => all = true,
            Token::Flag(flag) => return Err(unknown_flag(flag)),
            Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
        }
    }
    Ok(all)
}

fn parse_tab(sub: &str, rest: &[String]) -> Result<Parsed, String> {
    let mut args = Args::new(rest);
    let command = match sub {
        "list" => Command::TabList {
            all: parse_all(&mut args)?,
        },
        "rename" => {
            let mut tab = None;
            let mut clear = false;
            let mut words: Vec<&str> = Vec::new();
            while let Some(token) = args.next() {
                match token {
                    Token::Flag("--tab") => tab = Some(args.value("--tab")?.to_owned()),
                    Token::Flag("--clear") => clear = true,
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) => words.push(word),
                }
            }
            let name = match (words.is_empty(), clear) {
                (true, false) => return Err("tab rename needs a name, or --clear".into()),
                (false, true) => return Err("tab rename takes a name or --clear, not both".into()),
                (true, true) => String::new(),
                (false, false) => join_name("tab", &words)?,
            };
            Command::TabRename { tab, name }
        }
        "icon" => {
            let mut tab = None;
            let mut icon = None;
            while let Some(token) = args.next() {
                match token {
                    Token::Flag("--tab") => tab = Some(args.value("--tab")?.to_owned()),
                    Token::Flag(flag) => return Err(unknown_flag(flag)),
                    Token::Word(word) if icon.is_none() => icon = Some(word.to_owned()),
                    Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
                }
            }
            Command::TabIcon {
                tab,
                icon: icon.ok_or("usage: tab icon [--tab ID] <icon>")?,
            }
        }
        "icons" => return args.finish(Command::TabIcons),
        other => return Err(format!("unknown command: tab {other}")),
    };
    Ok(Parsed {
        command,
        json: args.json,
    })
}

fn parse_session(sub: &str, rest: &[String]) -> Result<Parsed, String> {
    let mut args = Args::new(rest);
    let command = match sub {
        "list" => return args.finish(Command::SessionList),
        "rename" => {
            let (session, name) = parse_rename(&mut args, "session")?;
            Command::SessionRename { session, name }
        }
        other => return Err(format!("unknown command: session {other}")),
    };
    Ok(Parsed {
        command,
        json: args.json,
    })
}

fn parse_space(sub: &str, rest: &[String]) -> Result<Parsed, String> {
    let mut args = Args::new(rest);
    let command = match sub {
        "list" => return args.finish(Command::SpaceList),
        "rename" => {
            let (space, name) = parse_rename(&mut args, "space")?;
            Command::SpaceRename { space, name }
        }
        other => return Err(format!("unknown command: space {other}")),
    };
    Ok(Parsed {
        command,
        json: args.json,
    })
}

/// `<noun> rename [--<noun> ID] <name...>`: the id given, and the new name.
fn parse_rename(args: &mut Args<'_>, noun: &str) -> Result<(Option<String>, String), String> {
    let id_flag = format!("--{noun}");
    let mut id = None;
    let mut words: Vec<&str> = Vec::new();
    while let Some(token) = args.next() {
        match token {
            Token::Flag(flag) if flag == id_flag => id = Some(args.value(flag)?.to_owned()),
            Token::Flag(flag) => return Err(unknown_flag(flag)),
            Token::Word(word) => words.push(word),
        }
    }
    if words.is_empty() {
        return Err(format!("{noun} rename needs a name"));
    }
    Ok((id, join_name(noun, &words)?))
}

/// Every word of a name, joined: quoting it is optional.
fn join_name(noun: &str, words: &[&str]) -> Result<String, String> {
    let name = words.join(" ");
    if name.trim().is_empty() {
        return Err(format!("a {noun}'s name cannot be empty"));
    }
    Ok(name)
}

fn parse_spawn(args: &mut Args<'_>) -> Result<Command, String> {
    let mut harness = None;
    let mut cwd = None;
    let mut title = None;
    let mut prompt: Option<Text> = None;
    let mut model = None;
    let mut placement = None;
    let mut below = false;
    let mut share = None;
    let mut native = false;
    let set_prompt = |prompt: &mut Option<Text>, text: Text| match prompt {
        Some(_) => Err("give the prompt once: --prompt, --prompt-file or -".to_owned()),
        None => {
            *prompt = Some(text);
            Ok(())
        }
    };
    let mut set_placement = |value: Placement| match placement.replace(value) {
        Some(_) => Err("--tab, --split and --split-down exclude each other".to_owned()),
        None => Ok(()),
    };
    while let Some(token) = args.next() {
        match token {
            Token::Flag("--cwd") => cwd = Some(PathBuf::from(args.value("--cwd")?)),
            Token::Flag("--title") => title = Some(args.value("--title")?.to_owned()),
            Token::Flag("--model") => model = Some(args.value("--model")?.to_owned()),
            Token::Flag("--prompt") => {
                let text = Text::Inline(args.value("--prompt")?.to_owned());
                set_prompt(&mut prompt, text)?;
            }
            Token::Flag("--prompt-file") => {
                let text = Text::File(PathBuf::from(args.value("--prompt-file")?));
                set_prompt(&mut prompt, text)?;
            }
            Token::Flag("--tab") => set_placement(Placement::Tab)?,
            Token::Flag("--split") => set_placement(Placement::Split)?,
            Token::Flag("--split-down") => {
                set_placement(Placement::Split)?;
                below = true;
            }
            Token::Flag("--share") => {
                // How much of the caller's room the new pane takes: "30" or "30%".
                let value = args.value("--share")?;
                let percent = value.trim_end_matches('%').parse::<u8>().ok();
                share = Some(
                    percent
                        .filter(|percent| (10..=90).contains(percent))
                        .ok_or_else(|| {
                            format!("--share takes a percentage from 10 to 90, not {value:?}")
                        })?,
                );
            }
            Token::Flag("--native") => native = true,
            Token::Flag(flag) => return Err(unknown_flag(flag)),
            Token::Word("-") => set_prompt(&mut prompt, Text::Stdin)?,
            Token::Word(word) if harness.is_none() => harness = Some(word.to_owned()),
            Token::Word(word) => return Err(format!("unexpected argument {word:?}")),
        }
    }
    let placement = placement.unwrap_or_default();
    if share.is_some() && placement != Placement::Split {
        return Err(
            "--share goes with --split or --split-down: it is a share of your own room".into(),
        );
    }
    Ok(Command::PaneSpawn {
        harness: harness.ok_or("usage: pane spawn <harness> [options]")?,
        cwd,
        title,
        prompt,
        model,
        placement,
        below,
        share,
        native,
    })
}

fn run_with(
    args: &[String],
    env: &Env,
    input: Input,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            // A harness must never be held up, or shown an error, by a hook
            // line we did not understand.
            if args.first().is_some_and(|first| first == "hook") {
                return EXIT_OK;
            }
            drop(writeln!(err, "kou-desktop: {message}\n\n{USAGE}"));
            return EXIT_USAGE;
        }
    };
    match parsed.command {
        Command::Help => {
            drop(write!(out, "{USAGE}"));
            EXIT_OK
        }
        Command::Hook {
            harness,
            event,
            payload,
        } => {
            hook(env, harness, event, payload, input);
            EXIT_OK
        }
        command => match request(command, env, input) {
            Ok((request, timeout)) => exchange(env, &request, timeout, parsed.json, out, err),
            Err(message) => {
                drop(writeln!(err, "kou-desktop: {message}"));
                EXIT_USAGE
            }
        },
    }
}

/// Report a hook to the app, within its budget ([`hook_budget`]) and without
/// a word on stdout: some harnesses read a hook's output as a decision.
fn hook(env: &Env, harness: String, event: String, payload: Option<String>, input: Input) {
    // Outside a pane (the generated settings got reused, say) there is no one
    // to tell.
    let Some(pane) = env.pane.clone() else {
        return;
    };
    let (budget, stdin_budget) = hook_budget(&event);
    let deadline = Instant::now() + budget;
    let raw = match payload {
        Some(payload) => payload,
        None if input.is_interactive() => String::new(),
        // A harness may keep the pipe open for as long as it lives. The event
        // matters more than its payload, so the read is given a short while
        // on a thread this function is free to abandon.
        None => {
            abandonable(stdin_budget, move || input.read().unwrap_or_default()).unwrap_or_default()
        }
    };
    let request = Request::Hook {
        pane,
        harness,
        event,
        payload: parse_payload(&raw),
    };
    // Told, not asked: the app reads the event whenever it gets to it, and
    // a hook has no use for its answer.
    let socket = env.socket.clone();
    let remaining = deadline.saturating_duration_since(Instant::now());
    abandonable(remaining, move || {
        drop(transport::tell(&socket, &request));
    });
}

/// Run `work` on a thread and wait for it no longer than `budget`. The thread
/// is left behind when it is late; the process is about to exit anyway.
fn abandonable<T: Send + 'static>(
    budget: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (done, finished) = mpsc::channel();
    thread::Builder::new()
        .name("kou-hook".into())
        .spawn(move || {
            let _ = done.send(work());
        })
        .ok()?;
    finished.recv_timeout(budget).ok()
}

/// A hook's input as JSON: `null` when empty, a string when it is not JSON.
fn parse_payload(raw: &str) -> serde_json::Value {
    let raw = raw.trim();
    if raw.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

/// Turn a command into the request to send and the time to allow for its answer.
fn request(
    command: Command,
    env: &Env,
    input: Input,
) -> Result<(Request, Option<Duration>), String> {
    let request = match command {
        Command::Help | Command::Hook { .. } => {
            return Err("this command is not a request".into());
        }
        Command::Ping => Request::Ping,
        Command::Harnesses => Request::Harnesses,
        Command::PaneList { all } => Request::PaneList {
            session: if all { None } else { env.session.clone() },
        },
        Command::PaneInfo { pane } => Request::PaneInfo {
            pane: pane
                .or_else(|| env.pane.clone())
                .ok_or("pane info needs a pane: none was given and $KOU_PANE is not set")?,
        },
        Command::PaneSpawn {
            harness,
            cwd,
            title,
            prompt,
            model,
            placement,
            below,
            share,
            native,
        } => Request::PaneSpawn(Spawn {
            from: env.pane.clone(),
            harness,
            // The app's working directory is not the caller's.
            cwd: cwd.map(|cwd| absolute(&env.cwd, &cwd)),
            title,
            prompt: prompt
                .map(|prompt| read_text(prompt, env, input))
                .transpose()?,
            model,
            placement,
            below,
            share,
            native,
        }),
        Command::PaneSend { pane, text, submit } => Request::PaneSend {
            pane,
            text: read_text(text, env, input)?,
            submit,
            from: env.pane.clone(),
        },
        Command::PaneRead {
            pane,
            lines,
            scrollback,
        } => Request::PaneRead {
            pane,
            lines,
            scrollback,
        },
        Command::PaneWait {
            panes,
            until,
            any,
            timeout,
        } => {
            let request = Request::PaneWait {
                panes,
                until,
                any,
                timeout_ms: timeout.map(|timeout| timeout.as_millis() as u64),
            };
            // The app keeps the time; the client only allows for a late answer.
            return Ok((request, timeout.map(|timeout| timeout + CALL_TIMEOUT)));
        }
        Command::PaneClose { pane } => Request::PaneClose { pane },
        Command::PaneFocus { pane } => Request::PaneFocus { pane },
        Command::TabList { all } => Request::TabList {
            session: if all { None } else { env.session.clone() },
        },
        // Without an id the app finds the tab, session or space from the
        // asking pane.
        Command::TabRename { tab, name } => {
            if tab.is_none() && env.pane.is_none() {
                return Err(no_target("tab rename", "tab"));
            }
            Request::TabSet {
                tab,
                pane: env.pane.clone(),
                name: Some(name),
                icon: None,
            }
        }
        Command::TabIcon { tab, icon } => {
            if tab.is_none() && env.pane.is_none() {
                return Err(no_target("tab icon", "tab"));
            }
            Request::TabSet {
                tab,
                pane: env.pane.clone(),
                name: None,
                icon: Some(icon),
            }
        }
        Command::TabIcons => Request::TabIcons,
        Command::SessionList => Request::SessionList { space: None },
        Command::SessionRename { session, name } => {
            let session = session.or_else(|| env.session.clone());
            if session.is_none() && env.pane.is_none() {
                return Err("session rename needs a session: no --session was given \
                            and neither $KOU_SESSION nor $KOU_PANE is set"
                    .into());
            }
            Request::SessionRename {
                session,
                pane: env.pane.clone(),
                name,
            }
        }
        Command::SpaceList => Request::SpaceList,
        Command::SpaceRename { space, name } => {
            if space.is_none() && env.pane.is_none() {
                return Err(no_target("space rename", "space"));
            }
            Request::SpaceRename {
                space,
                pane: env.pane.clone(),
                name,
            }
        }
    };
    Ok((request, Some(CALL_TIMEOUT)))
}

/// Why `command` cannot tell the app which `noun` it means.
fn no_target(command: &str, noun: &str) -> String {
    format!("{command} needs a {noun}: no --{noun} was given and $KOU_PANE is not set")
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn read_text(text: Text, env: &Env, input: Input) -> Result<String, String> {
    match text {
        Text::Inline(text) => Ok(text),
        Text::Stdin => input
            .read()
            .map_err(|error| format!("could not read stdin: {error}")),
        Text::File(path) => {
            let path = absolute(&env.cwd, &path);
            std::fs::read_to_string(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))
        }
    }
}

fn exchange(
    env: &Env,
    request: &Request,
    timeout: Option<Duration>,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let response = match transport::call(&env.socket, request, timeout) {
        Ok(response) => response,
        Err(error) => {
            drop(writeln!(err, "kou-desktop: {error:#}"));
            return if error.downcast_ref::<NotRunning>().is_some() {
                EXIT_NOT_RUNNING
            } else {
                EXIT_ERROR
            };
        }
    };
    let status = match &response {
        Response::Error { .. } => EXIT_ERROR,
        Response::Waited {
            timed_out: true, ..
        } => EXIT_TIMED_OUT,
        _ => EXIT_OK,
    };
    if json {
        match serde_json::to_string(&response) {
            Ok(line) => drop(writeln!(out, "{line}")),
            Err(error) => drop(writeln!(err, "kou-desktop: {error}")),
        }
        return status;
    }
    match &response {
        Response::Pong { version } => drop(writeln!(out, "pong (protocol {version})")),
        Response::Done => drop(writeln!(out, "ok")),
        Response::Pane(pane) => drop(writeln!(out, "{}", pane_line(pane))),
        Response::Panes { panes } => {
            for pane in panes {
                drop(writeln!(out, "{}", pane_line(pane)));
            }
        }
        Response::Text { text } => {
            drop(write!(out, "{text}"));
            if !text.is_empty() && !text.ends_with('\n') {
                drop(writeln!(out));
            }
        }
        Response::Waited { panes, timed_out } => {
            for pane in panes {
                drop(writeln!(out, "{}", pane_line(pane)));
            }
            if *timed_out {
                drop(writeln!(err, "kou-desktop: the wait timed out"));
            }
        }
        Response::Harnesses { harnesses } => {
            for harness in harnesses {
                drop(writeln!(out, "{}", harness_line(harness)));
            }
        }
        Response::Tab(tab) => drop(writeln!(out, "{}", tab_line(tab))),
        Response::Tabs { tabs } => {
            for tab in tabs {
                drop(writeln!(out, "{}", tab_line(tab)));
            }
        }
        Response::Session(session) => drop(writeln!(out, "{}", session_line(session))),
        Response::Sessions { sessions } => {
            for session in sessions {
                drop(writeln!(out, "{}", session_line(session)));
            }
        }
        Response::Space(space) => drop(writeln!(out, "{}", space_line(space))),
        Response::Spaces { spaces } => {
            for space in spaces {
                drop(writeln!(out, "{}", space_line(space)));
            }
        }
        Response::Error { message } => drop(writeln!(err, "kou-desktop: {message}")),
    }
    status
}

/// One tab on one line, tab separated: id, session, `front` or `-`, layout,
/// icon or `-`, its panes joined by commas, name.
fn tab_line(tab: &crate::protocol::Tab) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        tab.id,
        tab.session,
        if tab.front { "front" } else { "-" },
        word(&tab.layout),
        tab.icon.as_deref().unwrap_or("-"),
        tab.panes.join(","),
        single_line(&tab.name),
    )
}

/// One session on one line, tab separated: id, space, `selected`, `archived`
/// or `-`, how many tabs, directory, name.
fn session_line(session: &crate::protocol::Session) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        session.id,
        session.space,
        if session.selected {
            "selected"
        } else if session.archived {
            "archived"
        } else {
            "-"
        },
        session.tabs,
        session.cwd.display(),
        single_line(&session.name),
    )
}

/// One space on one line, tab separated: id, how many sessions, directory, name.
fn space_line(space: &crate::protocol::Space) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        space.id,
        space.sessions,
        space.path.display(),
        single_line(&space.name),
    )
}

/// One pane on one line, tab separated so an agent can cut it:
/// id, activity, harness, kind, age of the activity, directory, title.
fn pane_line(pane: &Pane) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        pane.id,
        word(&pane.activity),
        pane.harness,
        word(&pane.kind),
        age(pane.since_ms),
        pane.cwd.display(),
        single_line(&pane.title),
    )
}

/// One harness on one line: id, readiness, version, whether it runs natively,
/// name, and the reason it is not ready.
fn harness_line(harness: &Harness) -> String {
    let mut line = format!(
        "{}\t{}\t{}\t{}\t{}",
        harness.id,
        if harness.ready {
            "ready"
        } else {
            "unavailable"
        },
        harness.version.as_deref().unwrap_or("-"),
        if harness.native { "native" } else { "terminal" },
        harness.name,
    );
    if let Some(message) = &harness.message {
        line.push('\t');
        line.push_str(&single_line(message));
    }
    line
}

/// The protocol's own spelling of an enum value, so the readable output and
/// `--json` agree.
fn word<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        _ => "?".into(),
    }
}

fn age(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Activity, Layout, PaneKind, Session, Space, Tab};

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn command(line: &str) -> Command {
        parse(&args(line))
            .unwrap_or_else(|error| panic!("{line}: {error}"))
            .command
    }

    #[test]
    fn only_client_commands_keep_the_app_from_starting() {
        for line in [
            "ping",
            "harness list",
            "pane list",
            "tab list",
            "session list",
            "space list",
            "hook claude Stop",
            "--help",
            "-h",
            "help",
        ] {
            assert!(is_client_command(&args(line)), "{line}");
        }
        for line in ["", "--new-window", "/some/project", "open"] {
            assert!(!is_client_command(&args(line)), "{line}");
            assert_eq!(run(&args(line)), None, "{line}");
        }
    }

    #[test]
    fn every_command_parses() {
        assert_eq!(command("ping"), Command::Ping);
        assert_eq!(command("harness list"), Command::Harnesses);
        assert_eq!(command("pane list"), Command::PaneList { all: false });
        assert_eq!(command("pane list --all"), Command::PaneList { all: true });
        assert_eq!(command("pane info"), Command::PaneInfo { pane: None });
        assert_eq!(
            command("pane info p7"),
            Command::PaneInfo {
                pane: Some("p7".into())
            }
        );
        assert_eq!(
            command("pane spawn codex"),
            Command::PaneSpawn {
                harness: "codex".into(),
                cwd: None,
                title: None,
                prompt: None,
                model: None,
                placement: Placement::Auto,
                below: false,
                share: None,
                native: false,
            }
        );
        let spawn = [
            "pane",
            "spawn",
            "claude",
            "--cwd",
            "sub dir",
            "--title",
            "Review",
            "--prompt",
            "read the diff",
            "--model",
            "opus",
            "--tab",
            "--native",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&spawn).unwrap().command,
            Command::PaneSpawn {
                harness: "claude".into(),
                cwd: Some("sub dir".into()),
                title: Some("Review".into()),
                prompt: Some(Text::Inline("read the diff".into())),
                model: Some("opus".into()),
                placement: Placement::Tab,
                below: false,
                share: None,
                native: true,
            }
        );
        // A split to the right or below, with the share of the caller's
        // room the new pane takes.
        assert!(matches!(
            command("pane spawn codex --split --share 30"),
            Command::PaneSpawn {
                placement: Placement::Split,
                below: false,
                share: Some(30),
                ..
            }
        ));
        assert!(matches!(
            command("pane spawn codex --split-down --share 25%"),
            Command::PaneSpawn {
                placement: Placement::Split,
                below: true,
                share: Some(25),
                ..
            }
        ));
        for wrong in [
            "pane spawn codex --share 30",
            "pane spawn codex --tab --share 30",
            "pane spawn codex --split --share 5",
            "pane spawn codex --split --share most",
            "pane spawn codex --split --split-down",
        ] {
            assert!(parse(&args(wrong)).is_err(), "{wrong}");
        }
        assert!(matches!(
            command("pane spawn pi --prompt-file plan.md --split"),
            Command::PaneSpawn {
                prompt: Some(Text::File(_)),
                placement: Placement::Split,
                ..
            }
        ));
        assert!(matches!(
            command("pane spawn pi -"),
            Command::PaneSpawn {
                prompt: Some(Text::Stdin),
                ..
            }
        ));
        assert_eq!(
            command("pane send p2 run the tests"),
            Command::PaneSend {
                pane: "p2".into(),
                text: Text::Inline("run the tests".into()),
                submit: true,
            }
        );
        assert_eq!(
            command("pane send p2 --no-submit -"),
            Command::PaneSend {
                pane: "p2".into(),
                text: Text::Stdin,
                submit: false,
            }
        );
        assert_eq!(
            command("pane send p2 -- --not-a-flag"),
            Command::PaneSend {
                pane: "p2".into(),
                text: Text::Inline("--not-a-flag".into()),
                submit: true,
            }
        );
        assert_eq!(
            command("pane read p2"),
            Command::PaneRead {
                pane: "p2".into(),
                lines: None,
                scrollback: false,
            }
        );
        assert_eq!(
            command("pane read p2 --lines 40 --scrollback"),
            Command::PaneRead {
                pane: "p2".into(),
                lines: Some(40),
                scrollback: true,
            }
        );
        assert_eq!(
            command("pane wait p2"),
            Command::PaneWait {
                panes: vec!["p2".into()],
                until: Until::Idle,
                any: false,
                timeout: None,
            }
        );
        assert_eq!(
            command("pane wait p2 p3 --exit --any --timeout 1.5"),
            Command::PaneWait {
                panes: vec!["p2".into(), "p3".into()],
                until: Until::Exit,
                any: true,
                timeout: Some(Duration::from_millis(1500)),
            }
        );
        assert_eq!(
            command("pane close p2"),
            Command::PaneClose { pane: "p2".into() }
        );
        assert_eq!(
            command("pane focus p2"),
            Command::PaneFocus { pane: "p2".into() }
        );
        assert_eq!(command("tab list"), Command::TabList { all: false });
        assert_eq!(command("tab list --all"), Command::TabList { all: true });
        assert_eq!(
            command("tab rename api server"),
            Command::TabRename {
                tab: None,
                name: "api server".into(),
            }
        );
        // Quoting is optional: one word with a space is the same name.
        assert_eq!(
            parse(&["tab", "rename", "api server"].map(str::to_owned))
                .unwrap()
                .command,
            command("tab rename api server")
        );
        assert_eq!(
            command("tab rename --tab t2 api"),
            Command::TabRename {
                tab: Some("t2".into()),
                name: "api".into(),
            }
        );
        assert_eq!(
            command("tab rename --clear --tab t2"),
            Command::TabRename {
                tab: Some("t2".into()),
                name: String::new(),
            }
        );
        assert_eq!(
            command("tab rename -- --clear -x"),
            Command::TabRename {
                tab: None,
                name: "--clear -x".into(),
            }
        );
        assert_eq!(
            command("tab icon terminal"),
            Command::TabIcon {
                tab: None,
                icon: "terminal".into(),
            }
        );
        assert_eq!(
            command("tab icon --tab t2 auto"),
            Command::TabIcon {
                tab: Some("t2".into()),
                icon: "auto".into(),
            }
        );
        assert_eq!(command("tab icons"), Command::TabIcons);
        assert_eq!(command("session list"), Command::SessionList);
        assert_eq!(
            command("session rename Payments rework"),
            Command::SessionRename {
                session: None,
                name: "Payments rework".into(),
            }
        );
        assert_eq!(
            command("session rename --session s2 -- -draft-"),
            Command::SessionRename {
                session: Some("s2".into()),
                name: "-draft-".into(),
            }
        );
        assert_eq!(command("space list"), Command::SpaceList);
        assert_eq!(
            command("space rename Client work"),
            Command::SpaceRename {
                space: None,
                name: "Client work".into(),
            }
        );
        assert_eq!(
            command("space rename Client --space w2"),
            Command::SpaceRename {
                space: Some("w2".into()),
                name: "Client".into(),
            }
        );
        assert_eq!(
            command("hook claude PreToolUse"),
            Command::Hook {
                harness: "claude".into(),
                event: "PreToolUse".into(),
                payload: None,
            }
        );
        assert_eq!(
            command(r#"hook codex notify {"type":"agent-turn-complete"}"#),
            Command::Hook {
                harness: "codex".into(),
                event: "notify".into(),
                payload: Some(r#"{"type":"agent-turn-complete"}"#.into()),
            }
        );
        for line in [
            "--help",
            "-h",
            "help",
            "pane spawn --help",
            "pane -h",
            "tab rename --help",
        ] {
            assert_eq!(command(line), Command::Help, "{line}");
        }
        assert!(parse(&args("pane list --json")).unwrap().json);
        assert!(parse(&args("pane wait --json p1")).unwrap().json);
        assert!(!parse(&args("pane list")).unwrap().json);
        for line in [
            "tab list --all --json",
            "tab rename api --json",
            "tab icons --json",
            "session list --json",
            "session rename --json Payments",
            "space list --json",
            "space rename Work --json",
        ] {
            assert!(parse(&args(line)).unwrap().json, "{line}");
        }
        // After `--` it is a word of the name.
        let named = parse(&args("tab rename api -- --json")).unwrap();
        assert!(!named.json);
        assert_eq!(
            named.command,
            Command::TabRename {
                tab: None,
                name: "api --json".into(),
            }
        );
    }

    #[test]
    fn bad_usage_is_refused() {
        for line in [
            "harness",
            "harness remove",
            "pane",
            "pane explode",
            "pane list extra",
            "pane list --verbose",
            "pane spawn",
            "pane spawn a b",
            "pane spawn claude --tab --split",
            "pane spawn claude --prompt a --prompt-file b",
            "pane spawn claude --cwd",
            "pane send p1",
            "pane send",
            "pane read",
            "pane read p1 --lines many",
            "pane wait",
            "pane wait p1 --timeout soon",
            "pane wait p1 --timeout -1",
            "pane close",
            "pane focus a b",
            "ping extra",
            "tab",
            "tab move",
            "tab list extra",
            "tab list --verbose",
            "tab rename",
            "tab rename --tab t2",
            "tab rename --tab",
            "tab rename api --clear",
            "tab rename --clear api",
            "tab rename --icon x api",
            "tab icon",
            "tab icon --tab t2",
            "tab icon terminal extra",
            "tab icons extra",
            "session",
            "session archive",
            "session list --all",
            "session rename",
            "session rename --session s2",
            "session rename --space w1 Payments",
            "space",
            "space delete",
            "space list extra",
            "space rename",
            "space rename --space",
            "space rename --session s1 Work",
        ] {
            assert!(parse(&args(line)).is_err(), "{line}");
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let status = run_with(
                &args(line),
                &Env::default(),
                Input::Text(String::new()),
                &mut out,
                &mut err,
            );
            assert_eq!(status, EXIT_USAGE, "{line}");
            assert!(out.is_empty(), "{line}");
            assert!(String::from_utf8_lossy(&err).contains("kou-desktop pane wait"));
        }

        // What is wrong with a name is said plainly.
        let refusal = |words: &[&str]| {
            let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            parse(&words).unwrap_err()
        };
        for (words, message) in [
            (
                &["tab"][..],
                "tab needs a command: list, rename, icon, icons",
            ),
            (&["session"], "session needs a command: list, rename"),
            (&["space"], "space needs a command: list, rename"),
            (&["tab", "rename"], "tab rename needs a name, or --clear"),
            (
                &["tab", "rename", "api", "--clear"],
                "tab rename takes a name or --clear, not both",
            ),
            (&["tab", "rename", " "], "a tab's name cannot be empty"),
            (&["tab", "icon"], "usage: tab icon [--tab ID] <icon>"),
            (&["session", "rename"], "session rename needs a name"),
            (
                &["session", "rename", ""],
                "a session's name cannot be empty",
            ),
            (
                &["session", "rename", "--session", "s2", " ", "\t"],
                "a session's name cannot be empty",
            ),
            (&["session", "list", "--all"], "unknown option --all"),
            (&["space", "rename"], "space rename needs a name"),
            (&["space", "rename", "\n"], "a space's name cannot be empty"),
        ] {
            assert_eq!(refusal(words), message, "{words:?}");
        }
    }

    #[test]
    fn naming_without_a_target_is_refused_before_calling_the_app() {
        // Calling the app would end in "not running" (3), not bad usage (2).
        let outside = Env {
            socket: "/nowhere/kou.sock".into(),
            ..Env::default()
        };
        let in_session = Env {
            session: Some("s1".into()),
            ..outside.clone()
        };
        let no_tab = "tab rename needs a tab: no --tab was given and $KOU_PANE is not set";
        for (env, line, message) in [
            (
                &outside,
                "pane info",
                "pane info needs a pane: none was given and $KOU_PANE is not set",
            ),
            (&in_session, "tab rename api", no_tab),
            (&in_session, "tab rename --clear", no_tab),
            (
                &in_session,
                "tab icon auto",
                "tab icon needs a tab: no --tab was given and $KOU_PANE is not set",
            ),
            (
                &outside,
                "session rename Payments",
                "session rename needs a session: no --session was given \
                 and neither $KOU_SESSION nor $KOU_PANE is set",
            ),
            (
                &in_session,
                "space rename Work",
                "space rename needs a space: no --space was given and $KOU_PANE is not set",
            ),
        ] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let status = run_with(
                &args(line),
                env,
                Input::Text(String::new()),
                &mut out,
                &mut err,
            );
            assert_eq!(status, EXIT_USAGE, "{line}");
            assert!(out.is_empty(), "{line}");
            assert_eq!(
                String::from_utf8(err).unwrap(),
                format!("kou-desktop: {message}\n"),
                "{line}"
            );
        }
    }

    #[test]
    fn help_prints_the_usage() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let status = run_with(
            &args("--help"),
            &Env::default(),
            Input::Text(String::new()),
            &mut out,
            &mut err,
        );
        assert_eq!(status, EXIT_OK);
        assert_eq!(String::from_utf8(out).unwrap(), USAGE);
        assert!(err.is_empty());
    }

    #[test]
    fn requests_are_filled_from_the_environment() {
        let env = Env {
            socket: "/nowhere".into(),
            pane: Some("p1".into()),
            session: Some("s1".into()),
            cwd: "/work".into(),
        };
        let build = |line: &str, stdin: &str| {
            request(command(line), &env, Input::Text(stdin.to_owned()))
                .unwrap()
                .0
        };
        assert_eq!(
            build("pane info", ""),
            Request::PaneInfo { pane: "p1".into() }
        );
        assert_eq!(
            build("pane list", ""),
            Request::PaneList {
                session: Some("s1".into())
            }
        );
        assert_eq!(
            build("pane list --all", ""),
            Request::PaneList { session: None }
        );
        assert_eq!(
            build("pane spawn codex --cwd api - --split", "fix the build\n"),
            Request::PaneSpawn(Spawn {
                from: Some("p1".into()),
                harness: "codex".into(),
                cwd: Some("/work/api".into()),
                prompt: Some("fix the build\n".into()),
                placement: Placement::Split,
                ..Spawn::default()
            })
        );
        assert_eq!(
            build("pane spawn shell --split-down --share 20", ""),
            Request::PaneSpawn(Spawn {
                from: Some("p1".into()),
                harness: "shell".into(),
                placement: Placement::Split,
                below: true,
                share: Some(20),
                ..Spawn::default()
            })
        );
        assert_eq!(
            build("pane send p2 -", "status?"),
            Request::PaneSend {
                pane: "p2".into(),
                text: "status?".into(),
                submit: true,
                from: Some("p1".into()),
            }
        );
        let (wait, timeout) = request(
            command("pane wait p2 --timeout 2"),
            &env,
            Input::Text(String::new()),
        )
        .unwrap();
        assert_eq!(
            wait,
            Request::PaneWait {
                panes: vec!["p2".into()],
                until: Until::Idle,
                any: false,
                timeout_ms: Some(2000),
            }
        );
        assert_eq!(timeout, Some(Duration::from_secs(2) + CALL_TIMEOUT));
        // Without a deadline the client waits for as long as the app does.
        let (_, timeout) =
            request(command("pane wait p2"), &env, Input::Text(String::new())).unwrap();
        assert_eq!(timeout, None);

        // No pane to default to is a usage error, not a request.
        let outside = Env {
            pane: None,
            ..env.clone()
        };
        assert!(request(command("pane info"), &outside, Input::Text(String::new())).is_err());
    }

    #[test]
    fn tabs_sessions_and_spaces_are_found_from_the_environment() {
        let in_pane = Env {
            socket: "/nowhere".into(),
            pane: Some("p1".into()),
            session: Some("s1".into()),
            cwd: "/work".into(),
        };
        let in_session = Env {
            pane: None,
            ..in_pane.clone()
        };
        let outside = Env {
            session: None,
            ..in_session.clone()
        };
        let build = |env: &Env, line: &str| {
            request(command(line), env, Input::Text(String::new())).map(|(request, _)| request)
        };
        let tab_set =
            |tab: Option<&str>, pane: Option<&str>, name: Option<&str>, icon: Option<&str>| {
                Ok(Request::TabSet {
                    tab: tab.map(str::to_owned),
                    pane: pane.map(str::to_owned),
                    name: name.map(str::to_owned),
                    icon: icon.map(str::to_owned),
                })
            };

        assert_eq!(
            build(&in_pane, "tab list"),
            Ok(Request::TabList {
                session: Some("s1".into())
            })
        );
        assert_eq!(
            build(&in_pane, "tab list --all"),
            Ok(Request::TabList { session: None })
        );
        assert_eq!(
            build(&outside, "tab list"),
            Ok(Request::TabList { session: None })
        );

        assert_eq!(
            build(&in_pane, "tab rename api server"),
            tab_set(None, Some("p1"), Some("api server"), None)
        );
        assert_eq!(
            build(&in_pane, "tab rename --tab t2 --clear"),
            tab_set(Some("t2"), Some("p1"), Some(""), None)
        );
        assert_eq!(
            build(&in_pane, "tab icon terminal"),
            tab_set(None, Some("p1"), None, Some("terminal"))
        );
        assert_eq!(
            build(&outside, "tab rename --tab t2 api"),
            tab_set(Some("t2"), None, Some("api"), None)
        );
        assert_eq!(
            build(&outside, "tab icon --tab t2 auto"),
            tab_set(Some("t2"), None, None, Some("auto"))
        );
        assert!(build(&in_session, "tab rename api").is_err());
        assert!(build(&in_session, "tab icon auto").is_err());
        assert_eq!(build(&outside, "tab icons"), Ok(Request::TabIcons));

        // Every space's sessions, wherever it is asked from.
        for env in [&in_pane, &outside] {
            assert_eq!(
                build(env, "session list"),
                Ok(Request::SessionList { space: None })
            );
        }
        assert_eq!(
            build(&in_pane, "session rename Payments"),
            Ok(Request::SessionRename {
                session: Some("s1".into()),
                pane: Some("p1".into()),
                name: "Payments".into(),
            })
        );
        assert_eq!(
            build(&in_pane, "session rename --session s2 Payments"),
            Ok(Request::SessionRename {
                session: Some("s2".into()),
                pane: Some("p1".into()),
                name: "Payments".into(),
            })
        );
        assert_eq!(
            build(&in_session, "session rename Payments"),
            Ok(Request::SessionRename {
                session: Some("s1".into()),
                pane: None,
                name: "Payments".into(),
            })
        );
        assert_eq!(
            build(
                &Env {
                    session: None,
                    ..in_pane.clone()
                },
                "session rename Payments"
            ),
            Ok(Request::SessionRename {
                session: None,
                pane: Some("p1".into()),
                name: "Payments".into(),
            })
        );
        assert_eq!(
            build(&outside, "session rename --session s2 Payments"),
            Ok(Request::SessionRename {
                session: Some("s2".into()),
                pane: None,
                name: "Payments".into(),
            })
        );
        assert!(build(&outside, "session rename Payments").is_err());

        assert_eq!(build(&outside, "space list"), Ok(Request::SpaceList));
        assert_eq!(
            build(&in_pane, "space rename Client work"),
            Ok(Request::SpaceRename {
                space: None,
                pane: Some("p1".into()),
                name: "Client work".into(),
            })
        );
        assert_eq!(
            build(&outside, "space rename --space w2 Client"),
            Ok(Request::SpaceRename {
                space: Some("w2".into()),
                pane: None,
                name: "Client".into(),
            })
        );
        assert!(build(&in_session, "space rename Client").is_err());
    }

    #[test]
    fn a_tool_calls_hook_is_in_a_hurry_and_the_others_wait_for_a_busy_app() {
        for event in [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "preToolUse",
            "postToolUse",
            "BeforeTool",
            "AfterTool",
        ] {
            assert_eq!(hook_budget(event), (HOOK_BUDGET, STDIN_BUDGET), "{event}");
        }
        for event in [
            "Stop",
            "StopFailure",
            "stop",
            "agentStop",
            "AfterAgent",
            "notify",
            "Interrupt",
            "Notification",
            "PermissionRequest",
            "UserPromptSubmit",
            "SessionStart",
            "SessionEnd",
        ] {
            assert_eq!(
                hook_budget(event),
                (PATIENT_HOOK_BUDGET, PATIENT_STDIN_BUDGET),
                "{event}"
            );
        }
        // Never as long as a harness waits for a hook before calling it failed.
        assert!(PATIENT_HOOK_BUDGET < Duration::from_secs(5));
    }

    #[test]
    fn hook_payloads_become_json() {
        assert_eq!(parse_payload(""), serde_json::Value::Null);
        assert_eq!(parse_payload("  \n"), serde_json::Value::Null);
        assert_eq!(
            parse_payload(r#"{"hook_event_name":"Stop"}"#),
            serde_json::json!({ "hook_event_name": "Stop" })
        );
        assert_eq!(
            parse_payload("not json"),
            serde_json::Value::String("not json".into())
        );
    }

    /// Run one command line against `socket` as pane `p1`, with `stdin`.
    #[cfg(unix)]
    fn client(socket: &Path, line: &str, stdin: &str) -> (i32, String, String) {
        let env = Env {
            socket: socket.to_path_buf(),
            pane: Some("p1".into()),
            session: Some("s1".into()),
            cwd: "/work".into(),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let status = run_with(
            &args(line),
            &env,
            Input::Text(stdin.to_owned()),
            &mut out,
            &mut err,
        );
        (
            status,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_with_no_app_is_fast_silent_and_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let started = std::time::Instant::now();
        let (status, out, err) =
            client(&socket, "hook claude PreToolUse", r#"{"tool_name":"Bash"}"#);
        assert_eq!(status, EXIT_OK);
        assert!(out.is_empty() && err.is_empty(), "{out:?} {err:?}");
        assert!(started.elapsed() < Duration::from_millis(200));

        // A line the client cannot parse is the harness's business, not an error.
        let (status, out, err) = client(&socket, "hook claude", "");
        assert_eq!((status, out.as_str(), err.as_str()), (EXIT_OK, "", ""));
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_does_not_wait_for_the_apps_answer() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("kou.sock");
        let (_server, incoming) = crate::Server::bind(&socket).unwrap();
        let started = std::time::Instant::now();
        let (status, out, err) = client(&socket, "hook codex Stop", "");
        // The request sits unanswered in `incoming` the whole time, and the
        // hook is long gone: a turn's end has two seconds, and needs none.
        assert_eq!((status, out.as_str(), err.as_str()), (EXIT_OK, "", ""));
        assert!(started.elapsed() < Duration::from_millis(400));
        let request = incoming.recv_blocking().unwrap().request;
        assert_eq!(
            request,
            Request::Hook {
                pane: "p1".into(),
                harness: "codex".into(),
                event: "Stop".into(),
                payload: serde_json::Value::Null,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn commands_reach_the_app_and_report_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("kou.sock");
        let (server, incoming) = crate::Server::bind(&socket).unwrap();
        let pane = |id: &str, activity| Pane {
            id: id.into(),
            session: "s1".into(),
            harness: "codex".into(),
            title: "Review".into(),
            cwd: "/work".into(),
            activity,
            ..Pane::default()
        };
        let app = thread::spawn(move || {
            let mut seen = Vec::new();
            while let Ok(Incoming { request, reply }) = incoming.recv_blocking() {
                reply.send(match &request {
                    Request::Ping => Response::Pong {
                        version: crate::VERSION,
                    },
                    Request::Hook { .. } | Request::PaneSend { .. } => Response::Done,
                    Request::PaneSpawn(_) => Response::Pane(pane("p2", Activity::Working)),
                    Request::PaneRead { .. } => Response::Text {
                        text: "$ cargo test".into(),
                    },
                    Request::PaneWait { panes, .. } => Response::Waited {
                        panes: vec![pane(&panes[0], Activity::Working)],
                        timed_out: true,
                    },
                    Request::PaneList { .. } => Response::Panes {
                        panes: vec![pane("p1", Activity::Idle), pane("p2", Activity::Working)],
                    },
                    _ => Response::Error {
                        message: "no such pane".into(),
                    },
                });
                seen.push(request);
            }
            seen
        });
        use crate::Incoming;

        assert_eq!(
            client(&socket, "ping", ""),
            (EXIT_OK, "pong (protocol 1)\n".into(), String::new())
        );
        assert_eq!(
            client(&socket, "ping --json", ""),
            (
                EXIT_OK,
                "{\"ok\":\"pong\",\"version\":1}\n".into(),
                String::new()
            )
        );
        let (status, out, _) = client(&socket, "pane spawn codex --prompt review", "");
        assert_eq!(status, EXIT_OK);
        assert!(out.starts_with("p2\tworking\tcodex\t"), "{out}");
        assert_eq!(
            client(&socket, "pane send p2 -", "carry on"),
            (EXIT_OK, "ok\n".into(), String::new())
        );
        assert_eq!(
            client(&socket, "pane read p2 --lines 5", ""),
            (EXIT_OK, "$ cargo test\n".into(), String::new())
        );
        let (status, out, _) = client(&socket, "pane list", "");
        assert_eq!((status, out.lines().count()), (EXIT_OK, 2));
        let (status, out, err) = client(&socket, "pane wait p2 --timeout 0.01", "");
        assert_eq!(status, EXIT_TIMED_OUT);
        assert!(out.starts_with("p2\tworking"), "{out}");
        assert!(err.contains("timed out"), "{err}");
        let (status, out, err) = client(&socket, "pane close p9", "");
        assert_eq!((status, out.as_str()), (EXIT_ERROR, ""));
        assert_eq!(err, "kou-desktop: no such pane\n");
        let (status, out, _) = client(&socket, "pane close p9 --json", "");
        assert_eq!(status, EXIT_ERROR);
        assert_eq!(out, "{\"ok\":\"error\",\"message\":\"no such pane\"}\n");
        // A hook is answered, and still says nothing.
        assert_eq!(
            client(&socket, "hook claude Stop", r#"{"hook_event_name":"Stop"}"#),
            (EXIT_OK, String::new(), String::new())
        );

        drop(server);
        let (status, out, err) = client(&socket, "ping", "");
        assert_eq!((status, out.as_str()), (EXIT_NOT_RUNNING, ""));
        assert!(err.contains("not running"), "{err}");

        let seen = app.join().unwrap();
        assert!(seen.contains(&Request::PaneSpawn(Spawn {
            from: Some("p1".into()),
            harness: "codex".into(),
            prompt: Some("review".into()),
            ..Spawn::default()
        })));
        assert!(seen.contains(&Request::Hook {
            pane: "p1".into(),
            harness: "claude".into(),
            event: "Stop".into(),
            payload: serde_json::json!({ "hook_event_name": "Stop" }),
        }));
    }

    #[test]
    fn panes_and_harnesses_print_on_one_line() {
        let pane = Pane {
            id: "p3".into(),
            session: "s1".into(),
            harness: "claude".into(),
            title: "Fix the\nbuild".into(),
            cwd: "/work/api".into(),
            kind: PaneKind::Thread,
            activity: Activity::Blocked,
            parent: Some("p1".into()),
            since_ms: 125_000,
        };
        assert_eq!(
            pane_line(&pane),
            "p3\tblocked\tclaude\tthread\t2m\t/work/api\tFix the build"
        );
        let harness = Harness {
            id: "codex".into(),
            name: "Codex".into(),
            version: None,
            ready: false,
            message: Some("Run `codex login`.".into()),
            native: false,
        };
        assert_eq!(
            harness_line(&harness),
            "codex\tunavailable\t-\tterminal\tCodex\tRun `codex login`."
        );
    }

    /// The tab `t1` of `s1`, named and at the front, with two panes.
    fn named_tab() -> Tab {
        Tab {
            id: "t1".into(),
            session: "s1".into(),
            name: "api\n server".into(),
            named: true,
            icon: Some("terminal".into()),
            layout: Layout::Canvas,
            panes: vec!["p1".into(), "p3".into()],
            active: "p3".into(),
            front: true,
        }
    }

    fn session(id: &str, selected: bool, archived: bool) -> Session {
        Session {
            id: id.into(),
            space: "w1".into(),
            name: format!("Session {id}"),
            cwd: "/work".into(),
            archived,
            tabs: 2,
            selected,
        }
    }

    fn space() -> Space {
        Space {
            id: "w1".into(),
            name: "Client\twork".into(),
            path: "/work".into(),
            sessions: 3,
        }
    }

    #[test]
    fn tabs_sessions_and_spaces_print_on_one_line() {
        assert_eq!(
            tab_line(&named_tab()),
            "t1\ts1\tfront\tcanvas\tterminal\tp1,p3\tapi server"
        );
        let plain = Tab {
            id: "t2".into(),
            session: "s1".into(),
            name: "codex".into(),
            panes: vec!["p2".into()],
            active: "p2".into(),
            ..Tab::default()
        };
        assert_eq!(tab_line(&plain), "t2\ts1\t-\tpanes\t-\tp2\tcodex");
        assert_eq!(
            session_line(&session("s1", true, false)),
            "s1\tw1\tselected\t2\t/work\tSession s1"
        );
        assert_eq!(
            session_line(&session("s2", false, true)),
            "s2\tw1\tarchived\t2\t/work\tSession s2"
        );
        assert_eq!(
            session_line(&session("s3", false, false)),
            "s3\tw1\t-\t2\t/work\tSession s3"
        );
        assert_eq!(space_line(&space()), "w1\t3\t/work\tClient work");
    }

    #[cfg(unix)]
    #[test]
    fn tabs_sessions_and_spaces_reach_the_app_and_print_its_answer() {
        use crate::Incoming;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("kou.sock");
        let (server, incoming) = crate::Server::bind(&socket).unwrap();
        let tabs = Response::Tabs {
            tabs: vec![
                named_tab(),
                Tab {
                    id: "t2".into(),
                    session: "s1".into(),
                    name: "codex".into(),
                    panes: vec!["p2".into()],
                    ..Tab::default()
                },
            ],
        };
        let sessions = Response::Sessions {
            sessions: vec![session("s1", true, false), session("s2", false, true)],
        };
        let spaces = Response::Spaces {
            spaces: vec![space()],
        };
        let answers = (tabs.clone(), sessions.clone(), spaces.clone());
        let app = thread::spawn(move || {
            let (tabs, sessions, spaces) = answers;
            let mut seen = Vec::new();
            while let Ok(Incoming { request, reply }) = incoming.recv_blocking() {
                reply.send(match &request {
                    Request::TabList { .. } => tabs.clone(),
                    Request::TabSet { .. } => Response::Tab(named_tab()),
                    Request::TabIcons => Response::Text {
                        text: "auto\nterminal\nclaude".into(),
                    },
                    Request::SessionList { .. } => sessions.clone(),
                    Request::SessionRename { .. } => Response::Session(session("s1", true, false)),
                    Request::SpaceList => spaces.clone(),
                    Request::SpaceRename { .. } => Response::Space(space()),
                    _ => Response::Error {
                        message: "unexpected".into(),
                    },
                });
                seen.push(request);
            }
            seen
        });
        let ok = |out: &str| (EXIT_OK, out.to_owned(), String::new());
        let raw =
            |response: &Response| ok(&format!("{}\n", serde_json::to_string(response).unwrap()));

        assert_eq!(
            client(&socket, "tab list", ""),
            ok("t1\ts1\tfront\tcanvas\tterminal\tp1,p3\tapi server\n\
                t2\ts1\t-\tpanes\t-\tp2\tcodex\n")
        );
        assert_eq!(client(&socket, "tab list --all --json", ""), raw(&tabs));
        assert_eq!(
            client(&socket, "tab rename api server", ""),
            ok("t1\ts1\tfront\tcanvas\tterminal\tp1,p3\tapi server\n")
        );
        assert_eq!(
            client(&socket, "tab icon --tab t1 terminal --json", ""),
            raw(&Response::Tab(named_tab()))
        );
        assert_eq!(
            client(&socket, "tab icons", ""),
            ok("auto\nterminal\nclaude\n")
        );
        assert_eq!(
            client(&socket, "session list", ""),
            ok("s1\tw1\tselected\t2\t/work\tSession s1\n\
                s2\tw1\tarchived\t2\t/work\tSession s2\n")
        );
        assert_eq!(client(&socket, "session list --json", ""), raw(&sessions));
        assert_eq!(
            client(&socket, "session rename Session s1", ""),
            ok("s1\tw1\tselected\t2\t/work\tSession s1\n")
        );
        assert_eq!(
            client(&socket, "space list", ""),
            ok("w1\t3\t/work\tClient work\n")
        );
        assert_eq!(client(&socket, "space list --json", ""), raw(&spaces));
        assert_eq!(
            client(&socket, "space rename --space w1 Client work --json", ""),
            raw(&Response::Space(space()))
        );

        drop(server);
        let seen = app.join().unwrap();
        assert_eq!(
            seen,
            [
                Request::TabList {
                    session: Some("s1".into())
                },
                Request::TabList { session: None },
                Request::TabSet {
                    tab: None,
                    pane: Some("p1".into()),
                    name: Some("api server".into()),
                    icon: None,
                },
                Request::TabSet {
                    tab: Some("t1".into()),
                    pane: Some("p1".into()),
                    name: None,
                    icon: Some("terminal".into()),
                },
                Request::TabIcons,
                Request::SessionList { space: None },
                Request::SessionList { space: None },
                Request::SessionRename {
                    session: Some("s1".into()),
                    pane: Some("p1".into()),
                    name: "Session s1".into(),
                },
                Request::SpaceList,
                Request::SpaceList,
                Request::SpaceRename {
                    space: Some("w1".into()),
                    pane: Some("p1".into()),
                    name: "Client work".into(),
                },
            ]
        );
    }
}
