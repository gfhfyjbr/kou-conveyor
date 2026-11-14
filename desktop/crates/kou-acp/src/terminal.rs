//! The commands an agent runs through the client: `terminal/*`.
//!
//! A terminal here is a child process whose standard output and error share
//! one pipe, so the capture keeps the order the command wrote in. There is no
//! pseudo-terminal: the command sees a pipe, which is what a tool that only
//! wants the output expects.

use crate::{
    process::{self, Exit, Process},
    rpc::{self, RpcError},
};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// What a terminal tells the session thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalEvent {
    /// There is output the session has not looked at yet.
    Output(String),
    /// The command ended and its output is complete.
    Exited(String),
}

/// The parameters of `terminal/create`, resolved.
#[derive(Clone, Debug)]
pub(crate) struct Spawn {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// The most bytes to keep; `None` for [`DEFAULT_LIMIT`].
    pub limit: Option<u64>,
}

/// A terminal's output and status at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub output: String,
    /// Output was dropped from the front to stay within the limit.
    pub truncated: bool,
    pub exit: Option<Exit>,
}

/// Kept when the agent names no limit, so a runaway command cannot fill memory.
pub(crate) const DEFAULT_LIMIT: usize = 8 * 1024 * 1024;
/// How long output is awaited after the command ended: a child it left behind
/// can hold the pipe open for as long as it likes.
const DRAIN: Duration = Duration::from_millis(250);
/// How long a released command gets to die before its status is given up on.
const RELEASE_WAIT: Duration = Duration::from_secs(1);
/// How many released terminals stay readable for tool calls that show them late.
const KEPT: usize = 64;

/// Every terminal one session created.
pub(crate) struct Terminals {
    live: HashMap<String, Terminal>,
    released: VecDeque<(String, Snapshot)>,
    next: u64,
    notify: Arc<dyn Fn(TerminalEvent) + Send + Sync>,
}

struct Terminal {
    process: Arc<Process>,
    shared: Arc<Shared>,
}

struct Shared {
    capture: Mutex<Capture>,
    progressed: Condvar,
    /// Set while an [`TerminalEvent::Output`] is on its way and not yet handled.
    dirty: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Capture> {
        self.capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The retained tail of a command's output.
#[derive(Debug)]
struct Capture {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
    /// The pipe reached its end.
    complete: bool,
    exit: Option<Exit>,
}

impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            truncated: false,
            complete: false,
            exit: None,
        }
    }

    /// Append, then drop from the front what exceeds the limit, never
    /// splitting a character: the retained text must stay valid.
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= self.limit {
            return;
        }
        let mut cut = self.bytes.len() - self.limit;
        // A UTF-8 continuation byte cannot start the text; a character has at
        // most three of them, so this never eats more than that.
        while cut < self.bytes.len() && self.bytes[cut] & 0b1100_0000 == 0b1000_0000 {
            cut += 1;
        }
        self.bytes.drain(..cut);
        self.truncated = true;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            output: String::from_utf8_lossy(&self.bytes).into_owned(),
            truncated: self.truncated,
            exit: self.exit,
        }
    }
}

impl Terminals {
    /// `notify` is called from the terminals' own threads.
    pub fn new(notify: impl Fn(TerminalEvent) + Send + Sync + 'static) -> Self {
        Self {
            live: HashMap::new(),
            released: VecDeque::new(),
            next: 1,
            notify: Arc::new(notify),
        }
    }

    /// Start a command and return the terminal's id at once.
    pub fn create(&mut self, spawn: Spawn) -> Result<String, RpcError> {
        if !spawn.cwd.is_dir() {
            return Err(RpcError::new(
                rpc::INVALID_PARAMS,
                format!(
                    "the working directory {} does not exist",
                    spawn.cwd.display()
                ),
            ));
        }
        let (child, reader) = start(&spawn).map_err(|error| {
            RpcError::new(
                rpc::INTERNAL_ERROR,
                format!("could not run {}: {error}", spawn.command),
            )
        })?;
        let id = format!("term-{}", self.next);
        self.next += 1;
        let limit = spawn
            .limit
            .map_or(DEFAULT_LIMIT, |limit| limit.min(usize::MAX as u64) as usize);
        let shared = Arc::new(Shared {
            capture: Mutex::new(Capture::new(limit)),
            progressed: Condvar::new(),
            dirty: AtomicBool::new(false),
        });

        let process = {
            let (shared, notify, id) = (shared.clone(), self.notify.clone(), id.clone());
            Process::watch(child, "kou-acp terminal exit", move |exit| {
                let deadline = Instant::now() + DRAIN;
                let mut capture = shared.lock();
                while !capture.complete {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    capture = match shared.progressed.wait_timeout(capture, left) {
                        Ok((capture, _)) => capture,
                        Err(poisoned) => poisoned.into_inner().0,
                    };
                }
                capture.exit = Some(exit);
                drop(capture);
                shared.progressed.notify_all();
                notify(TerminalEvent::Exited(id));
            })
        }
        .map_err(|error| RpcError::new(rpc::INTERNAL_ERROR, error.to_string()))?;

        let reading = {
            let (shared, notify, id) = (shared.clone(), self.notify.clone(), id.clone());
            thread::Builder::new()
                .name("kou-acp terminal output".into())
                .spawn(move || {
                    read_output(reader, &shared, || {
                        notify(TerminalEvent::Output(id.clone()))
                    })
                })
        };
        if let Err(error) = reading {
            process.reap();
            return Err(RpcError::new(rpc::INTERNAL_ERROR, error.to_string()));
        }
        self.live.insert(id.clone(), Terminal { process, shared });
        Ok(id)
    }

    /// Whether the terminal exists and was not released.
    pub fn is_live(&self, id: &str) -> bool {
        self.live.contains_key(id)
    }

    /// The output so far of a live terminal, or the last of a released one.
    pub fn snapshot(&self, id: &str) -> Option<Snapshot> {
        if let Some(terminal) = self.live.get(id) {
            return Some(terminal.shared.lock().snapshot());
        }
        self.released
            .iter()
            .rev()
            .find(|(released, _)| released == id)
            .map(|(_, snapshot)| snapshot.clone())
    }

    /// How a live terminal's command ended, once it has and its output is complete.
    pub fn exit(&self, id: &str) -> Option<Exit> {
        self.live.get(id)?.shared.lock().exit
    }

    /// Like [`Self::snapshot`] for a live terminal, and marks its output as seen,
    /// so that the next output raises a new [`TerminalEvent::Output`].
    pub fn take_snapshot(&self, id: &str) -> Option<Snapshot> {
        let terminal = self.live.get(id)?;
        terminal.shared.dirty.store(false, Ordering::Release);
        Some(terminal.shared.lock().snapshot())
    }

    /// Note that the output so far needs no attention, so that the next
    /// output raises a new [`TerminalEvent::Output`].
    pub fn mark_seen(&self, id: &str) {
        if let Some(terminal) = self.live.get(id) {
            terminal.shared.dirty.store(false, Ordering::Release);
        }
    }

    /// `terminal/kill`: stop the command, keep the terminal.
    pub fn kill(&self, id: &str) -> Result<(), RpcError> {
        self.live.get(id).ok_or_else(|| unknown(id))?.process.kill();
        Ok(())
    }

    /// `terminal/release`: stop the command and forget the terminal. What it
    /// printed stays readable through [`Self::snapshot`] for a while.
    pub fn release(&mut self, id: &str) -> Result<Snapshot, RpcError> {
        let terminal = self.live.remove(id).ok_or_else(|| unknown(id))?;
        let snapshot = terminal.finish();
        self.released.push_back((id.to_owned(), snapshot.clone()));
        if self.released.len() > KEPT {
            self.released.pop_front();
        }
        Ok(snapshot)
    }

    /// Stop every command. Used when the session ends.
    pub fn release_all(&mut self) {
        for (_, terminal) in self.live.drain() {
            terminal.process.kill();
            terminal.process.reap();
        }
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        self.release_all();
    }
}

impl Terminal {
    /// Kill the command and whatever it left running, and take the last state.
    fn finish(self) -> Snapshot {
        let already = self.shared.lock().exit;
        let exit = already.or_else(|| {
            self.process.kill();
            self.process.wait_timeout(RELEASE_WAIT)
        });
        self.process.reap();
        let mut snapshot = self.shared.lock().snapshot();
        snapshot.exit = snapshot.exit.or(exit);
        snapshot
    }
}

fn unknown(id: &str) -> RpcError {
    RpcError::new(rpc::RESOURCE_NOT_FOUND, format!("no terminal {id}"))
}

/// Spawn the command with both output streams on one pipe.
fn start(spawn: &Spawn) -> io::Result<(std::process::Child, io::PipeReader)> {
    let run = |program: &str, args: &[String]| {
        let (reader, writer) = io::pipe()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(spawn.env.iter().map(|(name, value)| (name, value)))
            .current_dir(&spawn.cwd)
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer);
        process::own_group(&mut command);
        // `command` owns our copies of the pipe's write end; it is dropped
        // on return, which is what lets the reader see the end of the output.
        command.spawn().map(|child| (child, reader))
    };
    match run(&spawn.command, &spawn.args) {
        // Agents also send a whole command line in `command`, with no `args`.
        Err(error) if error.kind() == io::ErrorKind::NotFound && spawn.args.is_empty() => {
            let (shell, flag) = if cfg!(windows) {
                ("cmd", "/C")
            } else {
                ("/bin/sh", "-c")
            };
            run(shell, &[flag.to_owned(), spawn.command.clone()])
        }
        result => result,
    }
}

fn read_output(mut reader: impl Read, shared: &Shared, announce: impl Fn()) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                shared.lock().push(&chunk[..count]);
                // One event stands for all the output until it is looked at.
                if !shared.dirty.swap(true, Ordering::AcqRel) {
                    announce();
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    shared.lock().complete = true;
    shared.progressed.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn the_capture_keeps_the_tail_and_never_splits_a_character() {
        let mut capture = Capture::new(10);
        capture.push(b"0123456789");
        assert_eq!(
            capture.snapshot(),
            Snapshot {
                output: "0123456789".into(),
                truncated: false,
                exit: None
            }
        );
        capture.push(b"abcdef");
        assert_eq!(capture.snapshot().output, "6789abcdef");
        assert!(capture.snapshot().truncated);

        // "é" is two bytes; a cut in its middle drops the whole character.
        let mut capture = Capture::new(4);
        capture.push("aéé".as_bytes());
        assert_eq!(capture.snapshot().output, "éé");
        capture.push("é".as_bytes());
        assert_eq!(capture.snapshot().output, "éé");
        let mut capture = Capture::new(3);
        capture.push("éé".as_bytes());
        assert_eq!(capture.snapshot().output, "é");
        // Four-byte characters, and a limit smaller than one of them.
        let mut capture = Capture::new(5);
        capture.push("😀😀".as_bytes());
        assert_eq!(capture.snapshot().output, "😀");
        let mut capture = Capture::new(2);
        capture.push("😀".as_bytes());
        assert_eq!(capture.snapshot().output, "");
        assert!(capture.snapshot().truncated);
    }

    #[cfg(unix)]
    fn terminals() -> (Terminals, mpsc::Receiver<TerminalEvent>) {
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        let terminals = Terminals::new(move |event| {
            let _ = sender.lock().unwrap().send(event);
        });
        (terminals, receiver)
    }

    #[cfg(unix)]
    fn spawn(command: &str, args: &[&str]) -> Spawn {
        Spawn {
            command: command.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: vec![("KOU_ACP_TEST".into(), "yes".into())],
            cwd: std::env::temp_dir(),
            limit: None,
        }
    }

    #[cfg(unix)]
    fn wait_exit(events: &mpsc::Receiver<TerminalEvent>, id: &str) {
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(TerminalEvent::Exited(exited)) if exited == id => return,
                Ok(_) => {}
                Err(error) => panic!("terminal {id} did not exit: {error}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_command_runs_with_both_streams_in_order_and_its_environment() {
        let (mut terminals, events) = terminals();
        let id = terminals
            .create(spawn(
                "/bin/sh",
                &["-c", "echo out; echo err >&2; echo $KOU_ACP_TEST; exit 7"],
            ))
            .unwrap();
        assert!(terminals.is_live(&id));
        wait_exit(&events, &id);
        assert_eq!(
            terminals.snapshot(&id),
            Some(Snapshot {
                output: "out\nerr\nyes\n".into(),
                truncated: false,
                exit: Some(Exit::Code(7)),
            })
        );
        let last = terminals.release(&id).unwrap();
        assert_eq!(last.exit, Some(Exit::Code(7)));
        assert!(!terminals.is_live(&id));
        // Released, but a tool call that shows it late still finds the output.
        assert_eq!(terminals.snapshot(&id), Some(last));
        assert_eq!(
            terminals.release(&id).unwrap_err().code,
            rpc::RESOURCE_NOT_FOUND
        );
        assert_eq!(
            terminals.kill("term-99").unwrap_err().code,
            rpc::RESOURCE_NOT_FOUND
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_line_without_arguments_goes_through_the_shell() {
        let (mut terminals, events) = terminals();
        let id = terminals
            .create(spawn("printf 'a b' && printf ' c'", &[]))
            .unwrap();
        wait_exit(&events, &id);
        let snapshot = terminals.snapshot(&id).unwrap();
        assert_eq!(
            (snapshot.output.as_str(), snapshot.exit),
            ("a b c", Some(Exit::Code(0)))
        );

        let error = terminals
            .create(spawn("/no/such/program", &["x"]))
            .unwrap_err();
        assert_eq!(error.code, rpc::INTERNAL_ERROR);
        let mut missing = spawn("true", &[]);
        missing.cwd = PathBuf::from("/no/such/directory");
        assert_eq!(
            terminals.create(missing).unwrap_err().code,
            rpc::INVALID_PARAMS
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_limit_keeps_the_end_of_the_output() {
        let (mut terminals, events) = terminals();
        let mut limited = spawn("/bin/sh", &["-c", "printf 0123456789abcdef"]);
        limited.limit = Some(10);
        let id = terminals.create(limited).unwrap();
        wait_exit(&events, &id);
        let snapshot = terminals.snapshot(&id).unwrap();
        assert_eq!(
            (snapshot.output.as_str(), snapshot.truncated),
            ("6789abcdef", true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn kill_keeps_the_terminal_and_release_ends_a_running_command() {
        let (mut terminals, events) = terminals();
        let id = terminals
            .create(spawn("/bin/sh", &["-c", "echo started; sleep 600"]))
            .unwrap();
        // The first output raises one event; it is not repeated until looked at.
        assert_eq!(
            events.recv_timeout(Duration::from_secs(10)),
            Ok(TerminalEvent::Output(id.clone()))
        );
        assert_eq!(terminals.take_snapshot(&id).unwrap().output, "started\n");
        terminals.kill(&id).unwrap();
        wait_exit(&events, &id);
        let snapshot = terminals.snapshot(&id).unwrap();
        assert_eq!(snapshot.exit, Some(Exit::Signal(libc::SIGKILL)));
        assert_eq!(snapshot.output, "started\n");
        assert!(terminals.is_live(&id));

        let running = terminals
            .create(spawn("/bin/sh", &["-c", "sleep 600"]))
            .unwrap();
        let last = terminals.release(&running).unwrap();
        assert_eq!(last.exit, Some(Exit::Signal(libc::SIGKILL)));
    }

    #[cfg(unix)]
    #[test]
    fn a_child_left_behind_does_not_hold_up_the_exit() {
        let (mut terminals, events) = terminals();
        let started = Instant::now();
        let id = terminals
            .create(spawn("/bin/sh", &["-c", "sleep 600 & echo done"]))
            .unwrap();
        wait_exit(&events, &id);
        assert!(started.elapsed() < Duration::from_secs(5));
        let snapshot = terminals.snapshot(&id).unwrap();
        assert_eq!(
            (snapshot.output.as_str(), snapshot.exit),
            ("done\n", Some(Exit::Code(0)))
        );
        terminals.release_all();
        assert!(!terminals.is_live(&id));
    }
}
