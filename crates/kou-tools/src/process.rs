mod capture;
pub use capture::{CapturePaths, CapturedProcessResult, run_captured_process};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group, test_kill_process_group},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const MAX_CAPTURE_BYTES: usize = 1_000_000;
const TICK: Duration = Duration::from_millis(20);
const IO_BUDGET: usize = 256 * 1024;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct ProcessRequest {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
    // None inherits, Some(empty) intentionally starts with an empty environment.
    pub environment: Option<BTreeMap<OsString, OsString>>,
    pub input: Option<Vec<u8>>,
    pub output_limit: usize,
    pub timeout: Option<Duration>,
    pub termination_grace: Duration,
}
impl ProcessRequest {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            directory: PathBuf::new(),
            environment: None,
            input: None,
            output_limit: 40_000,
            timeout: Some(Duration::from_secs(600)),
            termination_grace: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    #[serde(skip)]
    pub canceled: bool,
}
impl ProcessResult {
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        if !self.stdout.is_empty() {
            parts.push(self.stdout.clone());
        }
        if !self.stderr.is_empty() {
            parts.push(format!("Stderr:\n{}", self.stderr));
        }
        if self.exit_code != 0 {
            parts.push(format!("Exit code: {}", self.exit_code));
        }
        if self.timed_out {
            parts.push("The command timed out and was stopped.".into());
        }
        parts.join("\n")
    }
}

struct Invocation {
    child: Child,
    group: Pid,
    armed: bool,
}
impl Drop for Invocation {
    fn drop(&mut self) {
        if self.armed {
            let _ = signal(self.group, Signal::KILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A blocking worker owns the child and all three nonblocking pipes. No copier
/// thread can outlive this call. Even normal leader exit terminates descendants;
/// a descendant which creates a different session is outside this boundary.
pub fn run_process(request: &ProcessRequest, cancel: &CancelToken) -> io::Result<ProcessResult> {
    if !request.executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "start process: path must be absolute",
        ));
    }
    if request.output_limit == 0 || request.output_limit > MAX_CAPTURE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process output limit",
        ));
    }
    if cancel.is_canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "context canceled",
        ));
    }
    let started = Instant::now();
    let deadline = request
        .timeout
        .map(|timeout| {
            started.checked_add(timeout).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid process timeout")
            })
        })
        .transpose()?;
    let mut command = Command::new(&request.executable);
    command
        .args(&request.arguments)
        .process_group(0)
        .stdin(if request.input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !request.directory.as_os_str().is_empty() {
        command.current_dir(&request.directory);
    }
    if let Some(environment) = &request.environment {
        command.env_clear().envs(environment);
    }
    let child = command.spawn()?;
    let group = Pid::from_raw(child.id() as i32).expect("positive child PID");
    let mut invocation = Invocation {
        child,
        group,
        armed: true,
    };
    let mut stdout = invocation.child.stdout.take();
    let mut stderr = invocation.child.stderr.take();
    let mut stdin = invocation.child.stdin.take();
    if let Some(fd) = &stdout {
        nonblocking(fd)?;
    }
    if let Some(fd) = &stderr {
        nonblocking(fd)?;
    }
    if let Some(fd) = &stdin {
        nonblocking(fd)?;
    }
    let input = request.input.as_deref().unwrap_or_default();
    let mut input_at = 0;
    let mut out = Capture::new(request.output_limit);
    let mut err = Capture::new(request.output_limit);
    let mut status = None;
    let mut stopping = None;
    let mut timed_out = false;
    let mut canceled = false;
    loop {
        drain(&mut stdout, &mut out)?;
        drain(&mut stderr, &mut err)?;
        if status.is_none() {
            status = invocation.child.try_wait()?;
        }
        if stopping.is_none() {
            canceled = status.is_none() && cancel.is_canceled();
            timed_out =
                status.is_none() && !canceled && deadline.is_some_and(|at| Instant::now() >= at);
            if status.is_some() || canceled || timed_out {
                stdin.take();
                signal(group, Signal::TERM)?;
                stopping = Some(
                    Instant::now()
                        .checked_add(request.termination_grace)
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "invalid termination grace")
                        })?,
                );
            }
        }
        if let Some(until) = stopping {
            if status.is_some() && !group_exists(group)? {
                drain(&mut stdout, &mut out)?;
                drain(&mut stderr, &mut err)?;
                break;
            }
            if Instant::now() >= until {
                signal(group, Signal::KILL)?;
                if status.is_none() {
                    status = Some(invocation.child.wait()?);
                }
                // Pipes are nonblocking: an escaped descendant cannot hold
                // completion hostage by keeping them open or writing forever.
                drain(&mut stdout, &mut out)?;
                drain(&mut stderr, &mut err)?;
                break;
            }
        } else if let Some(pipe) = stdin.as_mut() {
            if input_at < input.len() {
                let end = input_at.saturating_add(32 * 1024).min(input.len());
                match pipe.write(&input[input_at..end]) {
                    Ok(n) => input_at += n,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                        stdin.take();
                    }
                    Err(e) => return Err(e),
                }
            }
            if input_at == input.len() {
                stdin.take();
            }
        }
        let mut fds = Vec::with_capacity(3);
        if let Some(fd) = &stdout {
            fds.push(PollFd::new(fd, PollFlags::IN));
        }
        if let Some(fd) = &stderr {
            fds.push(PollFd::new(fd, PollFlags::IN));
        }
        if let Some(fd) = &stdin {
            fds.push(PollFd::new(fd, PollFlags::OUT));
        }
        let now = Instant::now();
        let wait = stopping
            .or(deadline)
            .map_or(TICK, |at| at.saturating_duration_since(now).min(TICK));
        let timeout =
            Timespec::try_from(wait).map_err(|_| io::Error::other("invalid poll timeout"))?;
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    invocation.armed = false;
    let status = status.expect("completion reaps the leader");
    Ok(ProcessResult {
        stdout: out.render(),
        stderr: err.render(),
        exit_code: status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        timed_out,
        canceled,
    })
}

fn nonblocking(fd: &impl AsFd) -> io::Result<()> {
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
    Ok(())
}
fn signal(group: Pid, signal: Signal) -> io::Result<()> {
    match kill_process_group(group, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn group_exists(group: Pid) -> io::Result<bool> {
    match test_kill_process_group(group) {
        Ok(()) | Err(rustix::io::Errno::PERM) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn drain<R: Read>(reader: &mut Option<R>, output: &mut Capture) -> io::Result<()> {
    let Some(pipe) = reader.as_mut() else {
        return Ok(());
    };
    let mut bytes = [0; 32 * 1024];
    let mut used = 0;
    while used < IO_BUDGET {
        match pipe.read(&mut bytes) {
            Ok(0) => {
                reader.take();
                break;
            }
            Ok(n) => {
                output.write(&bytes[..n]);
                used += n;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

struct Capture {
    limit: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    at: usize,
    full: bool,
    total: u64,
}
impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(16),
            head: Vec::new(),
            tail: Vec::new(),
            at: 0,
            full: false,
            total: 0,
        }
    }
    fn write(&mut self, mut bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let n = bytes.len().min(self.limit - self.head.len());
        self.head.extend_from_slice(&bytes[..n]);
        bytes = &bytes[n..];
        if bytes.is_empty() {
            return;
        }
        if self.tail.is_empty() {
            self.tail.resize(self.limit, 0);
        }
        while !bytes.is_empty() {
            let n = bytes.len().min(self.limit - self.at);
            self.tail[self.at..self.at + n].copy_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            self.at += n;
            if self.at == self.limit {
                self.at = 0;
                self.full = true;
            }
        }
    }
    fn render(&self) -> String {
        if self.total <= self.head.len() as u64 {
            return valid_utf8(&self.head, true);
        }
        let ordered = if self.full {
            [&self.tail[self.at..], &self.tail[..self.at]].concat()
        } else {
            self.tail[..self.at].to_owned()
        };
        let mut head = &self.head[..(self.limit / 2).min(self.head.len())];
        let mut tail = &ordered[ordered.len().saturating_sub(self.limit - self.limit / 2)..];
        while !head.is_empty() && std::str::from_utf8(head).is_err() {
            head = &head[..head.len() - 1];
        }
        while !tail.is_empty() && tail[0] & 0xc0 == 0x80 {
            tail = &tail[1..];
        }
        format!(
            "{}...{} bytes truncated...{}",
            valid_utf8(head, false),
            self.total - head.len() as u64 - tail.len() as u64,
            valid_utf8(tail, false)
        )
    }
}

fn valid_utf8(mut bytes: &[u8], collapse: bool) -> String {
    let mut result = String::new();
    let mut in_invalid = false;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(rest) => {
                result.push_str(rest);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                result.push_str(std::str::from_utf8(&bytes[..good]).expect("validated prefix"));
                if good != 0 || !in_invalid || !collapse {
                    result.push('\u{fffd}');
                }
                in_invalid = true;
                bytes = &bytes[good + 1..];
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_ring_bounds_memory_and_keeps_chunk_independent_utf8() {
        let text = format!("начало{}конец", "中".repeat(10000));
        let mut once = Capture::new(400);
        once.write(text.as_bytes());
        let mut chunked = Capture::new(400);
        for part in text.as_bytes().chunks(7) {
            chunked.write(part);
        }
        assert_eq!(once.render(), chunked.render());
        assert!(once.render().ends_with("конец"));
        assert!(once.head.capacity() <= 800 && once.tail.capacity() <= 800);
        assert_eq!(valid_utf8(b"a\xff\xffb\xfe", true), "a�b�");
        assert_eq!(valid_utf8(b"a\xff\xffb\xfe", false), "a��b�");
    }
}
