//! A child process that one thread waits for while others signal it.
//!
//! `std::process::Child` needs `&mut self` to wait, so it cannot be waited on
//! and killed at once, and signalling by process id after the child was reaped
//! can hit an unrelated process that got the same id. Here the child is its own
//! process group, a watcher thread learns of its exit without reaping it, and
//! every signal goes out under the lock that also guards the reaping. Until
//! [`Process::reap`] the id therefore stays ours, and whatever Claude Code left
//! running in its group (its shells, its MCP servers) can still be swept up.
//!
//! The same as `kou-acp`'s, which keeps its own private.

use std::{
    io,
    process::{Child, Command, ExitStatus},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// It returned this code.
    Code(i32),
    /// This signal killed it.
    Signal(i32),
    /// The system would not say.
    Unknown,
}

impl Exit {
    /// For a sentence: `code 1`, `signal SIGKILL`.
    pub fn describe(self) -> String {
        match self {
            Exit::Code(code) => format!("code {code}"),
            Exit::Signal(signal) => format!("signal {}", signal_name(signal)),
            Exit::Unknown => "an unknown status".into(),
        }
    }
}

impl From<ExitStatus> for Exit {
    fn from(status: ExitStatus) -> Self {
        if let Some(code) = status.code() {
            return Exit::Code(code);
        }
        #[cfg(unix)]
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return Exit::Signal(signal);
        }
        Exit::Unknown
    }
}

fn signal_name(signal: i32) -> String {
    #[cfg(unix)]
    {
        let name = match signal {
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            libc::SIGABRT => "SIGABRT",
            libc::SIGKILL => "SIGKILL",
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGPIPE => "SIGPIPE",
            libc::SIGTERM => "SIGTERM",
            _ => "",
        };
        if !name.is_empty() {
            return name.into();
        }
    }
    signal.to_string()
}

/// Make the command the leader of a new process group, so that signalling the
/// group reaches the children it starts and never our own process.
pub(crate) fn own_group(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// A running or finished child. See the module documentation.
pub(crate) struct Process {
    pid: u32,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    child: Child,
    exit: Option<Exit>,
    reaped: bool,
}

/// How often the status is polled where the system cannot wait without reaping.
const POLL: Duration = Duration::from_millis(50);
/// How long a killed process gets to disappear before we stop waiting for it.
const KILL_WAIT: Duration = Duration::from_secs(2);

impl Process {
    /// Take over `child`, which must have been spawned with [`own_group`], and
    /// start the thread that waits for it. `on_exit` runs on that thread.
    pub fn watch(
        child: Child,
        thread_name: &str,
        on_exit: impl FnOnce(Exit) + Send + 'static,
    ) -> io::Result<Arc<Self>> {
        let process = Arc::new(Self {
            pid: child.id(),
            state: Mutex::new(State {
                child,
                exit: None,
                reaped: false,
            }),
            changed: Condvar::new(),
        });
        let watched = process.clone();
        let spawned = thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let exit = watched.wait();
                watched.lock().exit = Some(exit);
                watched.changed.notify_all();
                // The callback must not keep the process alive in a cycle.
                drop(watched);
                on_exit(exit);
            });
        if let Err(error) = spawned {
            process.reap();
            return Err(error);
        }
        Ok(process)
    }

    /// How it ended, once the watcher has seen it end.
    pub fn exit(&self) -> Option<Exit> {
        self.lock().exit
    }

    /// Stop the process and its group at once.
    pub fn kill(&self) {
        self.signal(Signal::Kill);
    }

    /// Wait until the watcher has seen the process end, at most `timeout`.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Exit> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while state.exit.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = match self.changed.wait_timeout(state, left) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        state.exit
    }

    /// Terminate, wait out `grace`, kill what is left, and release the process id.
    pub fn stop(&self, grace: Duration) -> Exit {
        self.signal(Signal::Terminate);
        let exit = self.wait_timeout(grace).or_else(|| {
            self.kill();
            self.wait_timeout(KILL_WAIT)
        });
        self.reap();
        exit.or_else(|| self.exit()).unwrap_or(Exit::Unknown)
    }

    /// Kill whatever is left of the group and release the process id. After
    /// this the process can no longer be signalled.
    pub fn reap(&self) {
        let mut state = self.lock();
        if state.reaped {
            return;
        }
        #[cfg(unix)]
        self.signal_group(libc::SIGKILL);
        #[cfg(not(unix))]
        let _ = state.child.kill();
        match state.child.wait() {
            Ok(status) => {
                state.exit.get_or_insert(status.into());
            }
            Err(error) => log::debug!("waiting for process {} failed: {error}", self.pid),
        }
        state.reaped = true;
        drop(state);
        self.changed.notify_all();
    }

    #[cfg(unix)]
    fn signal(&self, signal: Signal) {
        // Held while signalling, so that nobody reaps in between.
        let state = self.lock();
        if !state.reaped {
            self.signal_group(match signal {
                Signal::Terminate => libc::SIGTERM,
                Signal::Kill => libc::SIGKILL,
            });
        }
    }

    /// Without signals both requests mean the end, and only for the process itself.
    #[cfg(not(unix))]
    fn signal(&self, _signal: Signal) {
        let mut state = self.lock();
        if !state.reaped && state.exit.is_none() {
            let _ = state.child.kill();
        }
    }

    /// Must be called with the state locked and the child not reaped: only
    /// then is the group id certain to still be the child's.
    #[cfg(unix)]
    fn signal_group(&self, signal: libc::c_int) {
        // Group 0 is our own and 1 is init's; neither can be a child's.
        if self.pid > 1 {
            // SAFETY: `killpg` takes plain integers and has no memory effects here.
            unsafe { libc::killpg(self.pid as libc::pid_t, signal) };
        }
    }

    /// Block until the process has ended, and say how.
    fn wait(&self) -> Exit {
        #[cfg(unix)]
        {
            let seen = wait_without_reaping(self.pid);
            // If it was reaped meanwhile the id may belong to someone else by
            // now, so what `waitid` saw cannot be trusted; the child's own
            // cached status below can.
            if !self.lock().reaped
                && let Some(exit) = seen
            {
                return exit;
            }
        }
        loop {
            {
                let mut state = self.lock();
                match state.child.try_wait() {
                    Ok(Some(status)) => {
                        state.reaped = true;
                        return status.into();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        log::debug!("polling process {} failed: {error}", self.pid);
                        state.reaped = true;
                        return Exit::Unknown;
                    }
                }
            }
            thread::sleep(POLL);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Never leave a zombie or a stray group behind.
        self.reap();
    }
}

#[derive(Clone, Copy)]
enum Signal {
    Terminate,
    Kill,
}

/// Block until `pid`, a child of ours, has ended, leaving it waitable so that
/// its id is not reused. `None` if the system refuses, which is also what
/// happens when someone reaped the child meanwhile.
#[cfg(unix)]
fn wait_without_reaping(pid: u32) -> Option<Exit> {
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to fill in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` outlives the call, and the flags leave the child unreaped.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            // SAFETY: `waitid` succeeded for a child, so the child fields are set.
            let status = unsafe { info.si_status() };
            return Some(match info.si_code {
                libc::CLD_EXITED => Exit::Code(status),
                libc::CLD_KILLED | libc::CLD_DUMPED => Exit::Signal(status),
                _ => Exit::Unknown,
            });
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
}
