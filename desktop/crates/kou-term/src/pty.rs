//! The pseudo-terminal: the child on its slave side, and three threads on the master
//! side. One reads what the child writes and feeds the emulator, one writes what is
//! typed and what the emulator answers, one waits for the child to exit.
//!
//! The spawn follows what alacritty_terminal's `tty::unix` does (Apache-2.0): a new
//! session with the slave as controlling terminal and default signal handlers.

use crate::osc::OscScanner;
use crate::{
    terminal::{Exit, GridPixels, Input, Internal, Shared, Spawn},
    vt::Vt,
};
use anyhow::Context as _;
use parking_lot::{Mutex, MutexGuard};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::{CommandExt as _, ExitStatusExt as _},
    },
    process::{Command, Stdio},
    sync::{Arc, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

/// How long a hung-up child gets before it is killed.
const KILL_GRACE: Duration = Duration::from_secs(1);

/// The most output parsed under one lock, so a view waiting to draw gets its turn.
const READ_CHUNK: usize = 64 * 1024;

/// How often the reader looks up from an idle pseudo-terminal to see whether it should
/// stop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the exit is held back for output still in flight to be read.
const DRAIN_LIMIT: Duration = Duration::from_millis(500);

/// Held while a pseudo-terminal is opened and its child forked.
static SPAWNING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn window_size(size: GridPixels) -> libc::winsize {
    libc::winsize {
        ws_row: size.lines,
        ws_col: size.columns,
        ws_xpixel: size.columns.saturating_mul(size.cell_width),
        ws_ypixel: size.lines.saturating_mul(size.cell_height),
    }
}

/// Tells the child the new size; the kernel sends it SIGWINCH when it changed.
pub(crate) fn set_window_size(master: &OwnedFd, size: GridPixels) {
    let window = window_size(size);
    // SAFETY: the descriptor is the pty's open master and `window` outlives the call.
    let result = unsafe {
        libc::ioctl(
            master.as_raw_fd(),
            libc::TIOCSWINSZ,
            &window as *const libc::winsize,
        )
    };
    if result < 0 {
        log::warn!(
            "resizing the pseudo-terminal failed: {}",
            io::Error::last_os_error()
        );
    }
}

/// Starts the child and the threads that serve it, and returns the master side for
/// resizing.
pub(crate) fn start(
    spawn: &Spawn,
    size: GridPixels,
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    events: async_channel::Sender<Internal>,
    wakeups: async_channel::Sender<()>,
    input: mpsc::Receiver<Input>,
) -> anyhow::Result<OwnedFd> {
    // A directory that does not exist would start the program wherever the application
    // happens to be: the wrong place for an agent to start editing files.
    anyhow::ensure!(
        spawn.cwd.is_dir(),
        "the working directory {} does not exist",
        spawn.cwd.display()
    );

    let mut env = HashMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    // Programs choose features by terminal name; a value inherited from wherever the
    // application was launched would describe some other terminal.
    env.insert("TERM_PROGRAM".to_string(), "kou".to_string());
    env.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    for (key, value) in &spawn.env {
        env.insert(key.clone(), value.clone());
    }

    let (master, child) = {
        // Opening a pseudo-terminal is not safe to do from two threads at once on macOS
        // (the slave's name comes back in a static buffer), and a child forked by one
        // thread must not inherit the descriptors another is half-way through setting up.
        let _spawning = SPAWNING.lock();
        let (master, slave) = open_pty(size)?;
        let mut command = Command::new(&spawn.program);
        command
            .args(&spawn.args)
            .current_dir(&spawn.cwd)
            .envs(&env)
            // Launch tokens meant for this application, not for what runs inside it.
            .env_remove("DESKTOP_STARTUP_ID")
            .env_remove("XDG_ACTIVATION_TOKEN")
            .stdin(Stdio::from(clone_file(&slave)?))
            .stdout(Stdio::from(clone_file(&slave)?))
            .stderr(Stdio::from(clone_file(&slave)?));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                // A session of its own, with the slave (now its stdin) as controlling
                // terminal, so the process group can be hung up as a whole.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                for signal in [
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGALRM,
                    libc::SIGPIPE,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", spawn.program.display()))?;
        // The parent keeps only the master; the child has its own copies of the slave.
        drop(slave);
        (master, child)
    };
    shared.set_child(child.id());

    let reader = clone_file(&master)?;
    let writer = clone_file(&master)?;

    {
        let shared = shared.clone();
        let wakeups = wakeups.clone();
        let events = events.clone();
        thread::Builder::new()
            .name("kou-term reader".into())
            .spawn(move || read_output(reader, vt, shared, wakeups, events))
            .context("starting the terminal reader")?;
    }

    // One queue for everything typed, pasted, submitted and answered, so a pause before
    // Enter delays what was queued behind it instead of letting it overtake.
    thread::Builder::new()
        .name("kou-term writer".into())
        .spawn(move || write_input(writer, input))
        .context("starting the terminal writer")?;

    {
        let shared = shared.clone();
        thread::Builder::new()
            .name("kou-term exit".into())
            .spawn(move || wait_for_exit(child, shared, events, wakeups))
            .context("starting the terminal exit watcher")?;
    }

    Ok(master)
}

/// Serves a program another terminal started and let go of: the same reader
/// and writer on its pseudo-terminal, and a watcher that polls for its end
/// instead of waiting on a child handle, which only the terminal that
/// started it has.
pub(crate) fn adopt(
    master: OwnedFd,
    pid: u32,
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    events: async_channel::Sender<Internal>,
    wakeups: async_channel::Sender<()>,
    input: mpsc::Receiver<Input>,
) -> anyhow::Result<OwnedFd> {
    shared.set_child(pid);
    let reader = clone_file(&master)?;
    let writer = clone_file(&master)?;
    {
        let shared = shared.clone();
        let wakeups = wakeups.clone();
        let events = events.clone();
        thread::Builder::new()
            .name("kou-term reader".into())
            .spawn(move || read_output(reader, vt, shared, wakeups, events))
            .context("starting the terminal reader")?;
    }
    thread::Builder::new()
        .name("kou-term writer".into())
        .spawn(move || write_input(writer, input))
        .context("starting the terminal writer")?;
    thread::Builder::new()
        .name("kou-term exit".into())
        .spawn(move || watch_adopted(pid, shared, events, wakeups))
        .context("starting the terminal exit watcher")?;
    Ok(master)
}

/// Waits for an adopted program to end, by asking now and then. Whoever
/// started it may reap it first, and its status with it: then all that is
/// known here is that it is gone.
fn watch_adopted(
    pid: u32,
    shared: Arc<Shared>,
    events: async_channel::Sender<Internal>,
    wakeups: async_channel::Sender<()>,
) {
    let exit = loop {
        // Handed on once more: the next terminal watches from here.
        if shared.released() {
            return;
        }
        let mut status: libc::c_int = 0;
        // SAFETY: a valid out pointer; WNOHANG returns at once.
        let reaped = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        let ran_for = shared.started().elapsed();
        if reaped == 0 {
            thread::sleep(POLL_INTERVAL);
        } else if reaped == pid as libc::pid_t {
            let status = std::process::ExitStatus::from_raw(status);
            break Exit {
                code: status.code(),
                signal: status.signal(),
                ran_for,
            };
        } else {
            break Exit {
                code: None,
                signal: None,
                ran_for,
            };
        }
    };
    shared.set_exit(exit);
    let deadline = Instant::now() + DRAIN_LIMIT;
    while !shared.drained.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let _ = wakeups.try_send(());
    let _ = events.try_send(Internal::Exited);
}

fn open_pty(size: GridPixels) -> anyhow::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut window = window_size(size);
    // SAFETY: valid out pointers; a null name and termios ask for the defaults.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut window,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error()).context("opening a pseudo-terminal");
    }
    // SAFETY: openpty returned two open descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // Programs this application starts later must not inherit either side.
        // SAFETY: plain flag change on an open descriptor.
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // The child must read what it is sent as UTF-8 and erase whole characters.
    // SAFETY: termios is plain data; tcgetattr fills it.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(master.as_raw_fd(), &mut termios) == 0 {
            termios.c_iflag |= libc::IUTF8;
            libc::tcsetattr(master.as_raw_fd(), libc::TCSANOW, &termios);
        }
    }
    Ok((master, slave))
}

fn clone_file(fd: &OwnedFd) -> anyhow::Result<File> {
    Ok(File::from(
        fd.try_clone()
            .context("duplicating the pseudo-terminal handle")?,
    ))
}

/// Reads the child's output into the emulator until the child's side closes or the
/// terminal goes away.
fn read_output(
    mut file: File,
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    wakeups: async_channel::Sender<()>,
    events: async_channel::Sender<Internal>,
) {
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut notifications = OscScanner::kitty_notifications();
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, POLL_INTERVAL.as_millis() as libc::c_int) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            continue;
        }
        let count = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            // EIO: every process on the child's side has closed it.
            Err(_) => break,
        };
        shared.note_output(count);
        notifications.feed(&buffer[..count], |event| {
            let _ = events.try_send(Internal::Osc(event));
        });
        let mut locked = vt.lock();
        locked.write(&buffer[..count]);
        // A view waiting to draw gets the lock before the next chunk is parsed.
        MutexGuard::unlock_fair(locked);
        if !shared.render_held() {
            let _ = wakeups.try_send(());
        }
    }
    shared.drained.store(true, Ordering::Release);
    let _ = wakeups.try_send(());
}

fn write_input(mut file: File, input: mpsc::Receiver<Input>) {
    for item in input {
        match item {
            Input::Bytes(bytes) => {
                if file.write_all(&bytes).is_err() {
                    break;
                }
            }
            Input::Pause(duration) => thread::sleep(duration),
        }
    }
}

fn wait_for_exit(
    mut child: std::process::Child,
    shared: Arc<Shared>,
    events: async_channel::Sender<Internal>,
    wakeups: async_channel::Sender<()>,
) {
    let status = child.wait();
    let ran_for = shared.started().elapsed();
    let exit = match status {
        Ok(status) => Exit {
            code: status.code(),
            signal: status.signal(),
            ran_for,
        },
        Err(error) => {
            log::warn!("waiting for the terminal's child failed: {error}");
            Exit {
                code: None,
                signal: None,
                ran_for,
            }
        }
    };
    // Recorded at once: the pid is reaped, and the kill watchdog must not signal it.
    shared.set_exit(exit);
    // What the child wrote just before it exited is still in the pseudo-terminal; it
    // belongs on screen before the exit is reported. A background process that keeps the
    // slave open would hold this forever, hence the limit.
    let deadline = Instant::now() + DRAIN_LIMIT;
    while !shared.drained.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let _ = wakeups.try_send(());
    let _ = events.try_send(Internal::Exited);
}

/// Hangs up the child's process group and, if it is still there after [`KILL_GRACE`],
/// kills it. The exit watcher notices the exit either way and reaps the child.
pub(crate) fn hang_up(shared: &Arc<Shared>) {
    let Some(pid) = shared.take_child_to_hang_up() else {
        return;
    };
    // The child called `setsid`, so its pid names its process group.
    // SAFETY: plain signal delivery to a process group this terminal started.
    unsafe { libc::killpg(pid as libc::pid_t, libc::SIGHUP) };
    let shared = shared.clone();
    let spawned = thread::Builder::new()
        .name("kou-term kill".into())
        .spawn(move || {
            let step = Duration::from_millis(25);
            let mut waited = Duration::ZERO;
            while waited < KILL_GRACE {
                // Checked before every signal: once the child is reaped its pid may
                // belong to someone else.
                if shared.has_exited() {
                    return;
                }
                thread::sleep(step);
                waited += step;
            }
            if !shared.has_exited() {
                // SAFETY: as above.
                unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
            }
        });
    if let Err(error) = spawned {
        log::warn!("could not start the terminal kill watchdog: {error}");
    }
}
