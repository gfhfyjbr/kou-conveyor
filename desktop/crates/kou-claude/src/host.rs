//! A keeper for a Claude Code that is to outlive the app: a small process of
//! its own that starts `claude`, owns its standard streams, and lets one
//! client at a time talk to it over a socket. The app closing only drops the
//! socket; Claude Code works on, and the app finds it again when it is back.
//!
//! The host is a daemon ([`run`]): its own session, not a child of the app.
//! The app reaches it through [`Keep`]: it attaches to the host of a socket,
//! or starts one and attaches.
//!
//! # The wire
//!
//! Lines, each ended by `\n`, in UTF-8.
//!
//! - **Client to host:** a line that begins with `{` goes to the program's
//!   standard input as it is. `#stop` ends the program and its process group
//!   (`SIGTERM`, then `SIGKILL` two seconds later), and the host with it.
//!   Anything else is ignored. Closing the connection is leaving: the
//!   program runs on.
//! - **Host to client:** first, once, `#hello <json>` ([`Hello`]). Then each
//!   line the program writes to its standard output that begins with `{`, as
//!   it is; `#noise <text>` for one that does not; `#stderr <text>` for each
//!   line of its standard error; and `#exit <json>` ([`Exit`]) when the
//!   program has ended, after which the host closes the connection and is
//!   gone.
//!
//! One client at a time: a new connection takes the place of the one before
//! it, which is told so (`#replaced`) and from then on not listened to; a
//! client that takes nothing of what is written to it for a minute
//! ([`STALL`]) is dropped. The program is never held up by a client. While
//! nobody is connected, what the program writes is looked at and dropped.
//!
//! Beside the socket, `<socket>.last` says why the last host there ended:
//! how its program ended and the last lines of its standard error ([`Last`]).
//! And in the same folder, `keepers.log` has a line for each thing that
//! befalls a host there (it starts, a client comes, leaves, is replaced or
//! dropped, the program ends), from the hosts and from their clients
//! ([`note`]): what tells afterwards why a Claude Code went away.

mod client;
#[cfg(unix)]
mod daemon;
#[cfg(unix)]
mod memory;

pub use client::{Attached, Client, Hello, Keep, Received};

/// The Codex app-server connection already initialized in this keeper.
/// The app server supplies the conversation history on reattachment.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CodexState {
    pub initialized: bool,
    pub thread: Option<String>,
    pub turn: Option<String>,
    /// Notifications for the current turn up to the instant of attachment.
    /// Codex's disk history can omit text that is still streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<serde_json::Value>>,
}

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

/// The version of the wire, said in `#hello`.
#[cfg(unix)]
const VERSION: u32 = 1;

/// The words of the wire that are not the program's own lines.
#[cfg(unix)]
const HELLO: &str = "#hello";
#[cfg(unix)]
const NOISE: &str = "#noise";
#[cfg(unix)]
const STDERR: &str = "#stderr";
#[cfg(unix)]
const EXIT: &str = "#exit";
#[cfg(unix)]
const STOP: &str = "#stop";
/// Said to a client whose place another has taken, last.
#[cfg(unix)]
const REPLACED: &str = "#replaced";

/// How long a client may take nothing of what is written to it before it is
/// dropped, unless the host is told otherwise. Long: a client that is slow
/// for a while (an app under load) is not one that is gone, and the host
/// is not held up by it meanwhile.
pub const STALL: Duration = Duration::from_secs(60);

/// The log of the hosts of a folder, beside their sockets.
pub const LOG: &str = "keepers.log";
/// How big the log grows before it is moved aside (to `keepers.log.old`)
/// and begun anew.
const LOG_SIZE: u64 = 1 << 20;

/// How long a host stays with nobody connected and nothing to do, unless
/// told otherwise.
const IDLE: Duration = Duration::from_secs(600);
/// How long a host stays with nobody connected, whatever its program does,
/// unless told otherwise.
const AWAY: Duration = Duration::from_secs(86_400);

/// How a program ended: the code it returned, or the signal that killed it.
/// Neither is known when the system would not say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
}

impl std::fmt::Display for Exit {
    /// For a sentence: `exited with code 1`, `was killed by signal 9`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (Some(code), _) => write!(f, "exited with code {code}"),
            (None, Some(signal)) => write!(f, "was killed by signal {signal}"),
            (None, None) => write!(f, "ended in a way the system would not say"),
        }
    }
}

/// Why a host ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// Its program ended by itself.
    Exited,
    /// A client sent `#stop`.
    Stopped,
    /// The host was sent `SIGTERM`.
    Terminated,
    /// Nobody was connected for its idle time, and its program had nothing
    /// to do.
    Idle,
    /// Nobody was connected for its away time.
    Away,
    /// A reason this build does not know, from a later one.
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for Reason {
    /// For a sentence: why the host ended.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reason::Exited => "its program ended",
            Reason::Stopped => "it was asked to stop",
            Reason::Terminated => "it was sent SIGTERM",
            Reason::Idle => "nobody came back to it, and nothing was going on",
            Reason::Away => "nobody came back to it for a day",
            Reason::Unknown => "for a reason this build does not know",
        })
    }
}

/// What `<socket>.last` says of the host that was there: the app reads it
/// to say why a Claude Code it left running is gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Last {
    pub reason: Reason,
    pub exit: Exit,
    /// The last lines of the program's standard error, the latest last.
    #[serde(default)]
    pub stderr: Vec<String>,
    /// When the host ended, in seconds since the epoch.
    #[serde(default)]
    pub ended: u64,
}

/// Where the host of `socket` leaves word of how it ended.
fn last_path(socket: &Path) -> PathBuf {
    let mut path: OsString = socket.as_os_str().to_owned();
    path.push(".last");
    PathBuf::from(path)
}

/// What the last host at `socket` left word of when it ended, if one did
/// and no host has started there since.
pub fn last(socket: &Path) -> Option<Last> {
    let text = std::fs::read_to_string(last_path(socket)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Add a line to the log of the hosts beside `socket` ([`LOG`]): when,
/// which socket, who says it (`host`, `app`, with the process's id), and
/// `text`. Nothing is said if it cannot be written: the log is for looking
/// back, never in the way.
pub fn note(socket: &Path, who: &str, text: &str) {
    let Some(folder) = socket.parent() else {
        return;
    };
    let log = folder.join(LOG);
    if std::fs::metadata(&log).is_ok_and(|metadata| metadata.len() > LOG_SIZE) {
        let mut old = log.clone().into_os_string();
        old.push(".old");
        let _ = std::fs::rename(&log, old);
    }
    let name = socket
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let line = format!(
        "{} {name} {who} {}: {}\n",
        utc(seconds_now()),
        std::process::id(),
        text.replace('\n', " / ")
    );
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    // One write of the whole line: lines of several processes do not mix.
    if let Ok(mut file) = options.open(&log) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `seconds` since the epoch as `2026-11-16T11:06:30Z`.
fn utc(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm), from 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// What the host is started with.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Clone, Debug, PartialEq)]
struct Arguments {
    socket: PathBuf,
    cwd: PathBuf,
    idle: Duration,
    away: Duration,
    stall: Duration,
    program: PathBuf,
    args: Vec<String>,
}

impl Arguments {
    /// `--socket <path> --cwd <dir> [--idle-secs <n>] [--away-secs <n>]
    /// [--stall-secs <n>] -- <program> <its arguments…>`
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut socket = None;
        let mut cwd = None;
        let mut idle = IDLE;
        let mut away = AWAY;
        let mut stall = STALL;
        let mut words = arguments.iter();
        loop {
            let Some(word) = words.next() else {
                return Err("no program is given (after `--`)".into());
            };
            if word == "--" {
                break;
            }
            let mut value = || {
                words
                    .next()
                    .ok_or_else(|| format!("{word} wants a value after it"))
            };
            match word.as_str() {
                "--socket" => socket = Some(PathBuf::from(value()?)),
                "--cwd" => cwd = Some(PathBuf::from(value()?)),
                "--idle-secs" => idle = seconds(word, value()?)?,
                "--away-secs" => away = seconds(word, value()?)?,
                "--stall-secs" => stall = seconds(word, value()?)?,
                _ => return Err(format!("{word} is not an option of the host")),
            }
        }
        let program = words.next().ok_or("no program is given after `--`")?.into();
        Ok(Self {
            socket: socket.ok_or("the socket is not given (--socket)")?,
            cwd: cwd.ok_or("the program's folder is not given (--cwd)")?,
            idle,
            away,
            stall,
            program,
            args: words.cloned().collect(),
        })
    }

    /// The command line that [`Arguments::parse`] reads back as `self`.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn to_words(&self) -> Vec<OsString> {
        let mut words: Vec<OsString> = vec![
            "--socket".into(),
            self.socket.clone().into(),
            "--cwd".into(),
            self.cwd.clone().into(),
            "--idle-secs".into(),
            self.idle.as_secs_f64().to_string().into(),
            "--away-secs".into(),
            self.away.as_secs_f64().to_string().into(),
            "--stall-secs".into(),
            self.stall.as_secs_f64().to_string().into(),
            "--".into(),
            self.program.clone().into(),
        ];
        words.extend(self.args.iter().map(OsString::from));
        words
    }
}

fn seconds(option: &str, value: &str) -> Result<Duration, String> {
    value
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| format!("{option} wants a number of seconds, got {value:?}"))
}

/// The host's entry point, given the words of its command line after the
/// program's own leading ones:
///
/// ```text
/// --socket <path> --cwd <dir> [--idle-secs <n>] [--away-secs <n>] [--stall-secs <n>] -- <program> <its arguments…>
/// ```
///
/// It turns itself into a daemon, starts `<program>` in `<dir>` with the
/// environment it was given, and serves clients at `<path>`. In the process
/// that called it, it returns at once: 0, or 2 for a command line it cannot
/// read. The daemon never returns: it exits when its program has ended.
/// Whatever stops the daemon from starting (a host already at the socket, a
/// program that cannot be started) is said on standard error, which it
/// keeps open until it listens, so that whoever started it can wait for
/// that pipe to close and read why.
///
/// It must be called before the process has started any thread (see the
/// daemon's `fork`).
pub fn run(arguments: &[String]) -> i32 {
    let arguments = match Arguments::parse(arguments) {
        Ok(arguments) => arguments,
        Err(why) => {
            let _ = writeln!(std::io::stderr(), "{why}");
            return 2;
        }
    };
    #[cfg(unix)]
    return daemon::run(arguments);
    #[cfg(not(unix))]
    {
        let _ = arguments;
        let _ = writeln!(
            std::io::stderr(),
            "keeping Claude Code apart from the app needs a Unix system"
        );
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn the_command_line_names_the_socket_the_folder_the_times_and_the_program() {
        let parsed = Arguments::parse(&words(
            "--socket /tmp/s --cwd /w --idle-secs 1.5 -- claude --resume=x --",
        ))
        .unwrap();
        let expected = Arguments {
            socket: "/tmp/s".into(),
            cwd: "/w".into(),
            idle: Duration::from_millis(1500),
            away: AWAY,
            stall: STALL,
            program: "claude".into(),
            args: vec!["--resume=x".into(), "--".into()],
        };
        assert_eq!(parsed, expected);
        let again: Vec<String> = expected
            .to_words()
            .into_iter()
            .map(|word| word.into_string().unwrap())
            .collect();
        assert_eq!(Arguments::parse(&again).unwrap(), expected);
    }

    #[test]
    fn a_command_line_that_lacks_something_says_what() {
        let error = |text: &str| Arguments::parse(&words(text)).unwrap_err();
        assert_eq!(
            error("--cwd /w -- claude"),
            "the socket is not given (--socket)"
        );
        assert_eq!(
            error("--socket /s --cwd /w"),
            "no program is given (after `--`)"
        );
        assert_eq!(
            error("--socket /s --cwd /w --"),
            "no program is given after `--`"
        );
        assert_eq!(
            error("--socket /s --cwd /w --idle-secs soon -- c"),
            "--idle-secs wants a number of seconds, got \"soon\""
        );
        assert_eq!(
            error("--socket /s --cwd /w --fast -- c"),
            "--fast is not an option of the host"
        );
        assert_eq!(error("--socket"), "--socket wants a value after it");
    }

    #[test]
    fn an_exit_is_said_as_a_code_or_a_signal() {
        let code = Exit {
            code: Some(3),
            signal: None,
        };
        let signal = Exit {
            code: None,
            signal: Some(15),
        };
        assert_eq!(serde_json::to_string(&code).unwrap(), r#"{"code":3}"#);
        assert_eq!(serde_json::to_string(&signal).unwrap(), r#"{"signal":15}"#);
        assert_eq!(serde_json::to_string(&Exit::default()).unwrap(), "{}");
        assert_eq!(code.to_string(), "exited with code 3");
        assert_eq!(signal.to_string(), "was killed by signal 15");
        let last: Last = serde_json::from_str(r#"{"reason":"later","exit":{"code":0}}"#).unwrap();
        assert_eq!(last.reason, Reason::Unknown);
        assert!(last.stderr.is_empty());
    }

    #[test]
    fn a_time_is_said_in_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_794_827_190), "2026-11-16T11:06:30Z");
    }

    #[test]
    fn the_log_beside_the_sockets_gets_a_line_for_each_note_and_is_moved_aside_when_big() {
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("p1-ab-1.sock");
        note(&socket, "host", "started claude\nwith two lines");
        note(&socket, "app", "left");
        let text = std::fs::read_to_string(folder.path().join(LOG)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let pid = std::process::id();
        assert!(
            lines[0].ends_with(&format!(
                " p1-ab-1.sock host {pid}: started claude / with two lines"
            )),
            "{}",
            lines[0]
        );
        assert!(lines[1].ends_with(&format!(" p1-ab-1.sock app {pid}: left")));
        std::fs::write(folder.path().join(LOG), vec![b'x'; LOG_SIZE as usize + 1]).unwrap();
        note(&socket, "app", "anew");
        let text = std::fs::read_to_string(folder.path().join(LOG)).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(folder.path().join("keepers.log.old").exists());
    }
}
