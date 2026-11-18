//! Running an agent's program for an answer: with a deadline, and never
//! left behind. The program is ended, with whatever it started, as soon as
//! the answer is in or the time is up.

use crate::Ask;
use serde_json::Value;
use std::{
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// What the app's own environment has that the program asked must not take
/// for its own: the pane it would think it runs in, and the marks of an
/// agent's session the app itself may have been started from.
const NOT_PASSED_ON: [&str; 7] = [
    "KOU_PANE",
    "KOU_SESSION",
    "KOU_SOCKET",
    "KOU_BIN",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
];

fn command(ask: &Ask, args: &[&str]) -> Command {
    let mut command = Command::new(&ask.program);
    command.args(args);
    for name in NOT_PASSED_ON {
        command.env_remove(name);
    }
    command.envs(ask.env.iter().map(|(name, value)| (name, value)));
    // Not in a project: nothing of one is read, and nothing asked about one.
    command.current_dir(std::env::temp_dir());
    // A group of its own, so that what it starts ends with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

/// Ends the program and everything in its group, and waits for it.
fn end(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a signal to a process group this process started; the id
        // is the child's own, which has not been waited for yet.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A program spoken with in lines of JSON, until [`Ask::timeout`] is up.
pub(crate) struct Talk {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    deadline: Instant,
    late: bool,
}

impl Talk {
    pub(crate) fn start(ask: &Ask, args: &[&str]) -> io::Result<Self> {
        let mut child = command(ask, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (heard, lines) = mpsc::channel();
        if let Some(stdout) = stdout {
            // Ends when the program's output does: when it exits or is ended.
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if heard.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            lines,
            deadline: Instant::now() + ask.timeout,
            late: false,
        })
    }

    /// One message, on a line of its own.
    pub(crate) fn say(&mut self, message: &Value) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("the program's input is closed"))?;
        let mut line = message.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes())?;
        stdin.flush()
    }

    /// The next line that is JSON. `None` when the program has ended, or
    /// the time is up (see [`Talk::late`]).
    pub(crate) fn hear(&mut self) -> Option<Value> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                        return Some(message);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.late = true;
                    return None;
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Whether [`Talk::hear`] gave up because the time was up.
    pub(crate) fn late(&self) -> bool {
        self.late
    }
}

impl Drop for Talk {
    fn drop(&mut self) {
        self.stdin = None;
        end(&mut self.child);
    }
}

/// What a program printed and how it ended.
pub(crate) struct Printed {
    /// `None` when it was ended for taking too long.
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
}

/// Run the program once and take what it prints, for at most the ask's
/// timeout.
pub(crate) fn print(ask: &Ask, args: &[&str]) -> io::Result<Printed> {
    let mut child = command(ask, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let collect = |pipe: Option<Box<dyn io::Read + Send>>| {
        let (done, text) = mpsc::channel();
        std::thread::spawn(move || {
            let mut read = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut read);
            }
            let _ = done.send(read);
        });
        text
    };
    let out = collect(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let err = collect(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = Instant::now() + ask.timeout;
    let code = loop {
        match child.try_wait()? {
            Some(status) => break status.code(),
            None if Instant::now() >= deadline => {
                end(&mut child);
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };
    // The pipes close with the program; a moment covers what it left unread.
    let moment = Duration::from_millis(500);
    Ok(Printed {
        code,
        out: out.recv_timeout(moment).unwrap_or_default(),
        err: err.recv_timeout(moment).unwrap_or_default(),
    })
}
