use kou_domain::{CallStatus, ItemData, Operation, Session, Timestamp, ToolCallStatus, Turn};
use kou_runtime::operation::shell::{ShellInput, decode_shell, run_shell, shell_spec};
use kou_store::{SessionLog, SessionStore, SessionWriter};
use kou_tools::process::CancelToken;
use std::{
    cell::Cell,
    fs, io,
    os::unix::fs::symlink,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn at() -> Timestamp {
    Timestamp::parse("2026-11-10T12:00:00Z").unwrap()
}
fn fixture(
    command: &str,
    timeout: f64,
) -> (
    tempfile::TempDir,
    PathBuf,
    SessionWriter,
    Operation,
    PathBuf,
) {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let store = SessionStore::new(root.join("sessions")).unwrap();
    let mut writer = store
        .create_new(Session {
            id: "native-shell".into(),
            created_at: at(),
        })
        .unwrap();
    let operation = shell_spec(
        ShellInput {
            command: command.into(),
            shell: "/bin/sh".into(),
            directory: root.to_string_lossy().into(),
            timeout,
        },
        root.to_string_lossy().into(),
        17,
    )
    .unwrap()
    .operation("owned".into());
    writer
        .append_item(
            ItemData::Turn(Turn {
                id: "turn".into(),
                kind: "regular".into(),
                ..Turn::default()
            }),
            at(),
        )
        .unwrap();
    writer
        .append_item(
            ItemData::ToolCallStatus(ToolCallStatus {
                turn_id: "turn".into(),
                call_id: "call".into(),
                status: CallStatus {
                    waiting_for: Some(vec![operation.id.clone()]),
                    ..CallStatus::default()
                },
                operations: vec![operation.clone()],
            }),
            at(),
        )
        .unwrap();
    let path = root.join("sessions/native-shell.session.jsonl");
    (directory, root, writer, operation, path)
}
fn latest(path: &Path) -> Operation {
    SessionLog::open(path)
        .unwrap()
        .resume_full()
        .unwrap()
        .operations
        .iter()
        .find(|current| current.id == "owned")
        .unwrap()
        .clone()
}
fn gone(pid: u32) {
    assert_eq!(
        rustix::process::test_kill_process_group(
            rustix::process::Pid::from_raw(pid as i32).unwrap()
        ),
        Err(rustix::io::Errno::SRCH)
    );
}
fn ready(path: &Path) {
    let end = Instant::now() + Duration::from_secs(3);
    while fs::read(path).unwrap() != b"ready" {
        assert!(Instant::now() < end, "fixture readiness deadline");
        std::thread::yield_now();
    }
}

#[test]
fn native_runner_journals_each_boundary_and_keeps_complete_output_on_disk() {
    let (_directory, root, mut writer, operation, path) = fixture(
        "printf invoked > effect; i=0; while [ \"$i\" -lt 20000 ]; do printf 0123456789; i=$((i+1)); done; i=0; while [ \"$i\" -lt 18000 ]; do printf abcdefghij >&2; i=$((i+1)); done; exit 7",
        0.0,
    );
    let mut phases = Vec::new();
    run_shell(operation, &CancelToken::default(), |current| {
        let state = decode_shell(&current).unwrap();
        if state.phase == "create_directory" {
            assert!(!root.join("owned").exists());
        }
        if state.phase == "process" && state.process_group_id == 0 {
            assert!(!root.join("effect").exists());
            assert!(root.join("owned/out").is_file() && root.join("owned/err").is_file());
        }
        let expected = current.state.clone();
        phases.push(state.phase.clone());
        writer.save_operation(current).map_err(io::Error::other)?;
        assert_eq!(latest(&path).state, expected);
        Ok(())
    })
    .unwrap();
    let current = latest(&path);
    assert_eq!(current.status, "completed");
    let state = decode_shell(&current).unwrap();
    let result = state.result.unwrap();
    assert_eq!(
        (result.out_size, result.err_size, result.exit_code),
        (200000, 180000, 7)
    );
    assert!(state.out_truncated && state.err_truncated);
    assert!(result.out.starts_with("01234567...") && result.out.ends_with("123456789"));
    assert!(result.err.starts_with("abcdefgh...") && result.err.ends_with("bcdefghij"));
    assert!(result.duration > 0.0 && result.duration < 20.0);
    assert!(!result.timed_out);
    assert_eq!(fs::metadata(root.join("owned/out")).unwrap().len(), 200000);
    assert_eq!(fs::metadata(root.join("owned/err")).unwrap().len(), 180000);
    assert_eq!(fs::read(root.join("effect")).unwrap(), b"invoked");
    assert!(
        phases.len() <= 12
            && phases.iter().any(|phase| phase == "read_out_tail")
            && phases.iter().any(|phase| phase == "read_err_tail")
    );
}

#[test]
fn refused_pre_effect_checkpoint_never_crosses_the_effect_and_can_resume() {
    for phase in ["create_directory", "create_out", "create_err", "process"] {
        let (_directory, root, mut writer, operation, path) =
            fixture("printf invoked > effect; printf out", 0.0);
        let error = run_shell(operation, &CancelToken::default(), |current| {
            let state = decode_shell(&current).unwrap();
            if state.phase == phase && state.process_group_id == 0 {
                return Err(io::Error::other("journal refused boundary"));
            }
            writer.save_operation(current).map_err(io::Error::other)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "journal refused boundary");
        assert!(!root.join("effect").exists());
        if phase == "create_directory" {
            assert!(!root.join("owned").exists());
        }
        if phase == "create_out" {
            assert!(!root.join("owned/out").exists());
        }
        if phase == "create_err" {
            assert!(!root.join("owned/err").exists());
        }
        let recovered = latest(&path);
        assert!(!recovered.terminal());
        run_shell(recovered, &CancelToken::default(), |current| {
            writer.save_operation(current).map_err(io::Error::other)
        })
        .unwrap();
        assert_eq!(latest(&path).status, "completed");
        assert_eq!(fs::read(root.join("effect")).unwrap(), b"invoked");
    }
}

#[test]
fn refused_or_panicking_started_checkpoint_reaps_and_never_relaunches_on_recovery() {
    for panic in [false, true] {
        let (_directory, root, mut writer, operation, path) = fixture(
            "printf start >> effect; printf ready; while :; do :; done",
            0.0,
        );
        let pid = Cell::new(0);
        let returned = catch_unwind(AssertUnwindSafe(|| {
            run_shell(operation, &CancelToken::default(), |current| {
                let state = decode_shell(&current).unwrap();
                if state.phase == "process" && state.process_group_id > 1 {
                    pid.set(state.process_group_id as u32);
                    if panic {
                        panic!("journal publisher panic");
                    }
                    return Err(io::Error::other("started checkpoint refused"));
                }
                writer.save_operation(current).map_err(io::Error::other)
            })
        }));
        if panic {
            assert!(returned.is_err());
        } else {
            assert_eq!(
                returned.unwrap().unwrap_err().to_string(),
                "started checkpoint refused"
            );
        }
        assert!(pid.get() > 1);
        gone(pid.get());
        let effect = fs::read(root.join("effect")).ok();
        let recovered = latest(&path);
        let state = decode_shell(&recovered).unwrap();
        assert_eq!(
            (
                recovered.status.as_str(),
                state.phase.as_str(),
                state.process_group_id
            ),
            ("awaiting", "process", 0)
        );
        run_shell(recovered, &CancelToken::default(), |current| {
            writer.save_operation(current).map_err(io::Error::other)
        })
        .unwrap();
        let current = latest(&path);
        assert_eq!(current.status, "failed");
        assert_eq!(fs::read(root.join("effect")).ok(), effect);
    }
}

#[test]
fn cancellation_before_effect_or_after_start_returns_only_after_owned_cleanup() {
    let (_directory, root, mut writer, operation, path) =
        fixture("printf must-not-run > effect", 0.0);
    let cancel = CancelToken::default();
    cancel.cancel();
    run_shell(operation, &cancel, |current| {
        writer.save_operation(current).map_err(io::Error::other)
    })
    .unwrap();
    assert_eq!(latest(&path).status, "canceled");
    assert!(!root.join("effect").exists() && !root.join("owned").exists());
    let (_directory, root, mut writer, operation, path) = fixture(
        "trap 'printf final >&2; exit 0' TERM; printf ready; while :; do :; done",
        0.0,
    );
    let cancel = CancelToken::default();
    let pid = Cell::new(0);
    run_shell(operation, &cancel, |current| {
        let state = decode_shell(&current).unwrap();
        if state.phase == "process" && state.process_group_id > 1 {
            pid.set(state.process_group_id as u32);
            ready(&root.join("owned/out"));
            cancel.cancel();
        }
        writer.save_operation(current).map_err(io::Error::other)
    })
    .unwrap();
    gone(pid.get());
    assert_eq!(latest(&path).status, "canceled");
    assert_eq!(fs::read(root.join("owned/err")).unwrap(), b"final");
}

#[test]
fn timeout_checkpoint_is_durable_before_the_owned_process_observes_term() {
    let (_directory, root, mut writer, operation, path) = fixture(
        "trap 'if [ -f timeout-ack ]; then printf ack >&2; else printf missing-ack >&2; fi; exit 0' TERM; printf ready; while :; do :; done",
        0.03,
    );
    let pid = Cell::new(0);
    let timeouts = Cell::new(0);
    run_shell(operation, &CancelToken::default(), |current| {
        let state = decode_shell(&current).unwrap();
        if state.phase == "process" && state.process_group_id > 1 {
            pid.set(state.process_group_id as u32);
            ready(&root.join("owned/out"));
        }
        let timed_out = state.phase == "process" && state.timed_out;
        writer.save_operation(current).map_err(io::Error::other)?;
        if timed_out {
            timeouts.set(timeouts.get() + 1);
            assert!(decode_shell(&latest(&path)).unwrap().timed_out);
            fs::write(root.join("timeout-ack"), "ack")?;
        }
        Ok(())
    })
    .unwrap();
    gone(pid.get());
    assert_eq!(timeouts.get(), 1);
    let current = latest(&path);
    assert_eq!(current.status, "completed");
    let state = decode_shell(&current).unwrap();
    let result = state.result.unwrap();
    assert!(result.timed_out);
    assert_eq!((result.err.as_str(), result.exit_code), ("ack", 0));
}

#[test]
fn native_failures_are_checkpointed_without_following_capture_symlinks() {
    let (_directory, root, mut writer, mut operation, path) =
        fixture("printf must-not-run > effect", 0.0);
    let mut state = decode_shell(&operation).unwrap();
    state.input.shell = "/owned-no-such-shell".into();
    operation.state = kou_domain::Json::parse(&serde_json::to_string(&state).unwrap()).unwrap();
    run_shell(operation, &CancelToken::default(), |current| {
        writer.save_operation(current).map_err(io::Error::other)
    })
    .unwrap();
    assert_eq!(latest(&path).status, "failed");
    assert!(!root.join("effect").exists());
    let (_directory, root, mut writer, mut operation, path) =
        fixture("printf must-not-run > effect", 0.0);
    fs::create_dir(root.join("owned")).unwrap();
    fs::write(root.join("target"), "untouched").unwrap();
    symlink(root.join("target"), root.join("owned/out")).unwrap();
    fs::write(root.join("owned/err"), "").unwrap();
    let mut state = decode_shell(&operation).unwrap();
    state.phase = "read_out".into();
    state.pending_exit_code = Some(0);
    operation.status = "awaiting".into();
    operation.state = kou_domain::Json::parse(&serde_json::to_string(&state).unwrap()).unwrap();
    run_shell(operation, &CancelToken::default(), |current| {
        writer.save_operation(current).map_err(io::Error::other)
    })
    .unwrap();
    assert_eq!(latest(&path).status, "failed");
    assert_eq!(fs::read(root.join("target")).unwrap(), b"untouched");
    assert!(!root.join("effect").exists());
}

#[test]
fn recovered_post_exit_reads_complete_without_starting_a_process() {
    let (_directory, root, mut writer, mut operation, path) =
        fixture("printf duplicated >> effect", 0.0);
    fs::create_dir(root.join("owned")).unwrap();
    fs::write(root.join("owned/out"), vec![b'a'; 200]).unwrap();
    fs::write(root.join("owned/err"), "stderr").unwrap();
    let mut state = decode_shell(&operation).unwrap();
    state.phase = "read_out".into();
    state.pending_exit_code = Some(3);
    operation.status = "awaiting".into();
    operation.state = kou_domain::Json::parse(&serde_json::to_string(&state).unwrap()).unwrap();
    writer.save_operation(operation.clone()).unwrap();
    run_shell(operation, &CancelToken::default(), |current| {
        assert_ne!(decode_shell(&current).unwrap().phase, "process");
        writer.save_operation(current).map_err(io::Error::other)
    })
    .unwrap();
    assert!(!root.join("effect").exists());
    let current = latest(&path);
    assert_eq!(current.status, "completed");
    let result = decode_shell(&current).unwrap().result.unwrap();
    assert_eq!(
        (result.out_size, result.err_size, result.exit_code),
        (200, 6, 3)
    );
}
