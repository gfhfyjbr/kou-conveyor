//! The runner process: what a build of it can do, the advisory lock that
//! keeps two front-ends from running one session, starting it attached with
//! its output read line by line, stopping it the way the cockpits do, and
//! taking up a run that a cockpit left running apart from itself.

use crate::connection::{Lease, create_private_dir};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead as _, BufReader, Read, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

/// An interrupted runner terminates its tools before it exits: SIGTERM, a
/// five-second grace period, then SIGKILL. Only a runner that overstays that
/// is killed outright.
pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// The runner's file name.
pub(crate) const RUNNER_NAME: &str = if cfg!(windows) {
    "kou-conveyor-runner.exe"
} else {
    "kou-conveyor-runner"
};

/// What a runner can do that runners built before could not, which its usage
/// says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Features {
    /// It takes messages while it runs (`-steer`), which keeps stdin open.
    pub steer: bool,
    /// Its messages link files.
    pub links: bool,
    /// It takes a line that stops it and says why, which it records.
    pub stop: bool,
    /// It writes the model's text as it streams and the output of commands
    /// as they run (`-stream`).
    pub stream: bool,
}

impl Features {
    fn of_usage(usage: &str) -> Self {
        Self {
            steer: usage.contains("-steer"),
            links: usage.contains("files: array of {path"),
            stop: usage.contains(r#"{"stop": {"reason""#),
            stream: usage.contains(r#"{"type": "delta""#),
        }
    }
}

/// A build of the runner: its path, size and time.
type Build = (PathBuf, u64, Option<SystemTime>);

/// Asks a runner what it can do. The answer is kept per build of the file.
pub(crate) fn features_of(runner: &Path) -> Features {
    static KNOWN: OnceLock<Mutex<HashMap<Build, Features>>> = OnceLock::new();
    let Ok(metadata) = std::fs::metadata(runner) else {
        return Features::default();
    };
    let key = (runner.to_owned(), metadata.len(), metadata.modified().ok());
    let known = KNOWN.get_or_init(Mutex::default);
    if let Some(features) = known.lock().ok().and_then(|known| known.get(&key).copied()) {
        return features;
    }
    let Ok(output) = Command::new(runner).arg("-h").stdin(Stdio::null()).output() else {
        return Features::default();
    };
    let mut usage = String::from_utf8_lossy(&output.stdout).into_owned();
    usage.push_str(&String::from_utf8_lossy(&output.stderr));
    let features = Features::of_usage(&usage);
    if let Ok(mut known) = known.lock() {
        known.insert(key, features);
    }
    features
}

/// Where a project keeps its sessions, as the cockpits' default.
pub(crate) fn session_dir(project: &Path) -> PathBuf {
    project.join(".harness").join("sessions")
}

/// The file that holds a session.
pub(crate) fn session_path(session_dir: &Path, id: &str) -> PathBuf {
    session_dir.join(format!("{id}.session.jsonl"))
}

/// Whether the runner's session store accepts the ID.
pub(crate) fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn lock_path(session_dir: &Path, id: &str) -> PathBuf {
    session_dir.join(".locks").join(format!("{id}.lock"))
}

/// Holds a session against other front-ends until it is dropped; the kernel
/// releases it if the process dies.
#[derive(Debug)]
pub(crate) struct SessionLock {
    _file: Held,
}

/// An open file whose advisory lock this process holds. Dropping it unlocks
/// the file before closing it.
///
/// Closing alone is not enough: a lock goes only when the last descriptor of
/// its open file closes, and while a process is spawned, by any thread of
/// this one, the new process holds a copy of every descriptor until it has
/// started its program. macOS copies even close-on-exec descriptors there,
/// and closes them only as the program starts. A lock released by closing
/// its file during someone's spawn stays held for as long as that spawn takes,
/// so a session showed as busy, and could not be locked, just after its run
/// ended. An explicit unlock ends the lock for every copy at once.
#[derive(Debug)]
pub(crate) struct Held(File);

impl Drop for Held {
    fn drop(&mut self) {
        unlock(&self.0);
    }
}

/// Why a session could not be locked.
#[derive(Debug)]
pub(crate) enum LockError {
    /// Another run holds the session.
    Busy,
    /// The lock file could not be made.
    Failed(anyhow::Error),
}

/// Takes the advisory lock that keeps other front-ends, in this or another
/// process, from running the same session.
pub(crate) fn lock_session(session_dir: &Path, id: &str) -> Result<SessionLock, LockError> {
    let open = || -> Result<File> {
        create_private_dir(&session_dir.join(".locks"))?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options
            .open(lock_path(session_dir, id))
            .context("lock session")
    };
    let file = open().map_err(LockError::Failed)?;
    // A probe for a busy session holds the lock for an instant; only a lock
    // that stays taken belongs to a run.
    for attempt in 1..=5 {
        match try_lock(&file) {
            Ok(true) => return Ok(SessionLock { _file: Held(file) }),
            Ok(false) if attempt < 5 => std::thread::sleep(Duration::from_millis(10)),
            Ok(false) => return Err(LockError::Busy),
            Err(error) => return Err(LockError::Failed(error.into())),
        }
    }
    Err(LockError::Busy)
}

/// Whether a run, in this or another process, holds the session right now.
pub(crate) fn session_busy(session_dir: &Path, id: &str) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path(session_dir, id))
    else {
        return false; // never locked
    };
    match try_lock(&file) {
        // Free: the probe took the lock, and lets it go before anyone sees it.
        Ok(true) => {
            drop(Held(file));
            false
        }
        Ok(false) => true,
        Err(_) => false,
    }
}

/// Tries to take an exclusive lock on the open file; `false` when another
/// open file holds it.
#[cfg(unix)]
fn try_lock(file: &File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: flock takes a descriptor this process owns and has no other
    // preconditions.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

/// Without `flock` there is no lock to take: other processes' runs are not seen.
#[cfg(not(unix))]
fn try_lock(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

/// Releases the lock this process holds on the open file, whatever copies of
/// its descriptor other processes hold for the moment. See [`Held`].
#[cfg(unix)]
fn unlock(file: &File) {
    use std::os::fd::AsRawFd as _;
    // SAFETY: flock takes a descriptor this process owns and has no other
    // preconditions.
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(not(unix))]
fn unlock(_file: &File) {}

/// Something a runner said, or that it is gone.
#[derive(Debug)]
pub(crate) enum Output {
    /// A line of its output.
    Line {
        /// The line without its newline.
        text: String,
        /// From its diagnostic output rather than its events.
        stderr: bool,
    },
    /// It exited; `None` when a signal ended it.
    Exited(Option<i32>),
}

/// A runner started for one request, attached to this process.
pub(crate) struct Process {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    done: Arc<AtomicBool>,
    stops: bool,
    _lock: SessionLock,
    _lease: Option<Lease>,
}

/// What starting a runner takes.
pub(crate) struct Start<'a> {
    pub runner: &'a Path,
    pub workspace: &'a Path,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The JSON request, one line.
    pub request: &'a [u8],
    /// Whether stdin stays open after the request, for messages and the
    /// line that says why the run is stopped.
    pub keep_input: bool,
    /// Whether the runner reads a stop line.
    pub stops: bool,
    pub lock: SessionLock,
    pub lease: Option<Lease>,
}

impl Process {
    /// Starts the runner in a process group of its own and reads its output
    /// into `output`, every line before the exit.
    pub(crate) fn start(
        start: Start,
        output: impl Fn(Output) + Send + Clone + 'static,
    ) -> Result<Self> {
        let mut command = Command::new(start.runner);
        command
            .args(&start.args)
            .current_dir(start.workspace)
            .envs(start.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            // Interrupts go to the runner's group; its tools run in groups
            // of their own, which the runner stops before it exits.
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let mut child = command.spawn().context("start runner")?;
        let mut stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no runner output"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("no runner output"))?;
        if let Some(input) = &mut stdin {
            // stdin keeps prompts out of process listings and argv limits.
            let sent = input
                .write_all(start.request)
                .and_then(|()| input.write_all(b"\n"))
                .and_then(|()| input.flush());
            if sent.is_err() {
                log::debug!("the runner closed its input before the request");
            }
        }
        if !start.keep_input {
            stdin = None;
        }

        let (drained, readers) = mpsc::channel::<()>();
        for (stream, is_stderr) in [
            (Box::new(stdout) as Box<dyn Read + Send>, false),
            (Box::new(stderr) as Box<dyn Read + Send>, true),
        ] {
            let (output, drained) = (output.clone(), drained.clone());
            std::thread::spawn(move || {
                read_lines(stream, |text| {
                    output(Output::Line {
                        text,
                        stderr: is_stderr,
                    });
                });
                let _ = drained.send(());
            });
        }
        let child = Arc::new(Mutex::new(child));
        let done = Arc::new(AtomicBool::new(false));
        {
            let (child, done) = (child.clone(), done.clone());
            std::thread::spawn(move || {
                let code = loop {
                    let status = child
                        .lock()
                        .ok()
                        .and_then(|mut child| child.try_wait().ok());
                    match status {
                        Some(Some(status)) => break status.code(),
                        Some(None) => std::thread::sleep(Duration::from_millis(25)),
                        None => break None,
                    }
                };
                done.store(true, Ordering::SeqCst);
                // What the runner wrote before it exited comes first; a tool
                // that kept a pipe open does not hold the exit forever.
                for _ in 0..2 {
                    let _ = readers.recv_timeout(Duration::from_secs(2));
                }
                output(Output::Exited(code));
            });
        }
        Ok(Self {
            child,
            stdin,
            done,
            stops: start.stops,
            _lock: start.lock,
            _lease: start.lease,
        })
    }

    /// Hands the running agent a line on its input: a message, or the stop.
    pub(crate) fn send(&mut self, line: &[u8]) -> bool {
        let Some(stdin) = &mut self.stdin else {
            return false;
        };
        let sent = stdin
            .write_all(line)
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        sent.is_ok()
    }

    /// Interrupts the runner, telling it why first when it takes that: the
    /// session records the reason, and the next run tells the agent. A
    /// runner still there after [`STOP_TIMEOUT`] is killed with its group.
    pub(crate) fn stop(&mut self, reason: &str) {
        if self.stops {
            let line = stop_line(reason);
            self.send(line.as_bytes());
        }
        self.signal(false);
        let (child, done) = (self.child.clone(), self.done.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + STOP_TIMEOUT;
            while Instant::now() < deadline {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            kill(&child, true);
        });
    }

    fn signal(&self, hard: bool) {
        kill(&self.child, hard);
    }

    /// Whether the runner has exited.
    pub(crate) fn exited(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

/// Signals a runner that still runs: an interrupt to its process group, or a
/// kill. Holding the child while signalling keeps the PID from being reaped
/// and reused under the signal.
fn kill(child: &Arc<Mutex<Child>>, hard: bool) {
    let Ok(mut child) = child.lock() else {
        return;
    };
    if matches!(child.try_wait(), Ok(None)) {
        signal_child(&mut child, hard);
    }
}

#[cfg(unix)]
fn signal_child(child: &mut Child, hard: bool) {
    signal_group(child.id(), if hard { libc::SIGKILL } else { libc::SIGINT });
}

/// Without process groups and signals, stopping is killing.
#[cfg(not(unix))]
fn signal_child(child: &mut Child, _hard: bool) {
    let _ = child.kill();
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: kill has no memory preconditions; the group is the runner's own.
    unsafe {
        libc::kill(-pid, signal);
    }
}

/// The line of a runner's input that stops it and says why.
pub(crate) fn stop_line(reason: &str) -> String {
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    serde_json::json!({ "stop": { "reason": reason } }).to_string()
}

/// Reads a stream to its end, a line at a time.
fn read_lines(stream: impl Read, mut each: impl FnMut(String)) {
    let mut reader = BufReader::with_capacity(256 << 10, stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if !line.is_empty() {
            each(String::from_utf8_lossy(&line).into_owned());
        }
    }
}

/// Reads a file that grows, a line at a time: the end of a line still being
/// written waits for the rest.
#[derive(Debug)]
pub(crate) struct Tail {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    /// A tail of the file at `path` from `offset` on. The file may not exist yet.
    pub(crate) fn new(path: PathBuf, offset: u64) -> Self {
        Self {
            path,
            file: None,
            offset,
            partial: Vec::new(),
        }
    }

    /// Hands each line written since the last read to `each`.
    pub(crate) fn read(&mut self, mut each: impl FnMut(&str)) -> std::io::Result<()> {
        if self.file.is_none() {
            match File::open(&self.path) {
                Ok(mut file) => {
                    file.seek(SeekFrom::Start(self.offset))?;
                    self.file = Some(file);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        let mut buffer = vec![0; 256 << 10];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            self.offset += read as u64;
            let mut data = &buffer[..read];
            while let Some(end) = memchr::memchr(b'\n', data) {
                self.partial.extend_from_slice(&data[..end]);
                data = &data[end + 1..];
                if !self.partial.is_empty() {
                    each(&String::from_utf8_lossy(&self.partial));
                }
                self.partial.clear();
            }
            self.partial.extend_from_slice(data);
        }
    }
}

/// What a detached run's directory records of it (`job.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DetachedInfo {
    /// The runner's process, which leads its own process group.
    pub pid: i32,
    /// The session it runs.
    pub session_id: String,
    /// How long the session's file was when the runner started: the run's
    /// output follows the session as it was then.
    pub session_size: u64,
    /// The runner takes the line that stops it.
    pub stops: bool,
    /// The runner reads messages for the running agent on its input.
    #[serde(default)]
    pub steers: bool,
}

/// A run that a cockpit started apart from itself and that goes on: its
/// directory holds the runner's input, output and diagnostics as files.
#[derive(Debug)]
pub(crate) struct Detached {
    /// The run's directory.
    pub dir: PathBuf,
    /// What its `job.json` says.
    pub info: DetachedInfo,
    /// Held while this front-end follows the run, so that no two do.
    _follow: Held,
}

/// Why a detached run was not taken up.
#[derive(Debug)]
pub(crate) enum AdoptError {
    /// No run of the session goes on apart from a front-end.
    None,
    /// Another front-end follows the run.
    Followed,
}

/// Takes up the detached run of a session, if one goes on and no front-end
/// follows it.
pub(crate) fn adopt(session_dir: &Path, id: &str) -> Result<Detached, AdoptError> {
    let runs = session_dir.join(".runs");
    let Ok(entries) = std::fs::read_dir(&runs) else {
        return Err(AdoptError::None);
    };
    let mut followed = false;
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(info) = std::fs::read(dir.join("job.json"))
            .ok()
            .and_then(|data| serde_json::from_slice::<DetachedInfo>(&data).ok())
        else {
            continue;
        };
        if info.session_id != id || info.pid <= 0 || !process_alive(info.pid) {
            continue;
        }
        let Ok(follow) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("follow.lock"))
        else {
            continue;
        };
        match try_lock(&follow) {
            Ok(true) => {
                return Ok(Detached {
                    dir,
                    info,
                    _follow: Held(follow),
                });
            }
            _ => followed = true,
        }
    }
    Err(if followed {
        AdoptError::Followed
    } else {
        AdoptError::None
    })
}

impl Detached {
    /// The runner's events, `{"type":"exit"}` last.
    pub(crate) fn output(&self) -> Tail {
        Tail::new(self.dir.join("out.jsonl"), 0)
    }

    /// The runner's diagnostics.
    pub(crate) fn errors(&self) -> Tail {
        Tail::new(self.dir.join("err.log"), 0)
    }

    /// Whether the runner has exited: it released the session, or is gone.
    pub(crate) fn gone(&self, session_dir: &Path) -> bool {
        !session_busy(session_dir, &self.info.session_id) || !process_alive(self.info.pid)
    }

    /// Hands the running agent a message, as a cockpit that follows the run
    /// would: a line appended to the runner's input.
    pub(crate) fn steer(&self, line: &str) -> Result<()> {
        if !self.info.steers {
            bail!("the run takes no messages");
        }
        let mut input = std::fs::OpenOptions::new()
            .append(true)
            .open(self.dir.join("in.jsonl"))
            .context("message the run")?;
        input.write_all(format!("{line}\n").as_bytes())?;
        Ok(())
    }

    /// Stops the run as a cockpit that follows it would: the line that says
    /// why on the runner's input, an interrupt to its group, and a kill for
    /// a runner still there after [`STOP_TIMEOUT`].
    pub(crate) fn stop(&self, reason: &str, session_dir: &Path) -> Result<()> {
        if self.info.stops {
            let mut input = std::fs::OpenOptions::new()
                .append(true)
                .open(self.dir.join("in.jsonl"))
                .context("stop the run")?;
            input.write_all(format!("{}\n", stop_line(reason)).as_bytes())?;
        }
        self.interrupt(session_dir)
    }

    #[cfg(unix)]
    fn interrupt(&self, session_dir: &Path) -> Result<()> {
        let (pid, id, dir) = (
            self.info.pid,
            self.info.session_id.clone(),
            session_dir.to_owned(),
        );
        signal_group(pid.unsigned_abs(), libc::SIGINT);
        std::thread::spawn(move || {
            let deadline = Instant::now() + STOP_TIMEOUT;
            while Instant::now() < deadline {
                if !session_busy(&dir, &id) || !process_alive(pid) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            signal_group(pid.unsigned_abs(), libc::SIGKILL);
        });
        Ok(())
    }

    #[cfg(not(unix))]
    fn interrupt(&self, _session_dir: &Path) -> Result<()> {
        bail!("a run apart from its front-end cannot be stopped here")
    }
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let answer = unsafe { libc::kill(pid, 0) };
    answer == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    false
}

/// Finds the runner: `$KOU_RUNNER` (or the cockpits' `$KOU_CONVEYOR_RUNNER`),
/// then `PATH`, then the repository's `bin/` beside or above the executable
/// or the working directory.
pub(crate) fn locate() -> Option<PathBuf> {
    for variable in ["KOU_RUNNER", "KOU_CONVEYOR_RUNNER"] {
        if let Some(path) = std::env::var_os(variable).filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            if is_executable(&path) {
                return std::path::absolute(path).ok();
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let path = directory.join(RUNNER_NAME);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    let starts = [
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_owned)),
        std::env::current_dir().ok(),
    ];
    for start in starts.into_iter().flatten() {
        for directory in start.ancestors() {
            for candidate in [
                directory.join(RUNNER_NAME),
                directory.join("bin").join(RUNNER_NAME),
            ] {
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && runnable(&metadata))
}

#[cfg(unix)]
fn runnable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn runnable(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// Makes sure a provider the cockpit will ask for is one the runner has, so
/// an outdated runner is named as the problem instead of failing the run.
pub(crate) fn check_provider(runner: &Path, provider: &str) -> Result<()> {
    const BASE: [&str; 5] = [
        "ollama",
        "openai",
        "openai-codex",
        "openrouter",
        "fireworks",
    ];
    if provider.is_empty() || BASE.contains(&provider) {
        return Ok(());
    }
    let Ok(output) = Command::new(runner)
        .arg("-providers")
        .stdin(Stdio::null())
        .output()
    else {
        return Ok(()); // starting the run reports a missing runner
    };
    if !output.status.success() {
        return Ok(()); // cannot tell; the run reports whatever is wrong
    }
    let listed = String::from_utf8_lossy(&output.stdout);
    if listed.split_whitespace().any(|known| known == provider) {
        return Ok(());
    }
    bail!(
        "the runner is older than this app: {} has no {provider:?} provider; rebuild it with `make build`",
        runner.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_follow_the_runner_s_usage() {
        let old = "  -steer\n  files: array of {path: string}";
        assert_eq!(
            Features::of_usage(old),
            Features {
                steer: true,
                links: true,
                stop: false,
                stream: false
            }
        );
        let new =
            r#"-steer {"stop": {"reason": string}} {"type": "delta", ...} files: array of {path"#;
        assert_eq!(
            Features::of_usage(new),
            Features {
                steer: true,
                links: true,
                stop: true,
                stream: true
            }
        );
        assert_eq!(
            stop_line("the user\n stopped  it"),
            r#"{"stop":{"reason":"the user stopped it"}}"#
        );
    }

    #[test]
    fn the_runner_is_found_in_the_repository_this_crate_lives_in() {
        // The tests run in the crate's directory, below the repository's bin/.
        let built = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../bin")
            .join(RUNNER_NAME);
        if !built.is_file() {
            eprintln!("skipped: {} is not built", built.display());
            return;
        }
        let found = locate().expect("a runner");
        assert_eq!(
            found.file_name().and_then(|name| name.to_str()),
            Some(RUNNER_NAME)
        );
        assert!(is_executable(&found), "{}", found.display());
    }

    #[cfg(unix)]
    #[test]
    fn a_session_is_locked_against_other_front_ends_until_released() {
        let directory = tempfile::tempdir().expect("a directory");
        assert!(!session_busy(directory.path(), "s1"));
        let lock = lock_session(directory.path(), "s1").expect("the lock");
        assert!(session_busy(directory.path(), "s1"));
        assert!(matches!(
            lock_session(directory.path(), "s1"),
            Err(LockError::Busy)
        ));
        assert!(!session_busy(directory.path(), "s2"));
        drop(lock);
        assert!(!session_busy(directory.path(), "s1"));
        assert!(lock_session(directory.path(), "s1").is_ok());
    }

    /// A process spawned by another thread holds a copy of every descriptor
    /// while it starts, close-on-exec ones included on macOS: a lock closed
    /// meanwhile stayed held until that spawn was over.
    #[cfg(unix)]
    #[test]
    fn a_released_lock_is_free_at_once_while_other_threads_spawn_processes() {
        let directory = tempfile::tempdir().expect("a directory");
        let stop = Arc::new(AtomicBool::new(false));
        let spawners: Vec<_> = (0..4)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let _ = Command::new("/usr/bin/true").status();
                    }
                })
            })
            .collect();
        let mut held_on = 0;
        for _ in 0..300 {
            let lock = lock_session(directory.path(), "s1").expect("the lock");
            std::thread::sleep(Duration::from_micros(200));
            drop(lock);
            if session_busy(directory.path(), "s1") {
                held_on += 1;
            }
        }
        stop.store(true, Ordering::SeqCst);
        for spawner in spawners {
            spawner.join().expect("a spawner");
        }
        assert_eq!(held_on, 0, "times a released lock was still held");
    }

    #[test]
    fn a_tail_hands_over_whole_lines_as_the_file_grows() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("out.jsonl");
        let mut tail = Tail::new(path.clone(), 0);
        let mut lines = Vec::new();
        tail.read(|line| lines.push(line.to_owned()))
            .expect("no file yet");
        let mut file = File::create(&path).expect("a file");
        file.write_all(b"one\n\ntw").expect("written");
        tail.read(|line| lines.push(line.to_owned())).expect("read");
        assert_eq!(lines, ["one"]);
        file.write_all(b"o\nthree\n").expect("written");
        tail.read(|line| lines.push(line.to_owned())).expect("read");
        assert_eq!(lines, ["one", "two", "three"]);

        let mut later = Tail::new(path, 4);
        let mut rest = Vec::new();
        later.read(|line| rest.push(line.to_owned())).expect("read");
        assert_eq!(rest, ["two", "three"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_process_s_lines_come_before_its_exit_and_an_interrupt_stops_its_group() {
        let directory = tempfile::tempdir().expect("a directory");
        let (send, events) = mpsc::channel();
        let start = |script: &str, keep_input: bool| {
            let send = send.clone();
            Process::start(
                Start {
                    runner: Path::new("/bin/sh"),
                    workspace: directory.path(),
                    args: vec!["-c".into(), script.into()],
                    env: vec![("KOU_TEST_VALUE".into(), "from the launch".into())],
                    request: br#"{"prompt":"hi"}"#,
                    keep_input,
                    stops: true,
                    lock: lock_session(directory.path(), "s1").expect("the lock"),
                    lease: None,
                },
                move |output| {
                    let _ = send.send(output);
                },
            )
            .expect("a process")
        };
        let collect = |events: &mpsc::Receiver<Output>| {
            let mut lines = Vec::new();
            loop {
                match events
                    .recv_timeout(Duration::from_secs(20))
                    .expect("an event")
                {
                    Output::Line { text, stderr } => lines.push((text, stderr)),
                    Output::Exited(code) => return (lines, code),
                }
            }
        };

        let process = start(
            "read request; echo \"$request\"; echo \"$KOU_TEST_VALUE\" >&2; exit 3",
            false,
        );
        let (mut lines, code) = collect(&events);
        lines.sort();
        assert_eq!(
            lines,
            [
                ("from the launch".to_owned(), true),
                (r#"{"prompt":"hi"}"#.to_owned(), false)
            ]
        );
        assert_eq!(code, Some(3));
        assert!(process.exited());
        drop(process);
        assert!(
            !session_busy(directory.path(), "s1"),
            "the lock goes with the run"
        );

        // A line on stdin reaches the runner; an interrupt reaches its group.
        let mut process = start(
            "trap 'echo interrupted; exit 130' INT; read request; read stop; echo \"$stop\"; sleep 30 & wait",
            true,
        );
        assert!(process.send(stop_line("the user stopped the run").as_bytes()));
        match events
            .recv_timeout(Duration::from_secs(20))
            .expect("an event")
        {
            Output::Line { text, .. } => {
                assert_eq!(text, r#"{"stop":{"reason":"the user stopped the run"}}"#);
            }
            Output::Exited(code) => panic!("exited with {code:?} before the line"),
        }
        process.signal(false);
        let (lines, code) = collect(&events);
        assert_eq!(lines, [("interrupted".to_owned(), false)]);
        assert_eq!(code, Some(130));
    }
}
