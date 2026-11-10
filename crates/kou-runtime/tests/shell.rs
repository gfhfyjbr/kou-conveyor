use kou_domain::{CallStatus, ItemData, Json, Operation, Session, Timestamp, ToolCallStatus, Turn};
use kou_runtime::operation::shell::*;
use kou_store::{SessionLog, SessionStore};
use kou_tools::process::{CancelToken, CapturePaths, ProcessRequest, run_captured_process};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
};

fn at(text: &str) -> Timestamp {
    Timestamp::parse(text).unwrap()
}
fn start() -> Timestamp {
    at("2026-11-10T12:00:00.000000001Z")
}
fn end() -> Timestamp {
    at("2026-11-10T12:00:01.234567891Z")
}
fn op(phase: &str, limit: i64) -> Operation {
    let mut value = shell_spec(
        ShellInput {
            shell: "/bin/sh".into(),
            ..ShellInput::default()
        },
        "/owned-inert".into(),
        limit,
    )
    .unwrap()
    .operation("owned".into());
    if !phase.is_empty() {
        let mut state = decode_shell(&value).unwrap();
        state.phase = phase.into();
        value.status = "awaiting".into();
        value.state = raw(&state);
    }
    value
}
fn raw(value: &impl serde::Serialize) -> Json {
    Json::parse(&serde_json::to_string(value).unwrap()).unwrap()
}
fn event(kind: &str, phase: &str, result: ShellEventResult) -> ShellEvent {
    ShellEvent {
        kind: kind.into(),
        source: "owned".into(),
        correlation_id: phase.into(),
        result,
    }
}

#[test]
fn clocked_process_start_timeout_and_escalation_are_pure_dispatches() {
    let mut current = op("process", 4000);
    let mut state = decode_shell(&current).unwrap();
    state.input.timeout = 0.125;
    current.state = raw(&state);
    let mut shell = Shell::new(current).unwrap();
    let started = shell
        .handle_at(
            Some(event(
                "process.started",
                "process",
                ShellEventResult::Started { pid: 424242 },
            )),
            start(),
        )
        .unwrap();
    assert_eq!(
        decode_shell(started.operation.as_ref().unwrap())
            .unwrap()
            .started_at,
        start()
    );
    let [ShellDispatch::Timer(timer)] = started.dispatches.as_slice() else {
        panic!("{started:?}")
    };
    assert_eq!(timer.correlation_id, "timeout");
    assert_eq!(timer.deadline, at("2026-11-10T12:00:00.125000001Z"));
    let timed_out = shell
        .handle_at(
            Some(event("timer.fired", "timeout", ShellEventResult::Empty)),
            timer.deadline,
        )
        .unwrap();
    assert!(
        decode_shell(timed_out.operation.as_ref().unwrap())
            .unwrap()
            .timed_out
    );
    let [
        ShellDispatch::Signal {
            group_id: 424242,
            signal: 15,
        },
        ShellDispatch::Timer(kill),
    ] = timed_out.dispatches.as_slice()
    else {
        panic!("{timed_out:?}")
    };
    assert_eq!(kill.correlation_id, "kill");
    assert_eq!(kill.deadline, at("2026-11-10T12:00:05.125000001Z"));
    let escalated = shell
        .handle_at(
            Some(event("timer.fired", "kill", ShellEventResult::Empty)),
            kill.deadline,
        )
        .unwrap();
    assert!(escalated.operation.is_none());
    assert!(matches!(
        escalated.dispatches.as_slice(),
        [ShellDispatch::Signal {
            group_id: 424242,
            signal: 9
        }]
    ));
}

#[test]
fn process_recovery_never_restarts_or_signals_a_saved_numeric_pid() {
    for pid in [0, 2, 424242] {
        let mut current = op("process", 4000);
        let mut state = decode_shell(&current).unwrap();
        state.process_group_id = pid;
        current.state = raw(&state);
        let mut shell = Shell::new(current).unwrap();
        let step = shell.handle_at(None, start()).unwrap();
        assert!(step.dispatches.is_empty());
        let current = step.operation.unwrap();
        assert_eq!(current.status, "failed");
        let state = decode_shell(&current).unwrap();
        assert_eq!(state.process_group_id, pid);
        assert!(state.terminal_error.contains(if pid == 0 {
            "outcome is unknown"
        } else {
            "interrupted"
        }));
        assert!(state.phase.is_empty());
    }
}

#[test]
fn read_chunks_are_bounded_ephemeral_and_reset_on_redispatch() {
    let mut shell = Shell::new(op("read_out", 2)).unwrap();
    let chunk = || {
        event(
            "io.read_output",
            "read_out",
            ShellEventResult::ReadOutput {
                offset: 0,
                data: vec![b'a'; 8],
            },
        )
    };
    let step = shell.handle_at(Some(chunk()), start()).unwrap();
    assert!(step.operation.is_none() && step.dispatches.is_empty());
    assert!(shell.state().inline_out.is_empty());
    let redispatch = shell.handle_at(None, start()).unwrap();
    assert!(matches!(
        redispatch.dispatches.as_slice(),
        [ShellDispatch::Read(ReadRequest {
            offset: 0,
            count: 8,
            ..
        })]
    ));
    assert!(
        shell
            .handle_at(Some(chunk()), start())
            .unwrap()
            .operation
            .is_none()
    );
    let overflow = shell
        .handle_at(
            Some(event(
                "io.read_output",
                "read_out",
                ShellEventResult::ReadOutput {
                    offset: 8,
                    data: vec![b'x'],
                },
            )),
            start(),
        )
        .unwrap();
    assert_eq!(overflow.operation.unwrap().status, "failed");
    assert!(shell.state().error_truncated);
}

#[test]
fn clocked_completion_preserves_duration_signal_status_and_frees_inline_buffers() {
    let mut shell = Shell::new(op("process", 4000)).unwrap();
    shell
        .handle_at(
            Some(event(
                "process.started",
                "process",
                ShellEventResult::Started { pid: 424242 },
            )),
            start(),
        )
        .unwrap();
    shell
        .handle_at(
            Some(event("timer.fired", "timeout", ShellEventResult::Empty)),
            start(),
        )
        .unwrap();
    shell
        .handle_at(
            Some(event(
                "process.exited",
                "process",
                ShellEventResult::Exited {
                    exit_code: -1,
                    signal: 15,
                },
            )),
            end(),
        )
        .unwrap();
    shell
        .handle_at(
            Some(event(
                "io.read_output",
                "read_out",
                ShellEventResult::ReadOutput {
                    offset: 0,
                    data: b"out".to_vec(),
                },
            )),
            end(),
        )
        .unwrap();
    shell
        .handle_at(
            Some(event(
                "io.read_completed",
                "read_out",
                ShellEventResult::ReadCompleted { size: 3 },
            )),
            end(),
        )
        .unwrap();
    let step = shell
        .handle_at(
            Some(event(
                "io.read_completed",
                "read_err",
                ShellEventResult::ReadCompleted { size: 0 },
            )),
            end(),
        )
        .unwrap();
    let current = step.operation.unwrap();
    assert_eq!(current.status, "completed");
    let state = decode_shell(&current).unwrap();
    let result = state.result.unwrap();
    assert_eq!(
        (result.out.as_str(), result.err.as_str(), result.exit_code),
        ("out", "", 143)
    );
    assert_eq!(result.duration, 1.23456789);
    assert!(result.timed_out);
    assert_eq!(
        (state.out_size, state.err_size, state.process_group_id),
        (3, 0, 0)
    );
    assert!(state.pending_exit_code.is_none() && state.phase.is_empty());
    assert_eq!(
        shell.state().inline_out.capacity()
            + shell.state().inline_err.capacity()
            + shell.state().inline_out_tail.capacity()
            + shell.state().inline_err_tail.capacity(),
        0
    );
}

#[test]
fn typed_boundaries_reject_positional_objects_and_inline_overflow() {
    for state in [
        r#"[]"#,
        r#"{"Input":[]}"#,
        r#"{"Input":{"Shell":"/bin/sh"},"Result":[]}"#,
    ] {
        let mut current = op("process", 40);
        current.state = Json::parse(state).unwrap();
        assert!(decode_shell(&current).is_err(), "{state}");
    }
    let mut current = op("read_out", 1);
    let mut state = decode_shell(&current).unwrap();
    state.inline_out = vec![b'x'; 5];
    current.state = raw(&state);
    assert!(Shell::new(current).is_err());
    for timeout in [-1.0, 14400.1, f64::NAN, f64::INFINITY] {
        assert!(
            shell_spec(
                ShellInput {
                    shell: "/bin/sh".into(),
                    timeout,
                    ..ShellInput::default()
                },
                "/owned".into(),
                10
            )
            .is_err()
        );
    }
}

#[test]
fn private_capture_effects_follow_actual_session_journal_checkpoints() {
    let directory = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(directory.path()).unwrap();
    let store = SessionStore::new(base.join("sessions")).unwrap();
    let mut writer = store
        .create_new(Session {
            id: "shell-session".into(),
            created_at: start(),
        })
        .unwrap();
    let mut latest = shell_spec(ShellInput { shell: "/bin/sh".into(), command: "i=0; while [ \"$i\" -lt 20000 ]; do printf 0123456789; i=$((i+1)); done; printf stderr >&2; exit 7".into(), ..ShellInput::default() }, base.to_string_lossy().into(), 11).unwrap().operation("owned".into());
    writer
        .append_item(
            ItemData::Turn(Turn {
                id: "turn".into(),
                kind: "regular".into(),
                ..Turn::default()
            }),
            start(),
        )
        .unwrap();
    writer
        .append_item(
            ItemData::ToolCallStatus(ToolCallStatus {
                turn_id: "turn".into(),
                call_id: "call".into(),
                status: CallStatus {
                    waiting_for: Some(vec![latest.id.clone()]),
                    ..CallStatus::default()
                },
                operations: vec![latest.clone()],
            }),
            start(),
        )
        .unwrap();
    let path = base.join("sessions/shell-session.session.jsonl");
    let mut shell = Shell::new(latest.clone()).unwrap();
    let mut step = shell.handle_at(None, start()).unwrap();
    let mut starts = 0;
    loop {
        if let Some(current) = step.operation.take() {
            writer.save_operation(current.clone()).unwrap();
            latest = current;
        }
        if latest.terminal() {
            assert!(step.dispatches.is_empty());
            break;
        }
        assert_eq!(step.dispatches.len(), 1);
        let dispatch = step.dispatches.pop().unwrap();
        let resumed = SessionLog::open(&path).unwrap().resume_full().unwrap();
        let durable = resumed
            .operations
            .iter()
            .find(|op| op.id == "owned")
            .unwrap();
        assert_eq!(durable.state, latest.state);
        assert!(durable.state.as_str().unwrap().len() < 2000);
        step = match dispatch {
            ShellDispatch::Create(request) => {
                if request.kind == 2 {
                    fs::DirBuilder::new()
                        .mode(request.mode)
                        .create(&request.path)
                        .unwrap();
                } else {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(request.mode)
                        .open(&request.path)
                        .unwrap();
                }
                shell
                    .handle_at(
                        Some(event(
                            "io.create_completed",
                            &request.correlation_id,
                            ShellEventResult::Created { kind: request.kind },
                        )),
                        start(),
                    )
                    .unwrap()
            }
            ShellDispatch::Start(request) => {
                starts += 1;
                assert_eq!(decode_shell(durable).unwrap().phase, "process");
                let mut process = ProcessRequest::new(request.path);
                process.arguments = request.arguments.into_iter().map(Into::into).collect();
                process.directory = request.directory.into();
                let captured = run_captured_process(
                    &process,
                    &CapturePaths {
                        stdout: request.stdout_path.into(),
                        stderr: request.stderr_path.into(),
                    },
                    &CancelToken::default(),
                    |pid| {
                        let started = shell
                            .handle_at(
                                Some(event(
                                    "process.started",
                                    "process",
                                    ShellEventResult::Started { pid: pid as i64 },
                                )),
                                start(),
                            )
                            .map_err(std::io::Error::other)?;
                        assert!(started.dispatches.is_empty());
                        let current = started.operation.unwrap();
                        writer
                            .save_operation(current.clone())
                            .map_err(std::io::Error::other)?;
                        latest = current;
                        Ok(())
                    },
                )
                .unwrap();
                shell
                    .handle_at(
                        Some(event(
                            "process.exited",
                            "process",
                            ShellEventResult::Exited {
                                exit_code: captured.exit_code as i64,
                                signal: captured.signal as i64,
                            },
                        )),
                        end(),
                    )
                    .unwrap()
            }
            ShellDispatch::Read(request) => {
                let mut file = fs::File::open(request.path).unwrap();
                let size = file.metadata().unwrap().len() as i64;
                file.seek(SeekFrom::Start(request.offset as u64)).unwrap();
                let mut data = Vec::new();
                file.take(request.count as u64)
                    .read_to_end(&mut data)
                    .unwrap();
                if !data.is_empty() {
                    let chunk = shell
                        .handle_at(
                            Some(event(
                                "io.read_output",
                                &request.correlation_id,
                                ShellEventResult::ReadOutput {
                                    offset: request.offset,
                                    data,
                                },
                            )),
                            end(),
                        )
                        .unwrap();
                    assert!(chunk.operation.is_none() && chunk.dispatches.is_empty());
                }
                shell
                    .handle_at(
                        Some(event(
                            "io.read_completed",
                            &request.correlation_id,
                            ShellEventResult::ReadCompleted { size },
                        )),
                        end(),
                    )
                    .unwrap()
            }
            other => panic!("unexpected fixture dispatch {other:?}"),
        };
    }
    assert_eq!(starts, 1);
    assert_eq!(latest.status, "completed");
    let state = decode_shell(&latest).unwrap();
    let result = state.result.unwrap();
    assert_eq!(
        (result.out_size, result.err_size, result.exit_code),
        (200000, 6, 7)
    );
    assert!(result.out.starts_with("01234...") && result.out.ends_with("456789"));
    assert_eq!(result.err, "stderr");
    assert!(state.out_truncated && !state.err_truncated);
    assert_eq!(fs::metadata(base.join("owned/out")).unwrap().len(), 200000);
    for (path, mode) in [
        (base.join("owned"), 0o700),
        (base.join("owned/out"), 0o600),
        (base.join("owned/err"), 0o600),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    drop(writer);
    let resumed = SessionLog::open(path).unwrap().resume_full().unwrap();
    let current = resumed
        .operations
        .iter()
        .find(|op| op.id == "owned")
        .unwrap();
    assert_eq!(current.state, latest.state);
}
