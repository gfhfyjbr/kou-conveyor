//! A session's Claude Code kept by a host ([`crate::host`]), so that it
//! outlives the app: where a session's hosts listen, finding the one that
//! is the session's, starting one, and hearing what it relays as a link's
//! events.
//!
//! A session's processes listen at `<stem>-<n>.sock` ([`crate::Keeper`]),
//! `n` counted from 1: an edit of a sent prompt moves the session to a new
//! process, at the next `n`, and the host before it is stopped once the new
//! one is taken up. The highest `n` that answers is therefore the session's.

use super::{
    link::{self, Event},
    process::Exit,
};
use crate::{
    Keeper, Options,
    host::{self, Attached, Received},
};
use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};

/// The program of a host, reached through its client.
pub(crate) struct Kept {
    client: host::Client,
    over: Arc<Over>,
    /// Where the host listens: beside it, why it ended, and the log.
    socket: PathBuf,
}

/// The connection to a host is over: set by the thread that hears it.
#[derive(Default)]
pub(crate) struct Over {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Over {
    fn set(&self) {
        *self
            .done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.changed.notify_all();
    }
}

impl Kept {
    pub fn new(client: host::Client, over: Arc<Over>, socket: PathBuf) -> Self {
        Self {
            client,
            over,
            socket,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Write a line to the program.
    pub fn write(&self, line: &str) -> io::Result<()> {
        self.client.write(line)
    }

    /// Ask the host to end the program, what it started and itself.
    pub fn stop(&self) {
        if let Err(error) = self.client.stop() {
            log::debug!("the keeper did not take the stop: {error}");
        }
    }

    /// Wait until the host has closed the connection, at most `patience`.
    pub fn wait_over(&self, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        let mut done = self
            .over
            .done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            done = match self.over.changed.wait_timeout(done, left) {
                Ok((done, _)) => done,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        *done
    }
}

/// Where process `n` of a keeper listens, and how it is kept.
pub(crate) fn keep_at(keeper: &Keeper, n: u32) -> host::Keep {
    let mut socket: OsString = keeper.stem.as_os_str().to_owned();
    socket.push(format!("-{n}.sock"));
    host::Keep {
        host: keeper.host.clone(),
        host_args: keeper.host_args.clone(),
        socket: PathBuf::from(socket),
        idle: keeper.idle,
        away: keeper.away,
        stall: host::STALL,
    }
}

impl Keeper {
    /// The host of a pane that keeps one app-server process across turns and forks.
    pub fn single_process(&self) -> host::Keep {
        keep_at(self, 1)
    }
}

/// The numbers of the sockets of a keeper there are, highest first. (A
/// socket's file outlives a host that was killed: whether one answers is
/// for [`find`] to see.)
fn sockets(stem: &Path) -> Vec<u32> {
    let (Some(folder), Some(name)) = (stem.parent(), stem.file_name()) else {
        return Vec::new();
    };
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let prefix = format!("{}-", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<u32> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name();
            let name = name.to_str()?;
            name.strip_prefix(&prefix)?
                .strip_suffix(".sock")?
                .parse()
                .ok()
        })
        .collect();
    found.sort_unstable_by(|a, b| b.cmp(a));
    found
}

/// The host of the session, if one answers: the one of the highest number.
/// Any other that answers is a process an edit left behind, and is stopped.
/// Also the number the session's next host gets.
pub(crate) fn find(keeper: &Keeper) -> (Option<(u32, Attached)>, u32) {
    let numbers = sockets(&keeper.stem);
    let next = numbers.first().map_or(1, |highest| highest + 1);
    let mut found = None;
    for n in numbers {
        let keep = keep_at(keeper, n);
        match keep.attach() {
            Ok(Some(attached)) if found.is_none() => found = Some((n, attached)),
            Ok(Some(behind)) => {
                log::info!("stopping the keeper left at {}", keep.socket.display());
                let _ = behind.client.stop();
            }
            Ok(None) => {}
            Err(error) => log::warn!("{error:#}"),
        }
    }
    (found, next)
}

/// Start a host at number `n` for the program, arguments, folder and
/// environment a child would get, and attach to it.
pub(crate) fn start(
    keeper: &Keeper,
    n: u32,
    options: &Options,
    args: &[String],
) -> anyhow::Result<Attached> {
    link::check_folder(options)?;
    keep_at(keeper, n).start(
        &options.program,
        args,
        &options.cwd,
        &link::added_env(options),
    )
}

/// Pass on what the host relays as the events of link `id`: the program's
/// lines, its standard error (kept, the last of it, in `stderr`), its end;
/// another client taking this one's place; the connection ending with
/// neither told, which is the keeper gone (or the connection lost).
pub(crate) fn hear(
    id: u64,
    received: mpsc::Receiver<Received>,
    over: Arc<Over>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    events: mpsc::Sender<Event>,
    name: String,
) {
    let mut exited = false;
    let mut replaced = false;
    for said in received {
        let event = match said {
            Received::Line(line) => match serde_json::from_str(&line) {
                Ok(message @ serde_json::Value::Object(_)) => Event::Message(id, message),
                _ => {
                    log::warn!(
                        "{name} wrote a line that is not a message: {}",
                        link::clip(&line, 400)
                    );
                    continue;
                }
            },
            Received::Noise(text) => {
                log::warn!(
                    "{name} wrote a line that is not JSON: {}",
                    link::clip(&text, 400)
                );
                continue;
            }
            Received::Stderr(text) => {
                log::debug!("{name}: {text}");
                link::keep_last_word(&stderr, &text);
                continue;
            }
            Received::Exit(exit) => {
                exited = true;
                Event::Exited(id, exit.into())
            }
            Received::Replaced => {
                replaced = true;
                Event::Replaced(id)
            }
            Received::Closed => break,
        };
        if events.send(event).is_err() {
            break;
        }
    }
    over.set();
    if !exited && !replaced {
        let _ = events.send(Event::KeeperGone(id));
    }
    let _ = events.send(Event::StdoutClosed(id));
    let _ = events.send(Event::StderrClosed(id));
}

impl From<host::Exit> for Exit {
    fn from(exit: host::Exit) -> Self {
        match (exit.code, exit.signal) {
            (Some(code), _) => Exit::Code(code),
            (None, Some(signal)) => Exit::Signal(signal),
            (None, None) => Exit::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keeper_s_sockets_are_numbered_after_its_stem_highest_first() {
        let folder = tempfile::tempdir().unwrap();
        for name in [
            "s-1.sock",
            "s-3.sock",
            "s-2.sock.last",
            "s-x.sock",
            "t-4.sock",
            "s-10.sock",
        ] {
            std::fs::write(folder.path().join(name), "").unwrap();
        }
        let stem = folder.path().join("s");
        assert_eq!(sockets(&stem), [10, 3, 1]);
        let keeper = Keeper {
            host: "/bin/host".into(),
            host_args: vec!["claude-host".into()],
            stem: stem.clone(),
            idle: Duration::from_secs(1),
            away: Duration::from_secs(2),
        };
        let keep = keep_at(&keeper, 3);
        assert_eq!(keep.socket, folder.path().join("s-3.sock"));
        assert_eq!(keep.host_args, ["claude-host"]);
        // Nothing answers at any of them: none is found, and the next is
        // past the highest.
        let (found, next) = find(&keeper);
        assert!(found.is_none());
        assert_eq!(next, 11);
        assert!(sockets(&folder.path().join("none").join("s")).is_empty());
    }
}
