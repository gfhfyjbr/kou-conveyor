//! The host process: a daemon that starts the program, owns its standard
//! streams, and serves one client at a time on a Unix socket.
//!
//! One thread owns the state ([`Host::serve`]). One each reads the
//! program's output and its standard error, writes its input, waits for it
//! to end, waits for signals, and takes connections; two more read and write
//! the client of the moment. They all speak to the owner over one channel,
//! so that nothing the program or a client does can hold up the other.

use super::{
    Arguments, EXIT, Exit, HELLO, Last, NOISE, REPLACED, Reason, STDERR, STOP, last_path,
    memory::Memory, note,
};
use anyhow::Context as _;
use std::{
    collections::VecDeque,
    fs::{self, File, Permissions},
    io::{self, BufRead, BufReader, Read, Write},
    mem::MaybeUninit,
    net::Shutdown,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

/// How long the program has between `SIGTERM` and `SIGKILL`.
const GRACE: Duration = Duration::from_secs(2);
/// How long the output of a program that has ended is read on, for what it
/// wrote last, when something it left running still holds its pipes.
const DRAIN: Duration = Duration::from_millis(500);
/// How many of the last lines of the program's standard error
/// `<socket>.last` keeps, and the longest such line in characters.
const LAST_WORDS: usize = 20;
const LAST_WORD_LENGTH: usize = 400;

/// Become the daemon and serve; see [`super::run`].
pub(super) fn run(arguments: Arguments) -> i32 {
    // SAFETY: `fork` copies only the calling thread, so a child forked
    // while another thread holds a lock (the allocator's, stdio's) could
    // never take it. The host is the first thing its process does: the
    // example calls it from `main` at once, and the app's binary under its
    // subcommand before it starts anything, so this is the only thread there
    // is. Everything after the fork runs in a process of one thread until
    // it starts its own.
    match unsafe { libc::fork() } {
        -1 => {
            let error = io::Error::last_os_error();
            say(&format!("could not become a daemon: {error}"));
            return 1;
        }
        0 => {}
        _ => return 0,
    }
    close_inherited();
    let code = match serve(arguments) {
        Ok(()) => 0,
        Err(why) => {
            say(&format!("{why:#}"));
            1
        }
    };
    std::process::exit(code)
}

/// What the starter reads, while its pipe is still standard error here.
fn say(text: &str) {
    let _ = writeln!(io::stderr(), "{text}");
}

/// Close every descriptor but the standard three. Whatever the starter let
/// this process inherit (by mistake, or in a race of the system's: macOS
/// opens a pipe and marks it close-on-exec in two steps, so a child started
/// meanwhile by another thread of the app gets it) is not the daemon's to
/// hold open for as long as it lives.
fn close_inherited() {
    let Ok(entries) = fs::read_dir("/dev/fd") else {
        return;
    };
    let open: Vec<libc::c_int> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    // The listing's own descriptor is among them, closed already.
    for fd in open.into_iter().filter(|fd| *fd > libc::STDERR_FILENO) {
        // SAFETY: nothing in this process uses a descriptor above the
        // standard three yet; closing one that is not open does nothing.
        unsafe { libc::close(fd) };
    }
}

fn serve(arguments: Arguments) -> anyhow::Result<()> {
    // A hangup is ignored: by a handler that does nothing rather than by
    // `SIG_IGN`, which the program would inherit (a handler is undone when a
    // program is started). Once the host's threads run, the signal is
    // blocked and waited for instead (`wait_for_signals`).
    extern "C" fn ignore(_: libc::c_int) {}
    // SAFETY: the handler does nothing, which is safe for any signal.
    unsafe {
        libc::signal(
            libc::SIGHUP,
            ignore as extern "C" fn(libc::c_int) as libc::sighandler_t,
        )
    };
    // A session of its own: no terminal's hangup or the app's group reaches
    // it.
    // SAFETY: a plain call.
    if unsafe { libc::setsid() } == -1 {
        let error = io::Error::last_os_error();
        anyhow::bail!("could not leave the app's session: {error}");
    }
    // A client gone in the middle of a write is an error of that write, not
    // the host's end. (Rust's `main` ignores it already; the app's binary
    // might not have one. The program starts with it restored.)
    // SAFETY: a plain call.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // Standard error stays the starter's until the host listens.
    to_null(&[libc::STDIN_FILENO, libc::STDOUT_FILENO])
        .context("could not let go of the starter's input and output")?;
    anyhow::ensure!(
        arguments.cwd.is_dir(),
        "the folder {} does not exist",
        arguments.cwd.display()
    );
    let listener = listen(&arguments.socket)?;
    let socket = Socket {
        id: file_id(&arguments.socket),
        path: arguments.socket.clone(),
    };
    let host = match Host::start(&arguments, socket, listener) {
        Ok(host) => host,
        Err((socket, error)) => {
            note(&socket.path, "host", &format!("could not start: {error:#}"));
            socket.remove();
            return Err(error);
        }
    };
    note(
        &host.socket.path,
        "host",
        &format!(
            "started {} (pid {}) in {}",
            arguments.program.display(),
            host.pid,
            arguments.cwd.display()
        ),
    );
    // The starter reads its end of standard error to the end: this is it.
    let _ = to_null(&[libc::STDERR_FILENO]);
    host.serve();
    Ok(())
}

/// The signals the host takes: `SIGTERM` and `SIGINT` end it, `SIGHUP` is
/// ignored.
fn signal_set() -> libc::sigset_t {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initializes the set, and `sigaddset` adds valid
    // signals to it.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(set.as_mut_ptr(), signal);
        }
        set.assume_init()
    }
}

/// Block `set` in this thread and so in every thread started from it, for
/// one of them to wait for. Children inherit the mask, so this comes after
/// the program is started: it gets these signals as any program does.
fn block(set: &libc::sigset_t) -> io::Result<()> {
    // SAFETY: the set is initialized and the old mask is not asked for.
    match unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, set, ptr::null_mut()) } {
        0 => Ok(()),
        error => Err(io::Error::from_raw_os_error(error)),
    }
}

/// Point the descriptors `fds` at `/dev/null`.
fn to_null(fds: &[libc::c_int]) -> io::Result<()> {
    let null = File::options().read(true).write(true).open("/dev/null")?;
    for fd in fds {
        // SAFETY: both descriptors are open, and `dup2` replaces `fd` at once.
        if unsafe { libc::dup2(null.as_raw_fd(), *fd) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Listen at `path`, for its owner only. A socket that answers there is a
/// host already there; one that does not is stale and removed.
fn listen(path: &Path) -> anyhow::Result<UnixListener> {
    let place = path.display();
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if UnixStream::connect(path).is_ok() {
            anyhow::bail!("a keeper already listens at {place}");
        }
        anyhow::ensure!(
            metadata.file_type().is_socket(),
            "{place} is in the way: it is not a socket"
        );
        fs::remove_file(path).with_context(|| format!("could not remove the stale {place}"))?;
    }
    // Made for its owner only from the start, so that nobody else connects
    // before its permissions are set.
    // SAFETY: plain calls; with one thread, nothing else is created under
    // this mask meanwhile.
    let mask = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(path);
    // SAFETY: as above.
    unsafe { libc::umask(mask) };
    let listener = listener.with_context(|| format!("could not listen at {place}"))?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .with_context(|| format!("could not make {place} its owner's only"))?;
    Ok(listener)
}

/// A file's identity, to tell the socket this host made from one another
/// made at the same path since.
fn file_id(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

/// The socket this host listens at.
struct Socket {
    path: PathBuf,
    id: Option<(u64, u64)>,
}

impl Socket {
    /// Remove it, if it is still the one this host made.
    fn remove(&self) {
        if self.id.is_some() && file_id(&self.path) == self.id {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// What the threads tell the one that owns the state.
enum Event {
    /// A line of the program's standard output.
    Output(String),
    OutputClosed,
    /// A line of its standard error.
    Errors(String),
    ErrorsClosed,
    /// It has ended, and is not yet reaped.
    Exited(Exit),
    Connected(UnixStream),
    /// A line from the client of this number.
    FromClient(u64, String),
    /// The client of this number is gone.
    Left(u64),
    /// The client of this number was dropped, for this.
    Dropped(u64, String),
    /// `SIGTERM` or `SIGINT`.
    Terminate,
}

/// Where the program is in its life.
#[derive(Clone, Copy)]
enum Phase {
    Running,
    /// Sent `SIGTERM`, for `reason`; `SIGKILL` at `kill_at`, unless sent
    /// already.
    Stopping {
        reason: Reason,
        kill_at: Instant,
        killed: bool,
    },
    /// It has ended; what it wrote is read on until both pipes close or
    /// `until`.
    Ended {
        reason: Reason,
        exit: Exit,
        until: Instant,
    },
}

/// The client of the moment.
struct Connection {
    id: u64,
    /// Lines for its writer.
    lines: mpsc::Sender<String>,
    stream: UnixStream,
    writer: thread::JoinHandle<()>,
}

impl Connection {
    fn open(
        id: u64,
        stream: UnixStream,
        events: &mpsc::Sender<Event>,
        stall: Duration,
    ) -> io::Result<Self> {
        let started = (|| {
            // Only a backstop: see `send_patiently`.
            stream.set_write_timeout(Some(stall))?;
            let (reading, writing) = (stream.try_clone()?, stream.try_clone()?);
            let (lines, outbox) = mpsc::channel();
            let (heard, dropped) = (events.clone(), events.clone());
            thread::Builder::new()
                .name("kou-claude host client in".into())
                .spawn(move || read_client(reading, id, &heard))?;
            let writer = thread::Builder::new()
                .name("kou-claude host client out".into())
                .spawn(move || {
                    if let Err(why) = write_client(writing, &outbox, stall) {
                        let _ = dropped.send(Event::Dropped(id, why.to_string()));
                    }
                })?;
            Ok((lines, writer))
        })();
        match started {
            Ok((lines, writer)) => Ok(Self {
                id,
                lines,
                stream,
                writer,
            }),
            Err(error) => {
                let _ = stream.shutdown(Shutdown::Both);
                Err(error)
            }
        }
    }

    /// Close the connection at once; what was not yet written is lost.
    fn close(self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Tell the client that another has taken its place, and let it go:
    /// what waits to be written to it still is, then the connection
    /// closes. What it writes from now on is not taken (its lines carry its
    /// number, which is no longer the client's).
    fn replace(self) {
        let _ = self.lines.send(REPLACED.to_owned());
        // The writer ends once it has written all and its channel is
        // closed, which dropping `self` does, and closes the connection.
    }

    /// Write what is left and `last`, then close the connection.
    fn finish(self, last: String) {
        let _ = self.lines.send(last);
        drop(self.lines);
        // Bounded: each write the writer is held up in gives up after the
        // host's stall time.
        let _ = self.writer.join();
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// The state, owned by one thread.
struct Host {
    socket: Socket,
    idle: Duration,
    away: Duration,
    stall: Duration,
    /// The program's name, for the log.
    name: String,
    child: Child,
    pid: u32,
    /// When the program was started, in seconds since the epoch.
    started: u64,
    memory: Memory,
    events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    /// Lines for the program's standard input.
    input: mpsc::Sender<String>,
    client: Option<Connection>,
    /// How many clients there have been.
    clients: u64,
    /// When the last client left, or the host started.
    left: Instant,
    /// When the program last wrote to its standard output.
    written: Instant,
    last_words: VecDeque<String>,
    phase: Phase,
    output_open: bool,
    errors_open: bool,
}

impl Host {
    /// Start the program and the threads around it. On failure the socket
    /// is handed back, for the caller to remove.
    fn start(
        arguments: &Arguments,
        socket: Socket,
        listener: UnixListener,
    ) -> Result<Self, (Socket, anyhow::Error)> {
        let _ = fs::remove_file(last_path(&socket.path));
        let mut command = Command::new(&arguments.program);
        command
            .args(&arguments.args)
            .current_dir(&arguments.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own group, so that ending it ends what it started, and
            // never the host.
            .process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let why = format!("could not start {}: {error}", arguments.program.display());
                return Err((socket, anyhow::anyhow!(why)));
            }
        };
        let pid = child.id();
        let (sender, events) = mpsc::channel();
        let (input, lines) = mpsc::channel::<String>();
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let threads = (|| -> io::Result<()> {
            let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
                return Err(io::Error::other("the program has no pipes"));
            };
            let signals = signal_set();
            block(&signals)?;
            spawn("kou-claude host input", move || write_lines(stdin, &lines))?;
            let output = sender.clone();
            spawn("kou-claude host output", move || {
                read_lines(stdout, &output, Event::Output, Event::OutputClosed);
            })?;
            let errors = sender.clone();
            spawn("kou-claude host errors", move || {
                read_lines(stderr, &errors, Event::Errors, Event::ErrorsClosed);
            })?;
            let exited = sender.clone();
            spawn("kou-claude host exit", move || {
                let _ = exited.send(Event::Exited(wait_without_reaping(pid)));
            })?;
            let terminate = sender.clone();
            spawn("kou-claude host signals", move || {
                wait_for_signals(signals, &terminate);
            })?;
            let connected = sender.clone();
            spawn("kou-claude host accept", move || {
                accept(&listener, &connected)
            })?;
            Ok(())
        })();
        if let Err(error) = threads {
            // Half a host is none: the program does not stay behind.
            kill_group(pid, libc::SIGKILL);
            let _ = child.wait();
            let why = anyhow::Error::new(error).context("could not start the host's threads");
            return Err((socket, why));
        }
        let now = Instant::now();
        Ok(Self {
            socket,
            idle: arguments.idle,
            away: arguments.away,
            stall: arguments.stall,
            name: arguments.program.file_name().map_or_else(
                || "the program".into(),
                |name| name.to_string_lossy().into_owned(),
            ),
            child,
            pid,
            started: seconds_since_epoch(),
            memory: Memory::default(),
            events,
            sender,
            input,
            client: None,
            clients: 0,
            left: now,
            written: now,
            last_words: VecDeque::new(),
            phase: Phase::Running,
            output_open: true,
            errors_open: true,
        })
    }

    /// Serve until the program has ended, then end.
    fn serve(mut self) {
        loop {
            let event = match self.deadline() {
                Some(at) => {
                    let wait = at.saturating_duration_since(Instant::now());
                    self.events.recv_timeout(wait).ok()
                }
                // The host holds a sender itself: this cannot fail.
                None => self.events.recv().ok(),
            };
            if let Some(event) = event {
                self.handle(event);
            }
            let now = Instant::now();
            self.check(now);
            if self.over(now) {
                break;
            }
        }
        self.finish();
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Output(line) => {
                self.written = Instant::now();
                if line.starts_with('{') {
                    self.memory.note_program(&line);
                    self.tell(line);
                } else {
                    self.tell(format!("{NOISE} {line}"));
                }
            }
            Event::OutputClosed => self.output_open = false,
            Event::Errors(line) => {
                if self.last_words.len() == LAST_WORDS {
                    self.last_words.pop_front();
                }
                self.last_words.push_back(clip(&line, LAST_WORD_LENGTH));
                self.tell(format!("{STDERR} {line}"));
            }
            Event::ErrorsClosed => self.errors_open = false,
            Event::Exited(exit) => {
                let reason = match self.phase {
                    Phase::Stopping { reason, .. } => reason,
                    _ => Reason::Exited,
                };
                self.note(&format!("{} {exit}", self.name));
                self.phase = Phase::Ended {
                    reason,
                    exit,
                    until: Instant::now() + DRAIN,
                };
            }
            Event::Connected(stream) => self.welcome(stream),
            // Only the client of the moment is listened to: one whose place
            // was taken may still be writing, and its `#stop` is not for the
            // program any more.
            Event::FromClient(id, line) if self.is_client(id) => {
                if line.starts_with('{') {
                    self.memory.note_client(&line);
                    let _ = self.input.send(line);
                } else if line == STOP {
                    self.note(&format!("client {id} asked to stop"));
                    self.stop(Reason::Stopped);
                }
            }
            Event::FromClient(..) => {}
            Event::Left(id) => {
                if self.is_client(id) {
                    self.note(&format!("client {id} left"));
                    if let Some(client) = self.client.take() {
                        client.close();
                    }
                    self.left = Instant::now();
                }
            }
            Event::Dropped(id, why) => {
                if self.is_client(id) {
                    self.note(&format!("client {id} was dropped: {why}"));
                    if let Some(client) = self.client.take() {
                        client.close();
                    }
                    self.left = Instant::now();
                }
            }
            Event::Terminate => {
                self.note("was sent SIGTERM");
                self.stop(Reason::Terminated);
            }
        }
    }

    /// A new client takes the place of the one before it, and is told how
    /// things stand.
    fn welcome(&mut self, stream: UnixStream) {
        self.clients += 1;
        let id = self.clients;
        match self.client.take() {
            Some(before) => {
                self.note(&format!(
                    "client {id} took the place of client {}",
                    before.id
                ));
                before.replace();
            }
            None => self.note(&format!("client {id} came")),
        }
        let hello = self.memory.hello(self.pid, self.started, id == 1);
        match Connection::open(id, stream, &self.sender, self.stall) {
            Ok(client) => {
                let _ = client.lines.send(format!("{HELLO} {hello}"));
                self.client = Some(client);
            }
            Err(error) => {
                self.note(&format!("could not take client {id}: {error}"));
                self.left = Instant::now();
            }
        }
    }

    fn is_client(&self, id: u64) -> bool {
        self.client.as_ref().is_some_and(|client| client.id == id)
    }

    /// A line in the log beside the socket.
    fn note(&self, text: &str) {
        note(&self.socket.path, "host", text);
    }

    /// Pass a line to the client, if there is one.
    fn tell(&self, line: String) {
        if let Some(client) = &self.client {
            // A writer that gave up has closed the connection; its reader
            // says so.
            let _ = client.lines.send(line);
        }
    }

    /// End the program and its group: `SIGTERM` now, `SIGKILL` after
    /// [`GRACE`].
    fn stop(&mut self, reason: Reason) {
        if let Phase::Running = self.phase {
            kill_group(self.pid, libc::SIGTERM);
            self.phase = Phase::Stopping {
                reason,
                kill_at: Instant::now() + GRACE,
                killed: false,
            };
        }
    }

    /// When something is due by itself, if anything is.
    fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Running if self.client.is_none() => {
                let away = self.left.checked_add(self.away);
                let idle = self
                    .memory
                    .quiet()
                    .then(|| self.left.max(self.written).checked_add(self.idle))
                    .flatten();
                match (away, idle) {
                    (Some(away), Some(idle)) => Some(away.min(idle)),
                    (away, idle) => away.or(idle),
                }
            }
            Phase::Running => None,
            Phase::Stopping {
                kill_at, killed, ..
            } => (!killed).then_some(kill_at),
            Phase::Ended { until, .. } => Some(until),
        }
    }

    /// Do what is due at `now`.
    fn check(&mut self, now: Instant) {
        match self.phase {
            Phase::Running if self.client.is_none() => {
                let due = |since: Instant, after: Duration| {
                    since.checked_add(after).is_some_and(|at| now >= at)
                };
                if due(self.left, self.away) {
                    self.note("nobody came for its away time");
                    self.stop(Reason::Away);
                } else if self.memory.quiet() && due(self.left.max(self.written), self.idle) {
                    self.note("nobody came for its idle time, and nothing was going on");
                    self.stop(Reason::Idle);
                }
            }
            Phase::Stopping {
                reason,
                kill_at,
                killed: false,
            } if now >= kill_at => {
                kill_group(self.pid, libc::SIGKILL);
                self.phase = Phase::Stopping {
                    reason,
                    kill_at,
                    killed: true,
                };
            }
            _ => {}
        }
    }

    /// The program has ended and all it wrote has been read, or as much as
    /// will be.
    fn over(&self, now: Instant) -> bool {
        match self.phase {
            Phase::Ended { until, .. } => (!self.output_open && !self.errors_open) || now >= until,
            _ => false,
        }
    }

    /// Sweep the program's group, leave word of how it ended, take the
    /// socket away, tell the client, and be gone.
    fn finish(mut self) {
        let Phase::Ended { reason, exit, .. } = self.phase else {
            return;
        };
        // Whatever it left running goes with it; then its id is let go.
        kill_group(self.pid, libc::SIGKILL);
        let exit = match self.child.wait() {
            Ok(status) => Exit::from(status),
            Err(_) => exit,
        };
        let last = Last {
            reason,
            exit,
            stderr: self.last_words.drain(..).collect(),
            ended: seconds_since_epoch(),
        };
        let written = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(last_path(&self.socket.path))
            .and_then(|mut file| {
                let text = serde_json::to_string(&last).map_err(io::Error::other)?;
                file.write_all(text.as_bytes())
            });
        if let Err(error) = written {
            log::debug!("could not leave word of the end: {error}");
        }
        self.socket.remove();
        self.note(&format!("ended ({reason}): {} {exit}", self.name));
        if let Some(client) = self.client.take() {
            let exit = serde_json::to_string(&exit).unwrap_or_else(|_| "{}".into());
            client.finish(format!("{EXIT} {exit}"));
        }
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<()> {
    thread::Builder::new().name(name.into()).spawn(body)?;
    Ok(())
}

fn seconds_since_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Signal the program's group. Only called before the program is reaped,
/// while its id, which is the group's, cannot have been given to another.
fn kill_group(pid: u32, signal: libc::c_int) {
    // Group 0 is our own and 1 is init's; neither can be the program's.
    if pid > 1 {
        // SAFETY: `killpg` takes plain integers and has no memory effects.
        unsafe { libc::killpg(pid as libc::pid_t, signal) };
    }
}

/// Lines of `pipe`, each sent as `line(text)`, then `closed`. Blank lines
/// are skipped.
fn read_lines(
    pipe: impl Read,
    events: &mpsc::Sender<Event>,
    line: fn(String) -> Event,
    closed: Event,
) {
    let mut reader = BufReader::new(pipe);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            continue;
        }
        if events.send(line(text.to_owned())).is_err() {
            return;
        }
    }
    let _ = events.send(closed);
}

/// The program's input: one line each, flushed as written. Ends, closing the
/// pipe, when the program stops reading.
fn write_lines(mut stdin: impl Write, lines: &mpsc::Receiver<String>) {
    for line in lines {
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if written.is_err() {
            return;
        }
    }
}

/// A client's lines, until it is gone. A line cut short by the end of the
/// connection is not one.
fn read_client(stream: UnixStream, id: u64, events: &mpsc::Sender<Event>) {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(_) if bytes.ends_with(b"\n") => {}
            Ok(_) | Err(_) => break,
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_end_matches(['\n', '\r']).to_owned();
        if events.send(Event::FromClient(id, text)).is_err() {
            return;
        }
    }
    let _ = events.send(Event::Left(id));
}

/// Write the host's lines to a client, as many at once as are waiting, until
/// there are no more to come. A client that takes nothing for `stall` has
/// the connection ended, and is gone: the error says why.
fn write_client(
    stream: UnixStream,
    lines: &mpsc::Receiver<String>,
    stall: Duration,
) -> io::Result<()> {
    /// About as much as is written in one go.
    const BATCH: usize = 64 * 1024;
    let mut batch = Vec::new();
    while let Ok(line) = lines.recv() {
        batch.clear();
        let mut line = Some(line);
        while let Some(more) = line.take() {
            batch.extend_from_slice(more.as_bytes());
            batch.push(b'\n');
            if batch.len() < BATCH {
                line = lines.try_recv().ok();
            }
        }
        if let Err(error) = send_patiently(&stream, &batch, stall) {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(error);
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

/// Write all of `bytes` to `stream`, giving up once it has taken nothing
/// for `stall`. The socket is waited on until it has room, then given a
/// piece small enough to fit, so that no write waits by itself. (A send
/// asked not to wait, `MSG_DONTWAIT`, was seen to wait all the same on
/// macOS, and the socket cannot be made one that never waits: the thread
/// that reads it shares that setting. The socket's own timeout, set as well, only ends a
/// write that took part of its bytes once the time is over, so alone it
/// could let a stall last twice as long.)
fn send_patiently(stream: &UnixStream, mut bytes: &[u8], stall: Duration) -> io::Result<()> {
    /// No more than the least room a socket that polls writable has (its
    /// low-water mark, two kilobytes unless set otherwise).
    const PIECE: usize = 1024;
    let mut out = stream;
    let mut taken = Instant::now();
    while !bytes.is_empty() {
        let left = stall.saturating_sub(taken.elapsed());
        if left.is_zero() || !writable(stream, left)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "it took nothing for too long",
            ));
        }
        match out.write(&bytes[..bytes.len().min(PIECE)]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(sent) => {
                bytes = &bytes[sent..];
                taken = Instant::now();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Whether `stream` has room for a write within `wait`. A connection that
/// is closed or broken counts as having room: the write says what is wrong.
fn writable(stream: &UnixStream, wait: Duration) -> io::Result<bool> {
    let mut ready = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let wait = wait.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
    loop {
        // SAFETY: one valid `pollfd`, for the length of the call.
        match unsafe { libc::poll(&mut ready, 1, wait) } {
            0 => return Ok(false),
            ready if ready > 0 => return Ok(true),
            _ => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

/// Take connections for as long as the host lives.
fn accept(listener: &UnixListener, events: &mpsc::Sender<Event>) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if events.send(Event::Connected(stream)).is_err() {
                    return;
                }
            }
            Err(error) => {
                // Out of descriptors, say: a moment, then again.
                log::debug!("could not take a connection: {error}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Wait for the blocked signals, for as long as the host lives.
fn wait_for_signals(set: libc::sigset_t, events: &mpsc::Sender<Event>) {
    loop {
        let mut signal = 0;
        // SAFETY: the set is initialized and blocked in every thread, as
        // `sigwait` wants.
        if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
            return;
        }
        // A hangup has no terminal of the host's to come from: ignored.
        if signal != libc::SIGHUP && events.send(Event::Terminate).is_err() {
            return;
        }
    }
}

/// Block until `pid`, a child of ours, has ended, leaving it unreaped so
/// that its id, and its group's, stay ours. An unknown exit if the system
/// refuses.
fn wait_without_reaping(pid: u32) -> Exit {
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to
        // fill in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` outlives the call, and the flags leave the child
        // unreaped.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            // SAFETY: `waitid` succeeded for a child, so the child fields
            // are set.
            let status = unsafe { info.si_status() };
            return match info.si_code {
                libc::CLD_EXITED => Exit {
                    code: Some(status),
                    signal: None,
                },
                libc::CLD_KILLED | libc::CLD_DUMPED => Exit {
                    code: None,
                    signal: Some(status),
                },
                _ => Exit::default(),
            };
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Exit::default();
        }
    }
}

/// At most `limit` characters of `text`.
fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}
