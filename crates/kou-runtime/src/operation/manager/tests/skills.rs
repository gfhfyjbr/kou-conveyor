use super::*;
use kou_tools::process::CancelToken;
use std::{fs, io, path::Path};

fn skill(path: &Path, id: &str) -> Operation {
    skill_spec(path.to_string_lossy().into())
        .unwrap()
        .operation(id.into())
}
fn small_limits() -> Limits {
    Limits {
        workers: 1,
        skill_read: SkillReadLimits {
            content_bytes: 32,
            chunk_bytes: 8,
        },
        ..Limits::default()
    }
}

#[tokio::test]
async fn skill_checkpoints_wait_for_receipts_and_preserve_immutable_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    let data = b"owned skill instructions\n".repeat(24_000);
    fs::write(&path, &data).unwrap();
    let current = skill(&path, "owned");
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
    assert_eq!(immutable.status, "awaiting");
    assert!(decode_skill(&immutable).unwrap().content.is_empty());
    assert_eq!(manager.stats().updates, 0);
    assert!(manager.add(value("full")).is_err());
    let reserved = manager.stats().reserved_bytes;
    let mut bytes = operation_bytes(&immutable);
    let mut count = 1;
    first.acknowledge(Ok(()));
    loop {
        let update = next(&mut updates).await;
        count += 1;
        bytes += operation_bytes(&update.operation);
        assert!(update.requires_acknowledgement());
        let state = decode_skill(&update.operation).unwrap();
        assert_eq!(state.content, data[..state.content.len()]);
        if update.operation.terminal() {
            assert_eq!(update.operation.status, "completed");
            assert_eq!(state.content, data);
            assert_eq!(manager.stats().admitted, 1);
            update.acknowledge(Ok(()));
            break;
        }
        update.acknowledge(Ok(()));
    }
    assert!(count <= SkillReadLimits::default().max_checkpoints().unwrap());
    assert!(bytes < reserved);
    assert!(decode_skill(&immutable).unwrap().content.is_empty());
    manager.add(current).unwrap();
    let shared = manager.shared.clone();
    drop(manager);
    assert!(updates.next().await.is_none());
    assert_eq!(shared.budget.usage.lock().unwrap().bytes, 0);
}

#[tokio::test]
async fn default_skill_budget_admits_parallel_workers_but_bounds_retained_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("not-created");
    let limits = Limits::default();
    let (mut manager, mut updates) = LocalManager::new(limits).unwrap();
    let mut admitted = 0;
    loop {
        match manager.add(skill(&path, &format!("owned-{admitted}"))) {
            Ok(()) => admitted += 1,
            Err(error) => {
                assert_eq!(error.to_string(), "operation runtime admission is full");
                break;
            }
        }
        assert!(
            admitted < 16,
            "default skill reservations stopped bounding admission"
        );
    }
    assert!(admitted >= limits.workers);
    assert!(manager.stats().reserved_bytes <= limits.retained_bytes);
    assert_eq!(manager.stats().seen_ids, admitted);
    let mut held = Vec::new();
    for _ in 0..limits.workers {
        let update = next(&mut updates).await;
        assert_eq!(update.operation.status, "awaiting");
        assert!(update.requires_acknowledgement());
        held.push(update);
    }
    let shared = manager.shared.clone();
    drop(manager);
    assert!(shared.state.lock().unwrap().active.is_empty());
    assert!(!path.exists());
    shared.state.lock().unwrap().updates.clear();
    assert_eq!(shared.budget.usage.lock().unwrap().count, limits.workers);
    drop(held);
    assert_eq!(shared.budget.usage.lock().unwrap().bytes, 0);
}

#[tokio::test]
async fn skill_file_is_opened_only_after_the_initial_receipt_is_acknowledged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    let staged = directory.path().join("staged");
    fs::write(&staged, b"owned instructions").unwrap();
    let (mut manager, mut updates) = LocalManager::new(small_limits()).unwrap();
    manager.add(skill(&path, "owned")).unwrap();
    let first = next(&mut updates).await;
    assert!(decode_skill(&first.operation).unwrap().content.is_empty());
    fs::rename(&staged, &path).unwrap();
    first.acknowledge(Ok(()));
    loop {
        let update = next(&mut updates).await;
        let terminal = update.operation.terminal();
        if terminal {
            assert_eq!(update.operation.status, "completed");
            assert_eq!(
                decode_skill(&update.operation).unwrap().content,
                b"owned instructions"
            );
        }
        update.acknowledge(Ok(()));
        if terminal {
            break;
        }
    }
}

#[tokio::test]
async fn refused_or_dropped_skill_receipts_stop_later_reads_and_keep_the_prefix() {
    for dropped in [false, true] {
        for stop in [0, 8] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("SKILL.md");
            let data = b"abcdefghijklmnopqrstuvwx";
            fs::write(&path, data).unwrap();
            let (mut manager, mut updates) = LocalManager::new(small_limits()).unwrap();
            manager.add(skill(&path, "owned")).unwrap();
            loop {
                let update = next(&mut updates).await;
                assert!(!update.operation.terminal());
                if decode_skill(&update.operation).unwrap().content.len() == stop {
                    if dropped {
                        drop(update);
                    } else {
                        update.acknowledge(Err(Error("owned skill journal refused".into())));
                    }
                    break;
                }
                update.acknowledge(Ok(()));
            }
            let failed = next(&mut updates).await;
            assert_eq!(failed.operation.status, "failed");
            assert!(!failed.requires_acknowledgement());
            let state = decode_skill(&failed.operation).unwrap();
            assert_eq!(state.content, data[..stop]);
            assert!(state.terminal_error.contains(if dropped {
                "skill-use checkpoint was dropped without acknowledgement"
            } else {
                "owned skill journal refused"
            }));
            drop(failed);
            let effect = directory.path().join("survived");
            manager.add(file("file", effect.to_str().unwrap())).unwrap();
            drop(next(&mut updates).await);
            assert_eq!(next(&mut updates).await.operation.status, "completed");
            assert_eq!(fs::read(effect).unwrap(), b"file");
            drop(manager);
            assert!(updates.next().await.is_none());
        }
    }
}

#[tokio::test]
async fn cancel_and_shutdown_wake_skill_receipts_without_losing_partial_content() {
    for action in ["cancel", "manager", "receiver"] {
        for prefix in [0, 8] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("SKILL.md");
            let data = b"abcdefghijklmnopqrstuvwx";
            fs::write(&path, data).unwrap();
            let (mut manager, mut updates) = LocalManager::new(small_limits()).unwrap();
            manager.add(skill(&path, "owned")).unwrap();
            let held = loop {
                let update = next(&mut updates).await;
                if decode_skill(&update.operation).unwrap().content.len() == prefix {
                    break update;
                }
                update.acknowledge(Ok(()));
            };
            let shared = manager.shared.clone();
            match action {
                "cancel" => {
                    manager.cancel("owned", "stop pending receipt").unwrap();
                    let canceled = next(&mut updates).await;
                    assert_eq!(canceled.operation.status, "canceled");
                    assert_eq!(
                        decode_skill(&canceled.operation).unwrap().content,
                        data[..prefix]
                    );
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
            let mut state = shared.state.lock().unwrap();
            assert!(state.active.is_empty());
            for update in &state.updates {
                assert_eq!(update.operation.status, "canceled");
                let state = decode_skill(&update.operation).unwrap();
                assert_eq!(state.content, data[..prefix]);
                assert_eq!(state.terminal_error, "skill-use operation canceled");
            }
            assert_eq!(held.operation.status, "awaiting");
            assert_eq!(shared.budget.usage.lock().unwrap().count, 1);
            drop(held);
            state.updates.clear();
            assert_eq!(shared.budget.usage.lock().unwrap().count, 0);
        }
    }
}

#[tokio::test]
async fn skill_shares_file_workers_and_queued_cancellation_never_opens_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("not-created");
    let (mut manager, mut updates, entered, release) = gated(Limits {
        operations: 2,
        ..small_limits()
    });
    manager.add(file("file", "/inert-file")).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    manager.add(skill(&path, "queued")).unwrap();
    assert_eq!(manager.stats().queued, 1);
    assert!(manager.add(value("full")).is_err());
    manager.cancel("queued", "queued stop").unwrap();
    release.send(()).unwrap();
    drop(next(&mut updates).await);
    assert_eq!(next(&mut updates).await.operation.status, "completed");
    let canceled = next(&mut updates).await;
    assert_eq!(canceled.operation.status, "canceled");
    let state = decode_skill(&canceled.operation).unwrap();
    assert!(state.content.is_empty());
    assert_eq!(state.terminal_error, "skill-use operation canceled");
    canceled.acknowledge(Ok(()));
    assert!(!path.exists());
    assert!(entered.try_recv().is_err());
}

#[test]
fn skill_limits_and_invalid_recovered_states_reject_before_admission() {
    for skill_read in [
        SkillReadLimits {
            content_bytes: 0,
            chunk_bytes: 8,
        },
        SkillReadLimits {
            content_bytes: 16,
            chunk_bytes: 0,
        },
        SkillReadLimits {
            content_bytes: 1 << 20,
            chunk_bytes: 1,
        },
        SkillReadLimits {
            content_bytes: usize::MAX,
            chunk_bytes: 8,
        },
        SkillReadLimits {
            content_bytes: 16,
            chunk_bytes: (256 << 10) + 1,
        },
    ] {
        assert_eq!(
            LocalManager::new(Limits {
                skill_read,
                ..Limits::default()
            })
            .err()
            .unwrap()
            .to_string(),
            "invalid operation runtime limits"
        );
    }
    let path = Path::new("/inert-skill");
    for limits in [
        Limits {
            retained_bytes: 1,
            ..small_limits()
        },
        Limits {
            completion_extra_bytes: usize::MAX,
            ..small_limits()
        },
        Limits {
            operation_bytes: 1,
            ..small_limits()
        },
    ] {
        let (mut manager, _updates) = LocalManager::new(limits).unwrap();
        assert!(manager.add(skill(path, "owned")).is_err());
        assert_eq!(manager.stats().seen_ids, 0);
        assert_eq!(manager.stats().reserved_bytes, 0);
    }
    let (mut manager, _updates) = LocalManager::new(small_limits()).unwrap();
    for case in ["version", "state", "path", "oversized-recovery"] {
        let mut current = skill(path, "owned");
        match case {
            "version" => current.version = 2,
            "state" => current.state = Json::parse("[]").unwrap(),
            "path" => current.state = Json::parse(r#"{"Path":""}"#).unwrap(),
            _ => {
                current.status = "awaiting".into();
                current.state = encode(&SkillState {
                    path: path.to_string_lossy().into(),
                    content: vec![0; 33],
                    terminal_error: String::new(),
                })
                .unwrap();
            }
        }
        assert!(manager.add(current).is_err(), "admitted {case}");
        assert_eq!(manager.stats().seen_ids, 0);
        assert_eq!(manager.stats().reserved_bytes, 0);
    }
}

struct BadSkill(&'static str);
impl Executor for BadSkill {
    fn run(&self, state: &FileState, canceled: &AtomicBool) -> Result<FileResult> {
        NativeFiles.run(state, canceled)
    }
    fn run_skill(
        &self,
        mut current: Operation,
        _: &CancelToken,
        limits: SkillReadLimits,
        publish: &mut dyn FnMut(Operation) -> io::Result<()>,
    ) -> io::Result<()> {
        current.status = "awaiting".into();
        match self.0 {
            "size" => current.state = encode(&"x".repeat(1 << 20)).unwrap(),
            "id" => current.id.push('x'),
            "kind" => current.kind = "shell".into(),
            "version" => current.version += 1,
            "output" => current.max_output_length += 1,
            "idempotency" => current.idempotency = Json::parse("0").unwrap(),
            "no-terminal" => return Ok(()),
            "count" => {
                for _ in 0..=limits.max_checkpoints()? {
                    publish(current.clone())?;
                }
                return Ok(());
            }
            "panic" => {
                let mut state = decode_skill(&current).unwrap();
                state.content = b"observed".to_vec();
                current.state = encode(&state).unwrap();
                publish(current)?;
                panic!("owned skill executor panic");
            }
            _ => current.state = Json::parse("null").unwrap(),
        }
        publish(current)?;
        Err(io::Error::other("injected skill executor failure"))
    }
}

#[tokio::test]
async fn skill_snapshot_metadata_count_and_panic_guards_leave_workers_usable() {
    for case in [
        "size",
        "id",
        "kind",
        "version",
        "output",
        "idempotency",
        "no-terminal",
        "count",
        "panic",
        "invalid-state",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (mut manager, mut updates) = LocalManager::with_executor(
            Limits {
                completion_extra_bytes: 256,
                ..small_limits()
            },
            Arc::new(BadSkill(case)),
        )
        .unwrap();
        manager
            .add(skill(&directory.path().join("inert-skill"), "owned"))
            .unwrap();
        let mut count = 0;
        loop {
            let update = next(&mut updates).await;
            count += 1;
            if update.operation.terminal() {
                assert_eq!(update.operation.status, "failed", "{case}");
                assert!(!update.requires_acknowledgement());
                let state = decode_skill(&update.operation).unwrap();
                assert!(!state.terminal_error.is_empty(), "{case}");
                assert_eq!(
                    state.content,
                    if case == "panic" {
                        b"observed".as_slice()
                    } else {
                        &[]
                    }
                );
                if case == "invalid-state" {
                    assert!(
                        state
                            .terminal_error
                            .contains("invalid skill-use checkpoint")
                    );
                }
                break;
            }
            update.acknowledge(Ok(()));
        }
        assert!(count <= small_limits().skill_read.max_checkpoints().unwrap() + 1);
        let effect = directory.path().join("survived");
        manager.add(file("file", effect.to_str().unwrap())).unwrap();
        drop(next(&mut updates).await);
        assert_eq!(next(&mut updates).await.operation.status, "completed");
        assert_eq!(fs::read(effect).unwrap(), b"file");
    }
}

struct LateSkill {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Executor for LateSkill {
    fn run(&self, _: &FileState, _: &AtomicBool) -> Result<FileResult> {
        unreachable!()
    }
    fn run_skill(
        &self,
        mut current: Operation,
        _: &CancelToken,
        _: SkillReadLimits,
        publish: &mut dyn FnMut(Operation) -> io::Result<()>,
    ) -> io::Result<()> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        let mut state = decode_skill(&current).unwrap();
        state.content = b"observed".to_vec();
        current.state = encode(&state).unwrap();
        current.status = "completed".into();
        publish(current)
    }
}

#[tokio::test]
async fn canceled_skill_overrides_late_success_without_erasing_observed_bytes() {
    let (entered, receive) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (mut manager, mut updates) = LocalManager::with_executor(
        small_limits(),
        Arc::new(LateSkill {
            entered,
            release: Mutex::new(wait),
        }),
    )
    .unwrap();
    manager
        .add(skill(Path::new("/inert-skill"), "owned"))
        .unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    manager.cancel("owned", "discard late success").unwrap();
    release.send(()).unwrap();
    let canceled = next(&mut updates).await;
    assert_eq!(canceled.operation.status, "canceled");
    let state = decode_skill(&canceled.operation).unwrap();
    assert_eq!(state.content, b"observed");
    assert_eq!(state.terminal_error, "skill-use operation canceled");
    canceled.acknowledge(Ok(()));
}

#[tokio::test]
async fn terminal_skills_are_forwarded_once_without_reopening_their_files() {
    for status in ["completed", "failed", "canceled"] {
        let mut current = skill(Path::new("/inert-skill"), "owned");
        current.status = status.into();
        let (mut manager, mut updates) = LocalManager::new(small_limits()).unwrap();
        manager.add(current.clone()).unwrap();
        manager.add(current.clone()).unwrap();
        let update = next(&mut updates).await;
        assert_eq!(update.operation.as_ref(), &current);
        assert!(!update.requires_acknowledgement());
        assert_eq!(manager.stats().active, 0);
        assert_eq!(manager.stats().updates, 0);
    }
}
