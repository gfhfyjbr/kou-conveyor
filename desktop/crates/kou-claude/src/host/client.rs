//! The app's end of a host: attach to the one that keeps a Claude Code at a
//! socket, or start one and attach; write lines to the program and receive
//! what the host relays, on a channel filled by a thread of the client's
//! own.

use super::{Exit, Last};
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
#[cfg(unix)]
use {
    super::{Arguments, EXIT, HELLO, NOISE, REPLACED, STDERR, STOP, VERSION},
    anyhow::Context as _,
    std::{
        ffi::OsString,
        io::{self, BufRead, BufReader, Read, Write},
        net::Shutdown,
        os::unix::net::UnixStream,
        process::{Command, Stdio},
        sync::{Mutex, PoisonError},
        thread,
        time::Instant,
    },
};

/// How long a host gets to say hello once connected.
#[cfg(unix)]
const HELLO_PATIENCE: Duration = Duration::from_secs(5);
/// How long a host gets to take a line written to it.
#[cfg(unix)]
const WRITE_PATIENCE: Duration = Duration::from_secs(5);
/// How long a host that is started gets to come up.
#[cfg(unix)]
const START_PATIENCE: Duration = Duration::from_secs(10);
/// How long a started host's word is waited for alone (it says why it does
/// not start, or closes its standard error once it listens), before its
/// socket is tried as well.
#[cfg(unix)]
const QUIET_START: Duration = Duration::from_secs(2);
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(100);

/// The variables Claude Code sets for what it starts, which would make it
/// take itself for a child of another Claude Code. The same list as the
/// one `live::link` strips, which is out of this module's reach.
#[cfg(unix)]
const NOT_PASSED_ON: [&str; 3] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
];

/// Where and how a Claude Code is kept.
///
/// The socket's path is the caller's to choose. A Unix socket's path must
/// fit in about a hundred bytes (104 on macOS, 108 on Linux), so it belongs
/// in a folder with a short path, not deep under a home.
#[derive(Clone, Debug)]
pub struct Keep {
    /// The program that is the host, and the words before the host's own
    /// arguments (`["claude-host"]` for the app's binary; none for the
    /// example).
    pub host: PathBuf,
    pub host_args: Vec<String>,
    /// The socket.
    pub socket: PathBuf,
    /// How long the host stays with nobody connected while its program has
    /// nothing to do.
    pub idle: Duration,
    /// How long it stays with nobody connected, whatever its program does.
    pub away: Duration,
    /// How long it waits for a client that takes nothing of what is written
    /// to it, before it drops that client ([`super::STALL`] as a rule).
    pub stall: Duration,
}

/// What a host says first: how things stand. The lines in it are the
/// program's own, whole.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Hello {
    /// The version of the wire.
    pub version: u32,
    /// The program's process id.
    pub pid: u32,
    /// When the program was started, in seconds since the epoch.
    pub started: u64,
    /// This is the first client the host has had.
    pub fresh: bool,
    #[serde(default)]
    pub codex: Option<super::CodexState>,
    /// The program has written a `result` since the client last wrote a
    /// `user` line and since its state last changed: the model has given
    /// its answer, and what still runs runs in the background.
    #[serde(default)]
    pub answered: bool,
    /// The program's `control_response` to the client's `initialize`. A
    /// client that finds one does not greet again.
    #[serde(default)]
    pub greeting: Option<Value>,
    /// The latest `system` lines of subtype `init`, `status` and
    /// `session_state_changed`.
    #[serde(default)]
    pub init: Option<Value>,
    #[serde(default)]
    pub status: Option<Value>,
    #[serde(default)]
    pub state: Option<Value>,
    /// The program's `control_request`s still unanswered, in the order they
    /// came.
    #[serde(default)]
    pub pending: Vec<Value>,
}

/// What a host relays, after its hello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    /// A line of the program's standard output that is a JSON object, as it
    /// was written.
    Line(String),
    /// A line of its standard output that is not.
    Noise(String),
    /// A line of its standard error.
    Stderr(String),
    /// The program has ended; the host is gone.
    Exit(Exit),
    /// Another client has taken this one's place. The program runs on, and
    /// nothing written to the host from now on reaches it.
    Replaced,
    /// The connection is over: after [`Received::Exit`] or
    /// [`Received::Replaced`], or because the host dropped this one for not
    /// taking what it wrote, or the host died. Nothing comes after it.
    Closed,
}

impl Received {
    /// What a line of the host says; `None` for a word this build does not
    /// know.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn parse(line: &str) -> Option<Self> {
        if line.starts_with('{') {
            return Some(Self::Line(line.to_owned()));
        }
        #[cfg(unix)]
        {
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
            match word {
                NOISE => return Some(Self::Noise(rest.to_owned())),
                STDERR => return Some(Self::Stderr(rest.to_owned())),
                EXIT => return Some(Self::Exit(serde_json::from_str(rest).unwrap_or_default())),
                REPLACED => return Some(Self::Replaced),
                _ => {}
            }
        }
        None
    }
}

/// A host attached to: the client, how things stood when it came, and what
/// the host relays from then on.
pub struct Attached {
    pub client: Client,
    pub hello: Hello,
    pub received: mpsc::Receiver<Received>,
}

/// The writing end of a connection to a host. Dropping it is leaving: the
/// program runs on.
pub struct Client {
    #[cfg(unix)]
    stream: Mutex<UnixStream>,
    #[cfg(not(unix))]
    never: std::convert::Infallible,
}

impl Keep {
    /// Attach to the host that listens at the socket, if one does. `None`
    /// when nothing listens there; a stale socket file is left to the host
    /// that starts next.
    #[cfg(unix)]
    pub fn attach(&self) -> anyhow::Result<Option<Attached>> {
        let place = self.socket.display();
        let stream = match UnixStream::connect(&self.socket) {
            Ok(stream) => stream,
            Err(error) if gone(&error) => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not reach the keeper at {place}"));
            }
        };
        stream.set_read_timeout(Some(HELLO_PATIENCE))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut first = Vec::new();
        match reader.read_until(b'\n', &mut first) {
            Ok(_) if !first.ends_with(b"\n") => return Ok(None),
            Ok(_) => {}
            Err(error) if gone(&error) => return Ok(None),
            Err(error) if timed_out(&error) => anyhow::bail!(
                "the keeper at {place} did not answer within {} seconds",
                HELLO_PATIENCE.as_secs()
            ),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not hear from the keeper at {place}"));
            }
        }
        let first = String::from_utf8_lossy(&first);
        let first = first.trim_end_matches(['\n', '\r']);
        let hello = first
            .strip_prefix(HELLO)
            .and_then(|rest| rest.strip_prefix(' '))
            .and_then(|json| serde_json::from_str::<Hello>(json).ok());
        let Some(hello) = hello else {
            anyhow::bail!("what listens at {place} is not a keeper of Claude Code");
        };
        anyhow::ensure!(
            hello.version == VERSION,
            "the keeper at {place} speaks version {} of its wire, and this app {VERSION}",
            hello.version
        );
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(Some(WRITE_PATIENCE))?;
        let (sender, received) = mpsc::channel();
        thread::Builder::new()
            .name("kou-claude host".into())
            .spawn(move || read_host(reader, &sender))
            .context("could not start the thread that hears the keeper")?;
        Ok(Some(Attached {
            client: Client {
                stream: Mutex::new(stream),
            },
            hello,
            received,
        }))
    }

    /// Start a host for `program` with `args` in `cwd`, wait until it
    /// listens, and attach to it. The program's environment is the app's,
    /// without the marks of a Claude Code session the app may run in, with
    /// `env` over it. A host that does not come up is an error that says
    /// why, in its own words.
    #[cfg(unix)]
    pub fn start(
        &self,
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
    ) -> anyhow::Result<Attached> {
        let place = self.socket.display();
        let arguments = Arguments {
            socket: self.socket.clone(),
            cwd: cwd.to_owned(),
            idle: self.idle,
            away: self.away,
            stall: self.stall,
            program: program.to_owned(),
            args: args.to_vec(),
        };
        let mut command = Command::new(&self.host);
        command
            .args(&self.host_args)
            .args(arguments.to_words())
            .env_clear()
            .envs(environment(std::env::vars_os(), env))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start the keeper {}", self.host.display()))?;
        // The process started returns at once. The daemon it forks keeps
        // this pipe until it listens, or says why it cannot and ends: the
        // pipe closing is the sign either way.
        let stderr = child.stderr.take();
        let (sender, said) = mpsc::channel();
        let reading = thread::Builder::new()
            .name("kou-claude host start".into())
            .spawn(move || {
                let mut text = Vec::new();
                if let Some(mut stderr) = stderr {
                    let _ = stderr.read_to_end(&mut text);
                }
                let _ = sender.send(String::from_utf8_lossy(&text).trim().to_owned());
            });
        let status = child.wait();
        reading.context("could not start the thread that hears the keeper start")?;
        let status = status.context("could not wait for the keeper to start")?;
        let begun = Instant::now();
        loop {
            let alone = QUIET_START.saturating_sub(begun.elapsed());
            match said.recv_timeout(if alone.is_zero() { POLL } else { alone }) {
                Ok(said) if said.is_empty() => break,
                Ok(said) => anyhow::bail!("the keeper at {place} did not start: {said}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // The pipe can be held open by another child of the app, which
            // got it in the system's race (see the daemon's
            // `close_inherited`): a host that listens is up all the same.
            if begun.elapsed() >= QUIET_START
                && let Some(attached) = self.attach()?
            {
                return Ok(attached);
            }
            anyhow::ensure!(
                begun.elapsed() < START_PATIENCE,
                "the keeper at {place} did not come up within {} seconds",
                START_PATIENCE.as_secs()
            );
        }
        anyhow::ensure!(
            status.success(),
            "the keeper at {place} did not start: it {}",
            Exit::from(status)
        );
        if let Some(attached) = self.attach()? {
            return Ok(attached);
        }
        // Gone already: the program ended at once.
        match self.last() {
            Some(last) => {
                let mut why = format!("{} {} at once", program.display(), last.exit);
                if !last.stderr.is_empty() {
                    why.push_str(":\n");
                    why.push_str(&last.stderr.join("\n"));
                }
                Err(anyhow::anyhow!(why))
            }
            None => anyhow::bail!("the keeper at {place} started but does not listen"),
        }
    }

    #[cfg(not(unix))]
    pub fn attach(&self) -> anyhow::Result<Option<Attached>> {
        Ok(None)
    }

    #[cfg(not(unix))]
    pub fn start(
        &self,
        _program: &Path,
        _args: &[String],
        _cwd: &Path,
        _env: &[(String, String)],
    ) -> anyhow::Result<Attached> {
        anyhow::bail!("keeping Claude Code apart from the app needs a Unix system")
    }

    /// What the last host at the socket left word of when it ended, if one
    /// did and no host has started there since.
    pub fn last(&self) -> Option<Last> {
        super::last(&self.socket)
    }
}

impl Client {
    /// Write `line`, a JSON object on one line, to the program's standard
    /// input.
    #[cfg(unix)]
    pub fn write(&self, line: &str) -> io::Result<()> {
        if !line.starts_with('{') || line.contains('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a JSON object on one line goes to the program",
            ));
        }
        self.send(line)
    }

    /// Ask the host to end the program and whatever it started, and itself.
    /// What the program says until it has ended, and how it ended, still
    /// come; then the connection closes.
    #[cfg(unix)]
    pub fn stop(&self) -> io::Result<()> {
        self.send(STOP)
    }

    #[cfg(unix)]
    fn send(&self, line: &str) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let mut stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        stream.write_all(&bytes)
    }

    #[cfg(not(unix))]
    pub fn write(&self, _line: &str) -> std::io::Result<()> {
        match self.never {}
    }

    #[cfg(not(unix))]
    pub fn stop(&self) -> std::io::Result<()> {
        match self.never {}
    }
}

#[cfg(unix)]
impl Drop for Client {
    fn drop(&mut self) {
        let stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        // Both ways, so that the thread that reads ends too.
        let _ = stream.shutdown(Shutdown::Both);
    }
}

/// Relay what the host says until the connection ends.
#[cfg(unix)]
fn read_host(mut reader: BufReader<UnixStream>, received: &mpsc::Sender<Received>) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        let Some(said) = Received::parse(text.trim_end_matches(['\n', '\r'])) else {
            continue;
        };
        if received.send(said).is_err() {
            // Nobody hears it any more: the host will find this client
            // stalled and drop it.
            return;
        }
    }
    let _ = received.send(Received::Closed);
}

/// The error of a socket nobody is at (any more).
#[cfg(unix)]
fn gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    )
}

#[cfg(unix)]
fn timed_out(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// The environment a host is started with: `inherited` (the app's own)
/// without the marks of a Claude Code session the app may run in, with
/// `added` over it.
#[cfg(unix)]
fn environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    added: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = inherited
        .into_iter()
        .filter(|(name, _)| !NOT_PASSED_ON.iter().any(|left| name == left))
        .collect();
    for (name, value) in added {
        match env.iter_mut().find(|(known, _)| known == name.as_str()) {
            Some(known) => known.1 = value.into(),
            None => env.push((name.into(), value.into())),
        }
    }
    env
}

#[cfg(unix)]
impl From<std::process::ExitStatus> for Exit {
    fn from(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
            signal: std::os::unix::process::ExitStatusExt::signal(&status),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn what_the_host_relays_is_told_apart_by_its_first_word() {
        let parse = |line: &str| Received::parse(line);
        assert_eq!(
            parse(r#"{"type":"result"}"#),
            Some(Received::Line(r#"{"type":"result"}"#.into()))
        );
        assert_eq!(
            parse("#noise plain words"),
            Some(Received::Noise("plain words".into()))
        );
        assert_eq!(parse("#stderr "), Some(Received::Stderr(String::new())));
        assert_eq!(
            parse(r#"#exit {"signal":9}"#),
            Some(Received::Exit(Exit {
                code: None,
                signal: Some(9),
            }))
        );
        assert_eq!(parse("#exit ?"), Some(Received::Exit(Exit::default())));
        assert_eq!(parse("#later something"), None);
        assert_eq!(parse("plain"), None);
    }

    #[test]
    fn the_host_s_environment_is_the_app_s_without_the_marks_of_claude_code_and_with_what_is_added()
    {
        let inherited = [
            ("HOME", "/Users/me"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_SSE_PORT", "4242"),
            ("PATH", "/bin"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let added = [
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("CLAUDE_CONFIG_DIR".to_owned(), "/accounts/two".to_owned()),
        ];
        let env = environment(inherited, &added);
        let pairs: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("HOME", "/Users/me"),
                ("PATH", "/usr/bin"),
                ("CLAUDE_CONFIG_DIR", "/accounts/two"),
            ]
        );
    }
}
