//! Shell v3 owns typed durable state and a bounded, non-durable read accumulator.
//! Handle is pure: commit its checkpoint before executing returned dispatches.
mod execute;
mod output;
mod wire;
use super::{Error, Result, Spec, decode_state, encode, null_default};
use chrono::{DateTime, TimeDelta};
pub use execute::run_shell;
use kou_domain::{Json, Operation, Timestamp};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
pub use wire::*;

pub const MAX_OUTPUT_LENGTH: i64 = 1_000_000;

pub fn shell_spec(
    input: ShellInput,
    base_directory: String,
    max_output_length: i64,
) -> Result<Spec> {
    validate_limit(max_output_length)?;
    let state = ShellState {
        input,
        base_directory,
        out_truncated: true,
        err_truncated: true,
        ..ShellState::default()
    };
    state.validate()?;
    Ok(Spec {
        kind: "shell".into(),
        version: 3,
        max_output_length,
        state: encode(&state)?,
        idempotency: Json::default(),
    })
}
pub fn decode_shell(current: &Operation) -> Result<ShellState> {
    if current.kind != "shell" {
        return Err(Error(format!(
            "advance shell operation {:?}: unsupported type {:?}: unsupported operation",
            current.id, current.kind
        )));
    }
    if current.version != 3 {
        return Err(Error(format!(
            "advance shell operation {:?}: unsupported version {}: unsupported operation",
            current.id, current.version
        )));
    }
    validate_limit(current.max_output_length)?;
    decode_state(&current.state, "ShellState").map_err(|error| {
        let detail = if error.0.starts_with("illegal character ") {
            wire::binary_error(&current.state).unwrap_or(error.0)
        } else {
            error.0
        };
        Error(format!(
            "decode shell operation {:?} state: {detail}",
            current.id
        ))
    })
}
fn validate_limit(limit: i64) -> Result<()> {
    if !(1..=MAX_OUTPUT_LENGTH).contains(&limit) {
        return Err(Error("max output length is out of range".into()));
    }
    Ok(())
}
struct Paths {
    directory: String,
    out: String,
    err: String,
}
fn paths(base: &str, id: &str) -> Result<Paths> {
    if id.is_empty() || matches!(id, "." | "..") || id.contains('/') {
        return Err(Error(format!(
            "operation ID {id:?} is not a path component"
        )));
    }
    let mut clean = PathBuf::new();
    for part in Path::new(base).join(id).components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            _ => clean.push(part.as_os_str()),
        }
    }
    Ok(Paths {
        directory: clean.to_string_lossy().into(),
        out: clean.join("out").to_string_lossy().into(),
        err: clean.join("err").to_string_lossy().into(),
    })
}

pub struct Shell {
    current: Operation,
    state: ShellState,
    read: Vec<u8>,
    // Supplied by an owned native invocation; never serialized or recovered.
    elapsed_seconds: Option<f64>,
}
impl Shell {
    pub fn new(mut current: Operation) -> Result<Self> {
        let state = decode_shell(&current)?;
        state.validate().map_err(|error| {
            Error(format!(
                "validate shell operation {:?} state: {error}",
                current.id
            ))
        })?;
        let limit = current.max_output_length as usize * 4;
        if state
            .inline_out
            .len()
            .saturating_add(state.inline_out_tail.len())
            > limit
            || state
                .inline_err
                .len()
                .saturating_add(state.inline_err_tail.len())
                > limit
        {
            return Err(Error(
                "inline shell result exceeds the configured limit".into(),
            ));
        }
        current.state = Json::default();
        Ok(Self {
            current,
            state,
            read: Vec::new(),
            elapsed_seconds: None,
        })
    }
    pub fn state(&self) -> &ShellState {
        &self.state
    }
    pub fn handle_at(&mut self, event: Option<ShellEvent>, at: Timestamp) -> Result<ShellStep> {
        match self.current.status.as_str() {
            "ready" => {
                if event.is_some() {
                    return Err(Error(format!(
                        "advance ready shell operation {:?}: unexpected primitive event",
                        self.current.id
                    )));
                }
                if !self.state.phase.is_empty()
                    || self.state.result.is_some()
                    || !self.state.terminal_error.is_empty()
                {
                    return Err(Error(format!(
                        "advance ready shell operation {:?}: state is not initial",
                        self.current.id
                    )));
                }
                match paths(&self.state.base_directory, &self.current.id) {
                    Ok(paths) => self.next("create_directory", paths),
                    Err(error) => self.fail(error.0),
                }
            }
            "awaiting" => match event {
                Some(event) => self.event(event, at),
                None => self.resume(),
            },
            "canceling" => {
                if event.is_some() {
                    return Err(Error(format!(
                        "advance canceling shell operation {:?}: unexpected primitive event",
                        self.current.id
                    )));
                }
                self.cancel()
            }
            _ => Err(Error(format!(
                "advance shell operation {:?}: terminal status {:?}",
                self.current.id, self.current.status
            ))),
        }
    }
    fn resume(&mut self) -> Result<ShellStep> {
        if self.state.phase == "process" {
            return self.fail(
                if self.state.process_group_id == 0 {
                    "shell execution outcome is unknown because process start was not recorded"
                } else {
                    "shell execution was interrupted before an exit status was recorded"
                }
                .into(),
            );
        }
        self.read = Vec::new();
        match paths(&self.state.base_directory, &self.current.id) {
            Ok(paths) => self.dispatch_phase(paths),
            Err(error) => self.fail(error.0),
        }
    }
    fn next(&mut self, phase: &str, paths: Paths) -> Result<ShellStep> {
        self.state.phase = phase.into();
        self.dispatch_phase(paths)
    }
    fn dispatch_phase(&mut self, paths: Paths) -> Result<ShellStep> {
        let source = self.current.id.clone();
        let correlation_id = self.state.phase.clone();
        let dispatch = match self.state.phase.as_str() {
            "create_directory" => ShellDispatch::Create(CreateRequest {
                source,
                correlation_id,
                kind: 2,
                path: paths.directory,
                mode: 0o700,
            }),
            "create_out" | "create_err" => ShellDispatch::Create(CreateRequest {
                source,
                correlation_id,
                kind: 1,
                path: if self.state.phase == "create_out" {
                    paths.out
                } else {
                    paths.err
                },
                mode: 0o600,
            }),
            "process" => ShellDispatch::Start(StartRequest {
                source,
                correlation_id,
                path: self.state.input.shell.clone(),
                arguments: vec!["-c".into(), self.state.input.command.clone()],
                directory: self.state.input.directory.clone(),
                environment: Vec::new(),
                pipes: 0,
                stdout_path: paths.out,
                stderr_path: paths.err,
            }),
            "read_out" | "read_err" | "read_out_tail" | "read_err_tail" => {
                match self.read_request(&paths) {
                    Ok(request) => ShellDispatch::Read(request),
                    Err(error) => return self.fail(error.0),
                }
            }
            _ => {
                return self.fail(format!(
                    "shell operation has invalid phase {:?}",
                    self.state.phase
                ));
            }
        };
        let mut step = self.await_()?;
        step.dispatches.push(dispatch);
        Ok(step)
    }
    fn event(&mut self, event: ShellEvent, at: Timestamp) -> Result<ShellStep> {
        if event.source != self.current.id {
            return self.fail(format!(
                "shell primitive event source is {:?}, want {:?}",
                event.source, self.current.id
            ));
        }
        if matches!(event.correlation_id.as_str(), "timeout" | "kill") {
            return self.timer_event(event, at);
        }
        if event.kind == "primitive.canceled" {
            return self.cancel();
        }
        if event.kind == "primitive.failed" {
            return self.fail(match event.result {
                ShellEventResult::Failure(error) => error,
                _ => "shell primitive failed with an invalid result".into(),
            });
        }
        if event.correlation_id != self.state.phase {
            return self.fail(format!(
                "shell primitive event correlation is {:?}, want {:?}",
                event.correlation_id, self.state.phase
            ));
        }
        let paths = match paths(&self.state.base_directory, &self.current.id) {
            Ok(paths) => paths,
            Err(error) => return self.fail(error.0),
        };
        match self.state.phase.as_str() {
            "create_directory" | "create_out" | "create_err" => self.created(event, paths),
            "process" => self.process(event, paths, at),
            "read_out" | "read_err" | "read_out_tail" | "read_err_tail" => {
                self.read_event(event, paths, at)
            }
            _ => self.fail(format!(
                "shell operation has invalid phase {:?}",
                self.state.phase
            )),
        }
    }
    fn created(&mut self, event: ShellEvent, paths: Paths) -> Result<ShellStep> {
        let expected = if self.state.phase == "create_directory" {
            2
        } else {
            1
        };
        if event.kind != "io.create_completed"
            || !matches!(event.result, ShellEventResult::Created { kind } if kind == expected)
        {
            return self.fail("create shell artifact returned an invalid result".into());
        }
        match self.state.phase.as_str() {
            "create_directory" => self.next("create_out", paths),
            "create_out" => {
                self.state.out_path = paths.out.clone();
                self.next("create_err", paths)
            }
            "create_err" => {
                self.state.err_path = paths.err.clone();
                self.next("process", paths)
            }
            _ => unreachable!("create phase checked"),
        }
    }
    fn process(&mut self, event: ShellEvent, paths: Paths, at: Timestamp) -> Result<ShellStep> {
        match event.kind.as_str() {
            "process.started" => {
                let ShellEventResult::Started { pid } = event.result else {
                    return self.fail("start shell returned an invalid result".into());
                };
                if pid <= 1 {
                    return self.fail("start shell returned an invalid result".into());
                }
                self.state.process_group_id = pid;
                self.state.started_at = at;
                let mut step = self.await_()?;
                if self.state.input.timeout > 0.0 {
                    step.dispatches.push(self.timer(
                        "timeout",
                        at,
                        (self.state.input.timeout * 1e9) as i64,
                    )?);
                }
                Ok(step)
            }
            "process.exited" => {
                let ShellEventResult::Exited { exit_code, signal } = event.result else {
                    return self.fail("shell returned an invalid exit result".into());
                };
                self.state.pending_exit_code = Some(if signal != 0 {
                    128i64.saturating_add(signal)
                } else {
                    exit_code
                });
                self.state.process_group_id = 0;
                self.next("read_out", paths)
            }
            "process.output" => {
                self.fail("captured shell process produced an unexpected output event".into())
            }
            "process.stream_failed" => {
                self.fail("captured shell process returned an unexpected stream failure".into())
            }
            _ => self.fail(format!(
                "shell process returned unexpected event {:?}",
                event.kind
            )),
        }
    }
    fn timer(&self, correlation: &str, at: Timestamp, delay_ns: i64) -> Result<ShellDispatch> {
        let now = DateTime::parse_from_rfc3339(&at.to_string())
            .map_err(|error| Error(error.to_string()))?;
        let end = now
            .checked_add_signed(TimeDelta::nanoseconds(delay_ns))
            .ok_or_else(|| Error("shell timer deadline is out of range".into()))?;
        let deadline =
            Timestamp::parse(&end.to_rfc3339()).map_err(|error| Error(error.to_string()))?;
        Ok(ShellDispatch::Timer(TimerRequest {
            source: self.current.id.clone(),
            correlation_id: correlation.into(),
            deadline,
        }))
    }
    fn timer_event(&mut self, event: ShellEvent, at: Timestamp) -> Result<ShellStep> {
        if event.kind != "timer.fired"
            || self.state.phase != "process"
            || self.state.process_group_id == 0
        {
            return Ok(ShellStep::default());
        }
        let signal = ShellDispatch::Signal {
            group_id: self.state.process_group_id,
            signal: if event.correlation_id == "timeout" {
                15
            } else {
                9
            },
        };
        if event.correlation_id == "timeout" {
            self.state.timed_out = true;
            let mut step = self.checkpoint()?;
            step.dispatches = vec![signal, self.timer("kill", at, 5_000_000_000)?];
            Ok(step)
        } else {
            Ok(ShellStep {
                operation: None,
                dispatches: vec![signal],
            })
        }
    }
    fn read_limit(&self) -> i64 {
        self.current.max_output_length * 4
    }
    fn tail_limit(&self) -> i64 {
        (self.current.max_output_length - self.current.max_output_length / 2) * 4
    }
    fn read_request(&self, paths: &Paths) -> Result<ReadRequest> {
        let (path, size) = match self.state.phase.as_str() {
            "read_out" | "read_out_tail" => (paths.out.clone(), self.state.out_size),
            "read_err" | "read_err_tail" => (paths.err.clone(), self.state.err_size),
            _ => {
                return Err(Error(format!(
                    "shell operation has invalid read phase {:?}",
                    self.state.phase
                )));
            }
        };
        let (offset, count) = if self.state.phase.ends_with("_tail") {
            if size <= self.read_limit() {
                return Err(Error("invalid shell tail read size".into()));
            }
            (size - self.tail_limit(), self.tail_limit())
        } else {
            (0, self.read_limit())
        };
        Ok(ReadRequest {
            source: self.current.id.clone(),
            correlation_id: self.state.phase.clone(),
            path,
            offset,
            count,
        })
    }
    fn read_event(&mut self, event: ShellEvent, paths: Paths, at: Timestamp) -> Result<ShellStep> {
        let request = match self.read_request(&paths) {
            Ok(request) => request,
            Err(error) => return self.fail(error.0),
        };
        let read_size = self.read.len() as i64;
        match event.kind.as_str() {
            "io.read_output" => {
                let ShellEventResult::ReadOutput { offset, data } = event.result else {
                    return self.fail("shell read returned invalid output".into());
                };
                if request.offset.checked_add(read_size) != Some(offset) {
                    return self.fail("shell read returned invalid output".into());
                }
                if data.len() as i64 > request.count - read_size {
                    return self.fail("shell read exceeded its configured limit".into());
                }
                self.read.extend(data);
                Ok(ShellStep::default())
            }
            "io.read_completed" => {
                let ShellEventResult::ReadCompleted { size } = event.result else {
                    return self.fail("shell read returned an invalid completion".into());
                };
                if size < request.offset || read_size != request.count.min(size - request.offset) {
                    return self.fail("shell read returned an invalid completion".into());
                }
                if self.state.phase.ends_with("_tail") && size != request.offset + request.count {
                    return self.fail("shell capture size changed while reading its tail".into());
                }
                let data = std::mem::take(&mut self.read);
                self.read_completed(data, size, paths, at)
            }
            _ => self.fail(format!(
                "shell read returned unexpected event {:?}",
                event.kind
            )),
        }
    }
    fn read_completed(
        &mut self,
        mut data: Vec<u8>,
        size: i64,
        paths: Paths,
        at: Timestamp,
    ) -> Result<ShellStep> {
        let tail = size > self.read_limit();
        if tail && matches!(self.state.phase.as_str(), "read_out" | "read_err") {
            data.truncate((self.read_limit() - self.tail_limit()) as usize);
        }
        match self.state.phase.as_str() {
            "read_out" => {
                self.state.inline_out = data;
                self.state.out_size = size;
                self.state.inline_out_tail.clear();
                self.next(if tail { "read_out_tail" } else { "read_err" }, paths)
            }
            "read_out_tail" => {
                self.state.inline_out_tail = data;
                self.next("read_err", paths)
            }
            "read_err" => {
                self.state.inline_err = data;
                self.state.err_size = size;
                self.state.inline_err_tail.clear();
                if tail {
                    self.next("read_err_tail", paths)
                } else {
                    self.finish(paths, at)
                }
            }
            "read_err_tail" => {
                self.state.inline_err_tail = data;
                self.finish(paths, at)
            }
            _ => unreachable!("read phase checked"),
        }
    }
    fn finish(&mut self, paths: Paths, at: Timestamp) -> Result<ShellStep> {
        let Some(exit_code) = self.state.pending_exit_code else {
            return self.fail("shell execution completed without an exit status".into());
        };
        let (out, out_truncated) = output::bound(
            &self.state.inline_out,
            &self.state.inline_out_tail,
            self.state.out_size,
            self.current.max_output_length as usize,
            &paths.out,
        )?;
        let (err, err_truncated) = output::bound(
            &self.state.inline_err,
            &self.state.inline_err_tail,
            self.state.err_size,
            self.current.max_output_length as usize,
            &paths.err,
        )?;
        let duration = if self.state.started_at.is_zero() {
            0.0
        } else if let Some(duration) = self.elapsed_seconds {
            duration
        } else {
            elapsed(at, self.state.started_at)?
        };
        self.state.result = Some(ShellResult {
            out,
            err,
            out_size: self.state.out_size,
            err_size: self.state.err_size,
            exit_code,
            timed_out: self.state.timed_out,
            duration,
        });
        self.state.out_truncated = out_truncated;
        self.state.err_truncated = err_truncated;
        self.state.out_path = paths.out;
        self.state.err_path = paths.err;
        self.state.pending_exit_code = None;
        self.state.inline_out = Vec::new();
        self.state.inline_err = Vec::new();
        self.state.inline_out_tail = Vec::new();
        self.state.inline_err_tail = Vec::new();
        self.state.phase.clear();
        self.current.status = "completed".into();
        self.checkpoint()
    }
    fn await_(&mut self) -> Result<ShellStep> {
        self.current.status = "awaiting".into();
        self.checkpoint()
    }
    fn checkpoint(&mut self) -> Result<ShellStep> {
        if !self.state.error_truncated {
            let (text, truncated) = output::bound(
                self.state.terminal_error.as_bytes(),
                &[],
                self.state.terminal_error.len() as i64,
                self.current.max_output_length as usize,
                "",
            )?;
            self.state.terminal_error = text;
            self.state.error_truncated = truncated;
        }
        let mut current = self.current.clone();
        current.state = encode(&self.state).map_err(|error| {
            Error(format!(
                "encode shell operation {:?} state: {error}",
                current.id
            ))
        })?;
        Ok(ShellStep {
            operation: Some(current),
            dispatches: Vec::new(),
        })
    }
    fn fail(&mut self, error: String) -> Result<ShellStep> {
        self.read = Vec::new();
        self.state.phase.clear();
        self.state.terminal_error = error;
        self.state.error_truncated = false;
        self.current.status = "failed".into();
        self.checkpoint()
    }
    fn cancel(&mut self) -> Result<ShellStep> {
        self.read = Vec::new();
        self.state.phase.clear();
        self.state.terminal_error = "shell operation canceled".into();
        self.state.error_truncated = false;
        self.current.status = "canceled".into();
        self.checkpoint()
    }
}
fn elapsed(at: Timestamp, start: Timestamp) -> Result<f64> {
    let end =
        DateTime::parse_from_rfc3339(&at.to_string()).map_err(|error| Error(error.to_string()))?;
    let start = DateTime::parse_from_rfc3339(&start.to_string())
        .map_err(|error| Error(error.to_string()))?;
    let diff = end - start;
    let ns = diff
        .num_nanoseconds()
        .unwrap_or(if diff < TimeDelta::zero() {
            i64::MIN
        } else {
            i64::MAX
        });
    Ok(ns as f64 / 1e9)
}
