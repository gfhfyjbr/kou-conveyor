use super::*;
use chrono::{SecondsFormat, Utc};
use kou_tools::{
    artifact::{self, Kind},
    process::{
        CancelToken, CapturePaths, CapturedEvent, ProcessRequest, run_captured_process_events,
    },
};
use std::{
    io,
    time::{Duration, Instant, SystemTime},
};

/// Run on a caller-owned worker. A checkpoint is emitted before its dispatch;
/// durability is acknowledged only if `publish` supplies that acknowledgement.
/// This call creates no threads and owns/reaps its process group. Blocking
/// callbacks cannot be preempted; output spool disk is not capped.
pub fn run_shell(
    current: Operation,
    cancel: &CancelToken,
    publish: impl FnMut(Operation) -> io::Result<()>,
) -> io::Result<()> {
    run_with_clock(current, cancel, publish, system_now)
}
fn system_now() -> io::Result<Timestamp> {
    let now = chrono::DateTime::<Utc>::from(SystemTime::now());
    Timestamp::parse(&now.to_rfc3339_opts(SecondsFormat::Nanos, true)).map_err(io::Error::other)
}
fn run_with_clock(
    current: Operation,
    cancel: &CancelToken,
    publish: impl FnMut(Operation) -> io::Result<()>,
    clock: impl FnMut() -> io::Result<Timestamp>,
) -> io::Result<()> {
    let mut driver = Driver {
        shell: Shell::new(current).map_err(io::Error::other)?,
        cancel,
        publish,
        clock,
        started: None,
        owned_pid: None,
        checkpoints: 0,
    };
    let step = driver.reduce(None)?;
    driver.drive(step)
}
struct Driver<'a, P, C> {
    shell: Shell,
    cancel: &'a CancelToken,
    publish: P,
    clock: C,
    started: Option<Instant>,
    owned_pid: Option<u32>,
    checkpoints: usize,
}
impl<P, C> Driver<'_, P, C>
where
    P: FnMut(Operation) -> io::Result<()>,
    C: FnMut() -> io::Result<Timestamp>,
{
    fn reduce(&mut self, event: Option<ShellEvent>) -> io::Result<ShellStep> {
        // Go's in-memory time.Time retains its monotonic component. Recovered
        // state does not, and correctly falls back to timestamp subtraction.
        self.shell.elapsed_seconds = self
            .started
            .map(|start| start.elapsed().as_nanos().min(i64::MAX as u128) as f64 / 1e9);
        self.shell
            .handle_at(event, (self.clock)()?)
            .map_err(io::Error::other)
    }
    fn event(&self, kind: &str, correlation: &str, result: ShellEventResult) -> ShellEvent {
        ShellEvent {
            kind: kind.into(),
            source: self.shell.current.id.clone(),
            correlation_id: correlation.into(),
            result,
        }
    }
    fn emit(&mut self, current: Option<Operation>) -> io::Result<()> {
        if let Some(current) = current {
            if self.checkpoints >= MAX_CHECKPOINTS {
                return Err(io::Error::other(
                    "shell checkpoint limit exceeded; the effect may already have occurred",
                ));
            }
            (self.publish)(current)?;
            self.checkpoints += 1;
        }
        Ok(())
    }
    fn canceled(&mut self) -> io::Result<ShellStep> {
        self.reduce(Some(self.event(
            "primitive.canceled",
            "",
            ShellEventResult::Empty,
        )))
    }
    fn failure(
        &mut self,
        correlation: &str,
        action: &str,
        error: io::Error,
    ) -> io::Result<ShellStep> {
        if self.cancel.is_canceled() {
            self.canceled()
        } else {
            self.reduce(Some(self.event(
                "primitive.failed",
                correlation,
                ShellEventResult::Failure(format!("{action}: {error}")),
            )))
        }
    }
    fn drive(&mut self, mut step: ShellStep) -> io::Result<()> {
        loop {
            if self.cancel.is_canceled() && !self.shell.current.terminal() {
                step = self.canceled()?;
            }
            self.emit(step.operation.take())?;
            if self.shell.current.terminal() {
                return Ok(());
            }
            if self.cancel.is_canceled() {
                step = self.canceled()?;
                continue;
            }
            if step.dispatches.len() != 1 {
                return Err(io::Error::other("native shell expected one owned dispatch"));
            }
            step = match step.dispatches.pop().unwrap() {
                ShellDispatch::Create(request) => self.create(request)?,
                ShellDispatch::Read(request) => self.read(request)?,
                ShellDispatch::Start(request) => self.process(request)?,
                _ => {
                    return Err(io::Error::other(
                        "native shell received a signal/timer outside its owned process invocation",
                    ));
                }
            };
        }
    }
    fn create(&mut self, request: CreateRequest) -> io::Result<ShellStep> {
        let kind = match request.kind {
            1 => Kind::File,
            2 => Kind::Directory,
            _ => {
                return Err(io::Error::other(
                    "native shell received an invalid artifact kind",
                ));
            }
        };
        match artifact::create(
            std::path::Path::new(&request.path),
            kind,
            request.mode,
            self.cancel,
        ) {
            Ok(_) if self.cancel.is_canceled() => self.canceled(),
            Ok(_) => self.reduce(Some(self.event(
                "io.create_completed",
                &request.correlation_id,
                ShellEventResult::Created { kind: request.kind },
            ))),
            Err(error) => self.failure(&request.correlation_id, "create shell artifact", error),
        }
    }
    fn read(&mut self, request: ReadRequest) -> io::Result<ShellStep> {
        let mut callback_failed = false;
        let cancel = self.cancel;
        let result = artifact::read_capture(
            std::path::Path::new(&request.path),
            request.offset,
            request.count,
            cancel,
            |offset, data| {
                let result = (|| {
                    let step = self.reduce(Some(self.event(
                        "io.read_output",
                        &request.correlation_id,
                        ShellEventResult::ReadOutput {
                            offset,
                            data: data.to_vec(),
                        },
                    )))?;
                    if step.operation.is_some() || !step.dispatches.is_empty() {
                        return Err(io::Error::other(
                            "native shell read chunk produced an unexpected checkpoint/dispatch",
                        ));
                    }
                    Ok(())
                })();
                callback_failed = result.is_err();
                result
            },
        );
        if callback_failed {
            return Err(result
                .err()
                .unwrap_or_else(|| io::Error::other("native shell read callback failed")));
        }
        match result {
            Ok(_) if self.cancel.is_canceled() => self.canceled(),
            Ok(result) => self.reduce(Some(self.event(
                "io.read_completed",
                &request.correlation_id,
                ShellEventResult::ReadCompleted { size: result.size },
            ))),
            Err(error) => self.failure(&request.correlation_id, "read shell capture", error),
        }
    }
    fn process(&mut self, request: StartRequest) -> io::Result<ShellStep> {
        let mut process = ProcessRequest::new(&request.path);
        process.arguments = request.arguments.into_iter().map(Into::into).collect();
        if !request.directory.is_empty() {
            process.directory = request.directory.into();
        }
        process.timeout = command_timeout(self.shell.state.input.timeout);
        process.termination_grace = Duration::from_secs(5);
        process.output_limit = 0;
        let mut callback_failed = false;
        let cancel = self.cancel;
        let result = run_captured_process_events(
            &process,
            &CapturePaths {
                stdout: request.stdout_path.into(),
                stderr: request.stderr_path.into(),
            },
            cancel,
            |event| {
                let result = self.process_event(event);
                callback_failed = result.is_err();
                result
            },
        );
        self.owned_pid = None;
        if callback_failed {
            return Err(result
                .err()
                .unwrap_or_else(|| io::Error::other("native shell process callback failed")));
        }
        match result {
            Ok(_) if self.cancel.is_canceled() => self.canceled(),
            Ok(result) => self.reduce(Some(self.event(
                "process.exited",
                &request.correlation_id,
                ShellEventResult::Exited {
                    exit_code: result.exit_code as i64,
                    signal: result.signal as i64,
                },
            ))),
            Err(error) => self.failure(&request.correlation_id, "run shell process", error),
        }
    }
    fn process_event(&mut self, event: CapturedEvent) -> io::Result<()> {
        let (step, timed_out, pid) = match event {
            CapturedEvent::Started { pid } => {
                if self.owned_pid.is_some() {
                    return Err(io::Error::other("duplicate native process start"));
                }
                self.owned_pid = Some(pid);
                self.started = Some(Instant::now());
                (
                    self.reduce(Some(self.event(
                        "process.started",
                        "process",
                        ShellEventResult::Started { pid: pid as i64 },
                    )))?,
                    false,
                    pid,
                )
            }
            CapturedEvent::TimedOut { pid } => {
                if self.owned_pid != Some(pid) {
                    return Err(io::Error::other(
                        "native shell timeout does not belong to the active invocation",
                    ));
                }
                (
                    self.reduce(Some(self.event(
                        "timer.fired",
                        "timeout",
                        ShellEventResult::Empty,
                    )))?,
                    true,
                    pid,
                )
            }
        };
        if timed_out {
            if !matches!(step.dispatches.as_slice(), [ShellDispatch::Signal { group_id, signal: 15 }, ShellDispatch::Timer(timer)] if *group_id == pid as i64 && timer.correlation_id == "kill" && timer.source == self.shell.current.id)
            {
                return Err(io::Error::other(
                    "native shell timeout produced unowned control dispatches",
                ));
            }
        } else if self.shell.state.input.timeout > 0.0 {
            if !matches!(step.dispatches.as_slice(), [ShellDispatch::Timer(timer)] if timer.correlation_id == "timeout" && timer.source == self.shell.current.id)
            {
                return Err(io::Error::other(
                    "native shell start produced invalid timeout dispatches",
                ));
            }
        } else if !step.dispatches.is_empty() {
            return Err(io::Error::other(
                "native shell start unexpectedly scheduled controls",
            ));
        }
        // The capture controller applies TERM/KILL to this same owned invocation
        // after this callback. No numeric PID from durable state is ever signaled.
        self.emit(step.operation)
    }
}

fn command_timeout(seconds: f64) -> Option<Duration> {
    // Shell state validation bounds seconds; Go truncates rather than rounding.
    (seconds > 0.0).then(|| Duration::from_nanos((seconds * 1e9) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_timeout_truncates_subnanosecond_fractions_like_go() {
        assert_eq!(
            command_timeout(0.1250000009),
            Some(Duration::from_nanos(125_000_000))
        );
        assert_eq!(command_timeout(0.0000000009), Some(Duration::ZERO));
        assert_eq!(command_timeout(0.0), None);
    }
    #[test]
    fn native_duration_uses_monotonic_time_across_a_backward_wall_clock_jump() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let current = shell_spec(
            ShellInput {
                shell: "/bin/sh".into(),
                command: "printf output".into(),
                ..ShellInput::default()
            },
            root.to_string_lossy().into(),
            40,
        )
        .unwrap()
        .operation("clocked".into());
        let initial = Timestamp::parse("2026-11-10T12:00:00Z").unwrap();
        let backward = Timestamp::parse("1900-01-01T00:00:00Z").unwrap();
        let mut calls = 0;
        let mut result = None;
        run_with_clock(
            current,
            &CancelToken::default(),
            |current| {
                if current.terminal() {
                    assert_eq!(current.status, "completed");
                    let state = decode_shell(&current).unwrap();
                    assert_eq!(state.started_at, initial);
                    result = state.result;
                }
                Ok(())
            },
            || {
                calls += 1;
                Ok(if calls <= 5 { initial } else { backward })
            },
        )
        .unwrap();
        let result = result.unwrap();
        assert_eq!(result.out, "output");
        assert!(result.duration > 0.0 && result.duration < 5.0);
        assert!(calls > 5);
    }
}
