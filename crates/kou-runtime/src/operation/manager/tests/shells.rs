use super::*;
use crate::operation::shell::{ShellInput, decode_shell, shell_spec};
use kou_tools::process::CancelToken;
use rustix::process::{Pid, test_kill_process_group};
use std::{fs, io, path::Path};

fn shell_operation(directory: &Path, id: &str, command: &str, output: i64) -> Operation {
    shell_spec(
        ShellInput {
            command: command.into(),
            shell: "/bin/sh".into(),
            ..ShellInput::default()
        },
        directory.to_str().unwrap().into(),
        output,
    )
    .unwrap()
    .operation(id.into())
}
fn reaped(pid: i64) {
    assert!(pid > 1);
    assert_eq!(
        test_kill_process_group(Pid::from_raw(pid as i32).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}
async fn started(updates: &mut Updates) -> Update {
    loop {
        let update = next(updates).await;
        assert!(!update.operation.terminal());
        if decode_shell(&update.operation).unwrap().process_group_id > 1 {
            return update;
        }
        update.acknowledge(Ok(()));
    }
}

#[tokio::test]
async fn shell_waits_for_ack_and_spools_output_beyond_the_file_completion_budget() {
    let directory = tempfile::tempdir().unwrap();
    let current = shell_operation(
        directory.path(),
        "owned",
        "/usr/bin/head -c 180000 /dev/zero; /usr/bin/head -c 160000 /dev/zero >&2",
        31,
    );
    let (mut manager, mut updates) = LocalManager::new(Limits {
        operations: 1,
        completion_extra_bytes: 256,
        ..Limits::default()
    })
    .unwrap();
    manager.add(current.clone()).unwrap();
    manager.add(current.clone()).unwrap();
    let first = next(&mut updates).await;
    assert!(first.requires_acknowledgement());
    let immutable = first.operation.clone();
    assert_eq!(decode_shell(&immutable).unwrap().phase, "create_directory");
    assert!(!directory.path().join("owned").exists());
    assert_eq!(manager.stats().updates, 0);
    assert!(manager.add(value("full")).is_err());
    let reserved = manager.stats().reserved_bytes;
    let mut bytes = operation_bytes(&immutable);
    let mut checkpoints = 1;
    first.acknowledge(Ok(()));
    loop {
        let update = next(&mut updates).await;
        checkpoints += 1;
        bytes += operation_bytes(&update.operation);
        assert!(update.requires_acknowledgement());
        if update.operation.terminal() {
            assert_eq!(update.operation.status, "completed");
            let state = decode_shell(&update.operation).unwrap();
            let result = state.result.unwrap();
            assert_eq!((result.out_size, result.err_size), (180_000, 160_000));
            assert!(state.out_truncated && state.err_truncated);
            assert!(result.out.len() < 512 && result.err.len() < 512);
            assert_eq!(fs::metadata(state.out_path).unwrap().len(), 180_000);
            assert_eq!(fs::metadata(state.err_path).unwrap().len(), 160_000);
            assert_eq!(manager.stats().admitted, 1);
            update.acknowledge(Ok(()));
            break;
        }
        update.acknowledge(Ok(()));
    }
    assert!(checkpoints <= shell::MAX_CHECKPOINTS);
    assert!(bytes < reserved);
    assert_eq!(decode_shell(&immutable).unwrap().phase, "create_directory");
    manager.add(current).unwrap();
    let shared = manager.shared.clone();
    drop(manager);
    assert!(updates.next().await.is_none());
    assert_eq!(shared.budget.usage.lock().unwrap().bytes, 0);
}

#[tokio::test]
async fn refused_or_discarded_shell_checkpoints_never_cross_create_or_start_boundaries() {
    for dropped in [false, true] {
        for phase in ["create_directory", "create_out", "create_err", "process"] {
            let directory = tempfile::tempdir().unwrap();
            let marker = directory.path().join("launched");
            let current = shell_operation(
                directory.path(),
                "owned",
                &format!("printf launched > '{}'", marker.display()),
                4096,
            );
            let (mut manager, mut updates) = LocalManager::new(Limits::default()).unwrap();
            manager.add(current).unwrap();
            loop {
                let update = next(&mut updates).await;
                assert!(!update.operation.terminal());
                if decode_shell(&update.operation).unwrap().phase == phase {
                    if dropped {
                        drop(update);
                    } else {
                        update.acknowledge(Err(Error("owned journal refused".into())));
                    }
                    break;
                }
                update.acknowledge(Ok(()));
            }
            let failed = next(&mut updates).await;
            assert_eq!(failed.operation.status, "failed");
            assert!(!failed.requires_acknowledgement());
            let state = decode_shell(&failed.operation).unwrap();
            assert!(state.phase.is_empty() && state.result.is_none());
            assert!(state.terminal_error.contains(if dropped {
                "dropped without acknowledgement"
            } else {
                "owned journal refused"
            }));
            assert!(!marker.exists());
            let artifacts = directory.path().join("owned");
            match phase {
                "create_directory" => assert!(!artifacts.exists()),
                "create_out" => assert!(!artifacts.join("out").exists()),
                "create_err" => assert!(!artifacts.join("err").exists()),
                _ => {
                    assert_eq!(fs::metadata(artifacts.join("out")).unwrap().len(), 0);
                    assert_eq!(fs::metadata(artifacts.join("err")).unwrap().len(), 0);
                }
            }
        }
    }
}

struct PanicStarted(mpsc::Sender<u32>);
impl Executor for PanicStarted {
    fn run(&self, state: &FileState, canceled: &AtomicBool) -> Result<FileResult> {
        NativeFiles.run(state, canceled)
    }
    fn run_shell(
        &self,
        current: Operation,
        cancel: &CancelToken,
        publish: &mut dyn FnMut(Operation) -> io::Result<()>,
    ) -> io::Result<()> {
        shell::run_shell(current, cancel, |operation| {
            let pid = decode_shell(&operation).unwrap().process_group_id;
            if pid > 1 {
                self.0.send(pid as u32).unwrap();
                panic!("owned started callback panic");
            }
            publish(operation)
        })
    }
}
#[tokio::test]
async fn refused_started_and_panicking_shells_are_reaped_and_leave_the_worker_usable() {
    for panic in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (send, receive) = mpsc::channel();
        let executor: Arc<dyn Executor> = if panic {
            Arc::new(PanicStarted(send))
        } else {
            Arc::new(NativeFiles)
        };
        let (mut manager, mut updates) = LocalManager::with_executor(
            Limits {
                workers: 1,
                ..Limits::default()
            },
            executor,
        )
        .unwrap();
        manager
            .add(shell_operation(
                directory.path(),
                "owned",
                "while :; do /bin/sleep 1; done",
                4096,
            ))
            .unwrap();
        let pid = if panic {
            loop {
                let update = next(&mut updates).await;
                if update.operation.terminal() {
                    assert_eq!(
                        decode_shell(&update.operation).unwrap().terminal_error,
                        "shell execution panicked"
                    );
                    break receive.recv_timeout(Duration::from_secs(2)).unwrap() as i64;
                }
                update.acknowledge(Ok(()));
            }
        } else {
            let update = started(&mut updates).await;
            let pid = decode_shell(&update.operation).unwrap().process_group_id;
            update.acknowledge(Err(Error("started write refused".into())));
            assert_eq!(next(&mut updates).await.operation.status, "failed");
            pid
        };
        reaped(pid);
        let path = directory.path().join("worker-survived");
        manager.add(file("file", path.to_str().unwrap())).unwrap();
        drop(next(&mut updates).await);
        assert_eq!(next(&mut updates).await.operation.status, "completed");
        assert_eq!(fs::read_to_string(path).unwrap(), "file");
    }
}

#[tokio::test]
async fn shells_share_the_file_pool_and_queued_cancellation_needs_no_effect_or_watcher() {
    let directory = tempfile::tempdir().unwrap();
    let (mut manager, mut updates, entered, release) = gated(Limits {
        workers: 1,
        operations: 2,
        ..Limits::default()
    });
    manager.add(file("file", "/inert-file")).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    manager
        .add(shell_operation(directory.path(), "queued", "exit 99", 128))
        .unwrap();
    assert_eq!(manager.stats().queued, 1);
    assert!(manager.add(value("full")).is_err());
    manager.cancel("queued", "queued stop").unwrap();
    release.send(()).unwrap();
    drop(next(&mut updates).await);
    assert_eq!(next(&mut updates).await.operation.status, "completed");
    let update = next(&mut updates).await;
    assert_eq!(update.operation.status, "canceled");
    assert!(decode_shell(&update.operation).unwrap().result.is_none());
    update.acknowledge(Ok(()));
    assert!(!directory.path().join("queued").exists());
    assert!(entered.try_recv().is_err());
}

#[tokio::test]
async fn cancel_and_drop_wake_pending_receipts_and_join_owned_processes() {
    for action in ["cancel", "manager", "receiver"] {
        for after_start in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (mut manager, mut updates) = LocalManager::new(Limits {
                workers: 1,
                ..Limits::default()
            })
            .unwrap();
            manager
                .add(shell_operation(
                    directory.path(),
                    "owned",
                    "while :; do /bin/sleep 1; done",
                    4096,
                ))
                .unwrap();
            let held = if after_start {
                started(&mut updates).await
            } else {
                next(&mut updates).await
            };
            let pid = decode_shell(&held.operation).unwrap().process_group_id;
            let shared = manager.shared.clone();
            match action {
                "cancel" => {
                    manager
                        .cancel("owned", "stop waiting for durable receipt")
                        .unwrap();
                    let canceled = next(&mut updates).await;
                    assert_eq!(canceled.operation.status, "canceled");
                    assert!(decode_shell(&canceled.operation).unwrap().result.is_none());
                    canceled.acknowledge(Ok(()));
                    drop(manager);
                }
                "manager" => {
                    let (send, receive) = mpsc::channel();
                    let dropper = thread::spawn(move || {
                        drop(manager);
                        send.send(()).unwrap();
                    });
                    receive.recv_timeout(Duration::from_secs(3)).unwrap();
                    dropper.join().unwrap();
                }
                _ => {
                    drop(updates);
                    drop(manager);
                }
            }
            assert!(shared.state.lock().unwrap().active.is_empty());
            if after_start {
                reaped(pid);
            } else {
                assert!(!directory.path().join("owned").exists());
            }
            assert_eq!(shared.budget.usage.lock().unwrap().count, 1);
            drop(held);
            // The canceled terminal update can still be retained in Shared.
            shared.state.lock().unwrap().updates.clear();
            assert_eq!(shared.budget.usage.lock().unwrap().count, 0);
        }
    }
}

#[test]
fn shell_limits_and_invalid_states_reject_before_admission_or_artifact_creation() {
    let directory = tempfile::tempdir().unwrap();
    for limits in [
        Limits {
            retained_bytes: 1,
            ..Limits::default()
        },
        Limits {
            completion_extra_bytes: usize::MAX,
            ..Limits::default()
        },
        Limits {
            operation_bytes: 1,
            ..Limits::default()
        },
    ] {
        let (mut manager, _updates) = LocalManager::new(limits).unwrap();
        assert!(
            manager
                .add(shell_operation(directory.path(), "owned", "exit 0", 128))
                .is_err()
        );
        assert_eq!(manager.stats().seen_ids, 0);
        assert_eq!(manager.stats().reserved_bytes, 0);
        assert!(!directory.path().join("owned").exists());
    }
    let (mut manager, _updates) = LocalManager::new(Limits::default()).unwrap();
    for invalid in ["version", "output", "path", "state"] {
        let mut operation = shell_operation(directory.path(), "owned", "exit 0", 128);
        match invalid {
            "version" => operation.version = 1,
            "output" => operation.max_output_length = 0,
            "path" => operation.id = "../escape".into(),
            _ => operation.state = Json::parse("[]").unwrap(),
        }
        assert!(
            manager.add(operation).is_err(),
            "invalid {invalid} was admitted"
        );
        assert_eq!(manager.stats().seen_ids, 0);
        assert_eq!(manager.stats().reserved_bytes, 0);
    }
    assert!(
        manager
            .add(shell_operation(
                directory.path(),
                "large-policy",
                "exit 0",
                1_000_000
            ))
            .is_err()
    );
}

struct BadShell(&'static str);
impl Executor for BadShell {
    fn run(&self, _: &FileState, _: &AtomicBool) -> Result<FileResult> {
        unreachable!()
    }
    fn run_shell(
        &self,
        mut current: Operation,
        _: &CancelToken,
        publish: &mut dyn FnMut(Operation) -> io::Result<()>,
    ) -> io::Result<()> {
        current.status = "awaiting".into();
        match self.0 {
            "size" => {
                current.state =
                    Json::parse(&serde_json::to_string(&"x".repeat(1 << 20)).unwrap()).unwrap()
            }
            "metadata" => current.max_output_length += 1,
            "no-terminal" => return Ok(()),
            "count" => {
                for _ in 0..=shell::MAX_CHECKPOINTS {
                    publish(current.clone())?;
                }
                return Ok(());
            }
            _ => {
                current.state = Json::parse("null").unwrap();
            }
        }
        publish(current)?;
        Err(io::Error::other("injected executor failure"))
    }
}
#[tokio::test]
async fn shell_snapshot_metadata_count_and_terminal_guards_fail_without_orphaning_workers() {
    for case in ["size", "metadata", "no-terminal", "count", "invalid-state"] {
        let directory = tempfile::tempdir().unwrap();
        let (mut manager, mut updates) = LocalManager::with_executor(
            Limits {
                workers: 1,
                completion_extra_bytes: 256,
                ..Limits::default()
            },
            Arc::new(BadShell(case)),
        )
        .unwrap();
        manager
            .add(shell_operation(directory.path(), "owned", "exit 99", 4096))
            .unwrap();
        let mut count = 0;
        loop {
            let update = next(&mut updates).await;
            count += 1;
            if update.operation.terminal() {
                assert_eq!(update.operation.status, "failed");
                assert!(
                    !decode_shell(&update.operation)
                        .unwrap()
                        .terminal_error
                        .is_empty()
                );
                break;
            }
            update.acknowledge(Ok(()));
        }
        assert!(count <= shell::MAX_CHECKPOINTS + 1);
        assert!(!directory.path().join("owned").exists());
        manager.add(value("survived")).unwrap();
        assert_eq!(next(&mut updates).await.operation.status, "completed");
    }
}

struct LateShell {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Executor for LateShell {
    fn run(&self, _: &FileState, _: &AtomicBool) -> Result<FileResult> {
        unreachable!()
    }
    fn run_shell(
        &self,
        mut current: Operation,
        _: &CancelToken,
        publish: &mut dyn FnMut(Operation) -> io::Result<()>,
    ) -> io::Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        let mut state = decode_shell(&current).unwrap();
        state.result = Some(shell::ShellResult {
            out: "late success".into(),
            ..shell::ShellResult::default()
        });
        current.status = "completed".into();
        current.state = encode(&state).unwrap();
        publish(current)
    }
}
#[tokio::test]
async fn canceled_shell_cannot_publish_a_late_success_result() {
    let directory = tempfile::tempdir().unwrap();
    let (entered, receive) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (mut manager, mut updates) = LocalManager::with_executor(
        Limits::default(),
        Arc::new(LateShell {
            entered,
            release: Mutex::new(wait),
        }),
    )
    .unwrap();
    manager
        .add(shell_operation(directory.path(), "owned", "exit 0", 4096))
        .unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    manager.cancel("owned", "discard late success").unwrap();
    release.send(()).unwrap();
    let update = next(&mut updates).await;
    assert_eq!(update.operation.status, "canceled");
    assert!(decode_shell(&update.operation).unwrap().result.is_none());
    update.acknowledge(Ok(()));
}
