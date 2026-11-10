use super::{CancelToken, Invocation, ProcessRequest, TICK, group_exists, nonblocking, signal};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal},
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

#[derive(Clone, Debug)]
pub struct CapturePaths {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}

#[derive(Debug)]
pub struct CapturedProcessResult {
    /// Raw exit status: -1 when terminated by a signal, not 128 + signal.
    pub exit_code: i32,
    /// Zero for an ordinary exit. Output bodies remain in the capture files.
    pub signal: i32,
    pub timed_out: bool,
    pub canceled: bool,
}

struct Captures {
    stdout: File,
    stderr: File,
}
impl Captures {
    fn open(paths: &CapturePaths) -> io::Result<Self> {
        for (stream, path) in [("stdout", &paths.stdout), ("stderr", &paths.stderr)] {
            if !path.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("start process: {stream} capture path must be absolute"),
                ));
            }
        }
        if paths.stdout == paths.stderr {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "start process: stdout and stderr capture paths must differ",
            ));
        }
        let stdout = open(&paths.stdout, "stdout")?;
        let stderr = open(&paths.stderr, "stderr")?;
        let out = stdout.metadata()?;
        let err = stderr.metadata()?;
        if out.dev() == err.dev() && out.ino() == err.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdout and stderr capture paths identify the same file",
            ));
        }
        // Validate both descriptors and their identity before truncating either.
        stdout.set_len(0)?;
        stderr.set_len(0)?;
        Ok(Self { stdout, stderr })
    }
    fn sync(&self) -> io::Result<()> {
        let mut errors = Vec::new();
        for (stream, file) in [("stdout", &self.stdout), ("stderr", &self.stderr)] {
            if let Err(error) = file.sync_all() {
                errors.push(format!("sync process {stream} capture: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("\n")))
        }
    }
}
fn open(path: &Path, stream: &str) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("open process {stream} capture {path:?}: {error}"),
            )
        })?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("open process {stream} capture {path:?}: path is not a regular file"),
        ));
    }
    fcntl_setfl(&file, fcntl_getfl(&file)? & !OFlags::NONBLOCK)?;
    Ok(file)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedEvent {
    Started {
        pid: u32,
    },
    /// Emitted before TERM; the backend still owns termination and escalation.
    TimedOut {
        pid: u32,
    },
}

/// Capture directly into existing regular files, preserving the child's stdout
/// descriptor type. No output body is retained in RAM; spool disk is not capped.
/// A failed/panicking start callback kills and reaps the owned process invocation.
pub fn run_captured_process(
    request: &ProcessRequest,
    paths: &CapturePaths,
    cancel: &CancelToken,
    on_started: impl FnOnce(u32) -> io::Result<()>,
) -> io::Result<CapturedProcessResult> {
    let mut on_started = Some(on_started);
    run_captured_process_events(request, paths, cancel, |event| match event {
        CapturedEvent::Started { pid } => on_started.take().expect("one process start")(pid),
        CapturedEvent::TimedOut { .. } => Ok(()),
    })
}

/// Callbacks run on the owned worker, are not preemptible and must return.
/// Callback failure/panic kills and reaps the invocation before unwinding out.
pub fn run_captured_process_events(
    request: &ProcessRequest,
    paths: &CapturePaths,
    cancel: &CancelToken,
    mut on_event: impl FnMut(CapturedEvent) -> io::Result<()>,
) -> io::Result<CapturedProcessResult> {
    if !request.executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "start process: path must be absolute",
        ));
    }
    if cancel.is_canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "context canceled",
        ));
    }
    if request
        .timeout
        .is_some_and(|timeout| Instant::now().checked_add(timeout).is_none())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process timeout",
        ));
    }
    if Instant::now()
        .checked_add(request.termination_grace)
        .is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid termination grace",
        ));
    }
    let captures = Captures::open(paths)?;
    if cancel.is_canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "context canceled",
        ));
    }
    let mut command = Command::new(&request.executable);
    command
        .args(&request.arguments)
        .process_group(0)
        .stdin(if request.input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(captures.stdout.try_clone()?))
        .stderr(Stdio::from(captures.stderr.try_clone()?));
    if !request.directory.as_os_str().is_empty() {
        command.current_dir(&request.directory);
    }
    if let Some(environment) = &request.environment {
        command.env_clear().envs(environment);
    }
    let child = command.spawn()?;
    let process_id = child.id();
    let group = Pid::from_raw(process_id as i32).expect("positive child PID");
    let mut invocation = Invocation {
        child,
        group,
        armed: true,
    };
    let mut stdin = invocation.child.stdin.take();
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    let deadline = request
        .timeout
        .map(|timeout| {
            Instant::now().checked_add(timeout).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid process timeout")
            })
        })
        .transpose()?;
    on_event(CapturedEvent::Started { pid: process_id })?;
    let input = request.input.as_deref().unwrap_or_default();
    let mut input_at = 0;
    let mut status = None;
    let mut stopping = None;
    let mut timed_out = false;
    let mut canceled = false;
    loop {
        if status.is_none() {
            status = invocation.child.try_wait()?;
        }
        if stopping.is_none() {
            canceled = status.is_none() && cancel.is_canceled();
            timed_out =
                status.is_none() && !canceled && deadline.is_some_and(|at| Instant::now() >= at);
            if status.is_some() || canceled || timed_out {
                if timed_out {
                    on_event(CapturedEvent::TimedOut { pid: process_id })?;
                }
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
                break;
            }
            if Instant::now() >= until {
                signal(group, Signal::KILL)?;
                if status.is_none() {
                    status = Some(invocation.child.wait()?);
                }
                break;
            }
        } else if let Some(pipe) = stdin.as_mut() {
            if input_at < input.len() {
                let end = input_at.saturating_add(32 * 1024).min(input.len());
                match pipe.write(&input[input_at..end]) {
                    Ok(count) => input_at += count,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                        stdin.take();
                    }
                    Err(error) => return Err(error),
                }
            }
            if input_at == input.len() {
                stdin.take();
            }
        }
        let mut descriptors = Vec::with_capacity(1);
        if let Some(pipe) = &stdin {
            descriptors.push(PollFd::new(pipe, PollFlags::OUT));
        }
        let wait = stopping.or(deadline).map_or(TICK, |at| {
            at.saturating_duration_since(Instant::now()).min(TICK)
        });
        let timeout =
            Timespec::try_from(wait).map_err(|_| io::Error::other("invalid poll timeout"))?;
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
    stdin.take();
    invocation.armed = false;
    captures.sync()?;
    let status = status.expect("completion reaps the leader");
    Ok(CapturedProcessResult {
        exit_code: status.code().unwrap_or(-1),
        signal: status.signal().unwrap_or(0),
        timed_out,
        canceled,
    })
}
