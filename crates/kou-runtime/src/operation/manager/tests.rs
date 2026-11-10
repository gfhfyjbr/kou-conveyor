use super::*;
use std::{sync::mpsc, time::Duration};
mod shells;
mod skills;

async fn next(updates: &mut Updates) -> Update {
    tokio::time::timeout(Duration::from_secs(3), updates.next())
        .await
        .unwrap()
        .unwrap()
}
fn value(id: &str) -> Operation {
    value_spec(Json::parse("null").unwrap())
        .unwrap()
        .operation(id.into())
}
fn file(id: &str, path: &str) -> Operation {
    file_spec(FileState {
        action: "write".into(),
        path: path.into(),
        content: id.into(),
        ..FileState::default()
    })
    .unwrap()
    .operation(id.into())
}
struct Gate {
    entered: mpsc::Sender<String>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Executor for Gate {
    fn run(&self, state: &FileState, _: &AtomicBool) -> Result<FileResult> {
        self.entered.send(state.content.clone()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(FileResult {
            text: "late success".into(),
            ..FileResult::default()
        })
    }
}
fn gated(
    limits: Limits,
) -> (
    LocalManager,
    Updates,
    mpsc::Receiver<String>,
    mpsc::Sender<()>,
) {
    let (entered, receive) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (manager, updates) = LocalManager::with_executor(
        limits,
        Arc::new(Gate {
            entered,
            release: Mutex::new(wait),
        }),
    )
    .unwrap();
    (manager, updates, receive, release)
}

#[tokio::test]
async fn completed_value_is_deduplicated_and_keeps_admission_until_forwarded() {
    let (mut manager, mut updates) = LocalManager::new(Limits {
        operations: 1,
        ..Limits::default()
    })
    .unwrap();
    manager.add(value("one")).unwrap();
    let first = next(&mut updates).await;
    assert_eq!(first.operation.status, "completed");
    manager.add(value("one")).unwrap();
    assert_eq!(manager.stats().updates, 0);
    assert_eq!(
        manager.add(value("two")).unwrap_err().to_string(),
        "operation runtime admission is full"
    );
    assert_eq!(manager.stats().admitted, 1);
    drop(first);
    assert_eq!(manager.stats().admitted, 0);
    assert_eq!(manager.stats().reserved_bytes, 0);
    manager.add(value("two")).unwrap();
    assert_eq!(next(&mut updates).await.operation.id, "two");
}

#[tokio::test]
async fn private_file_runs_and_published_snapshots_remain_immutable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("owned.txt")
        .to_str()
        .unwrap()
        .to_owned();
    let (mut manager, mut updates) = LocalManager::new(Limits::default()).unwrap();
    manager.add(file("write", &path)).unwrap();
    let initial = next(&mut updates).await;
    assert_eq!(initial.operation.status, "awaiting");
    let done = next(&mut updates).await;
    assert_eq!(done.operation.status, "completed");
    assert!(
        decode_file(&done.operation)
            .unwrap()
            .result
            .unwrap()
            .text
            .starts_with("Wrote ")
    );
    assert_eq!(initial.operation.status, "awaiting");
    assert!(decode_file(&initial.operation).unwrap().result.is_none());
    std::fs::write(&path, "preserve duplicate").unwrap();
    manager.add(file("write", &path)).unwrap();
    assert_eq!(manager.stats().updates, 0);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "preserve duplicate"
    );
    drop(initial);
    drop(done);
    drop(manager);
    assert!(updates.next().await.is_none());
}

#[tokio::test]
async fn awaiting_edit_and_patch_recover_without_running_their_effect() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned.txt");
    std::fs::write(&path, "original").unwrap();
    let (mut manager, mut updates) = LocalManager::new(Limits::default()).unwrap();
    for action in ["edit", "patch"] {
        let mut current = file_spec(FileState {
            action: action.into(),
            path: path.to_str().unwrap().into(),
            root: directory.path().to_str().unwrap().into(),
            old_string: "original".into(),
            new_string: "changed".into(),
            patch: "*** Begin Patch\n*** Delete File: owned.txt\n*** End Patch".into(),
            ..FileState::default()
        })
        .unwrap()
        .operation(action.into());
        current.status = "awaiting".into();
        manager.add(current).unwrap();
        let update = next(&mut updates).await;
        assert_eq!(update.operation.status, "failed");
        assert!(
            decode_file(&update.operation)
                .unwrap()
                .terminal_error
                .contains("interrupted by a restart")
        );
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    assert_eq!(manager.stats().active, 0);
}

#[tokio::test]
async fn cancel_stays_available_at_capacity_and_skips_a_queued_effect() {
    let (mut manager, mut updates, entered, release) = gated(Limits {
        workers: 1,
        operations: 2,
        ..Limits::default()
    });
    manager.add(file("one", "/inert-one")).unwrap();
    assert_eq!(entered.recv_timeout(Duration::from_secs(2)).unwrap(), "one");
    manager.add(file("two", "/inert-two")).unwrap();
    assert!(manager.add(value("three")).is_err());
    manager.cancel("two", "queued stop").unwrap();
    manager.cancel("one", "active stop").unwrap();
    release.send(()).unwrap();
    let mut terminal = HashSet::new();
    for _ in 0..4 {
        let update = next(&mut updates).await;
        if update.operation.status == "canceled" {
            assert!(decode_file(&update.operation).unwrap().result.is_none());
            terminal.insert(update.operation.id.clone());
        }
    }
    assert_eq!(terminal, HashSet::from(["one".into(), "two".into()]));
    assert!(entered.try_recv().is_err());
}

#[tokio::test]
async fn drop_joins_blocked_work_even_when_the_receiver_is_retained() {
    let (mut manager, mut updates, entered, release) = gated(Limits {
        workers: 1,
        ..Limits::default()
    });
    manager.add(file("one", "/inert-one")).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let shared = manager.shared.clone();
    let (done, receive_done) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(manager);
        done.send(()).unwrap();
    });
    {
        let guard = shared.state.lock().unwrap();
        let (guard, wait) = shared
            .work
            .wait_timeout_while(guard, Duration::from_secs(2), |state| !state.closed)
            .unwrap();
        assert!(guard.closed && !wait.timed_out());
    }
    assert!(receive_done.try_recv().is_err());
    release.send(()).unwrap();
    receive_done.recv_timeout(Duration::from_secs(2)).unwrap();
    dropper.join().unwrap();
    assert_eq!(next(&mut updates).await.operation.status, "awaiting");
    assert_eq!(next(&mut updates).await.operation.status, "canceled");
    assert!(updates.next().await.is_none());
}

#[tokio::test]
async fn byte_and_seen_id_caps_fail_before_acceptance_or_effects() {
    let limits = Limits {
        completion_extra_bytes: 256,
        retained_bytes: 8192,
        seen_ids: 1,
        operations: 1,
        ..Limits::default()
    };
    let (mut manager, mut updates) = LocalManager::new(limits).unwrap();
    let current = file_spec(FileState {
        action: "write".into(),
        path: "/inert-fixture".into(),
        content: "x".repeat(4096),
        ..FileState::default()
    })
    .unwrap()
    .operation("large".into());
    assert_eq!(
        manager.add(current).unwrap_err().to_string(),
        "operation runtime admission is full"
    );
    assert_eq!(manager.stats().seen_ids, 0);
    assert_eq!(manager.stats().reserved_bytes, 0);
    manager.add(value("one")).unwrap();
    drop(next(&mut updates).await);
    assert_eq!(
        manager.add(value("two")).unwrap_err().to_string(),
        "operation runtime seen-ID limit reached"
    );
    manager.add(value("one")).unwrap();
}

struct Huge;
impl Executor for Huge {
    fn run(&self, _: &FileState, _: &AtomicBool) -> Result<FileResult> {
        Ok(FileResult {
            text: "x".repeat(8192),
            ..FileResult::default()
        })
    }
}
#[tokio::test]
async fn oversized_completion_is_an_explicit_failure_not_silent_truncation() {
    let (mut manager, mut updates) = LocalManager::with_executor(
        Limits {
            completion_extra_bytes: 256,
            ..Limits::default()
        },
        Arc::new(Huge),
    )
    .unwrap();
    manager.add(file("one", "/inert-one")).unwrap();
    drop(next(&mut updates).await);
    let done = next(&mut updates).await;
    assert_eq!(done.operation.status, "failed");
    let state = decode_file(&done.operation).unwrap();
    assert!(state.result.is_none());
    assert!(
        state
            .terminal_error
            .contains("effect may already have occurred")
    );
}
#[tokio::test]
async fn dropping_the_receiver_closes_admission_and_keeps_shutdown_joinable() {
    let (mut manager, updates) = LocalManager::new(Limits::default()).unwrap();
    drop(updates);
    assert_eq!(
        manager.add(value("one")).unwrap_err().to_string(),
        "operation runtime is closed"
    );
    drop(manager);
}

#[tokio::test]
async fn seen_id_payload_bytes_remain_bounded_after_updates_are_drained() {
    let (mut manager, mut updates) = LocalManager::new(Limits {
        operations: 1,
        seen_ids: 10,
        seen_bytes: 5,
        ..Limits::default()
    })
    .unwrap();
    assert_eq!(
        manager.add(value("long-id")).unwrap_err().to_string(),
        "operation runtime seen-ID byte limit reached"
    );
    assert_eq!(manager.stats().seen_bytes, 0);
    manager.add(value("one")).unwrap();
    drop(next(&mut updates).await);
    assert_eq!(manager.stats().reserved_bytes, 0);
    assert_eq!(manager.stats().seen_bytes, 3);
    assert_eq!(
        manager.add(value("two")).unwrap_err().to_string(),
        "operation runtime seen-ID byte limit reached"
    );
    manager.add(value("one")).unwrap();
    assert_eq!(manager.stats().seen_ids, 1);
}

#[test]
fn invalid_runtime_limits_and_large_operations_are_rejected_without_acceptance() {
    for limits in [
        Limits {
            workers: 0,
            ..Limits::default()
        },
        Limits {
            workers: 9,
            ..Limits::default()
        },
        Limits {
            operations: 0,
            ..Limits::default()
        },
        Limits {
            completion_extra_bytes: 255,
            ..Limits::default()
        },
        Limits {
            seen_bytes: 0,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            LocalManager::new(limits).err().unwrap().to_string(),
            "invalid operation runtime limits"
        );
    }
    let (mut manager, _updates) = LocalManager::new(Limits {
        operation_bytes: 1,
        ..Limits::default()
    })
    .unwrap();
    assert_eq!(
        manager.add(value("one")).unwrap_err().to_string(),
        "operation exceeds runtime byte limit"
    );
    assert_eq!(manager.stats().seen_ids, 0);
    assert_eq!(manager.stats().reserved_bytes, 0);
}

struct PanicOnce(std::sync::atomic::AtomicUsize);
impl Executor for PanicOnce {
    fn run(&self, _: &FileState, _: &AtomicBool) -> Result<FileResult> {
        assert_ne!(
            self.0.fetch_add(1, Ordering::AcqRel),
            0,
            "owned executor panic"
        );
        Ok(FileResult {
            text: "worker survived".into(),
            ..FileResult::default()
        })
    }
}
#[tokio::test]
async fn a_panicking_executor_finishes_the_operation_and_does_not_orphan_its_worker() {
    let (mut manager, mut updates) = LocalManager::with_executor(
        Limits {
            workers: 1,
            ..Limits::default()
        },
        Arc::new(PanicOnce(std::sync::atomic::AtomicUsize::new(0))),
    )
    .unwrap();
    manager.add(file("one", "/inert-one")).unwrap();
    drop(next(&mut updates).await);
    let failed = next(&mut updates).await;
    assert_eq!(failed.operation.status, "failed");
    assert_eq!(
        decode_file(&failed.operation).unwrap().terminal_error,
        "file computation panicked"
    );
    drop(failed);
    manager.add(file("two", "/inert-two")).unwrap();
    drop(next(&mut updates).await);
    let done = next(&mut updates).await;
    assert_eq!(done.operation.status, "completed");
    assert_eq!(
        decode_file(&done.operation).unwrap().result.unwrap().text,
        "worker survived"
    );
}
