use kou_domain::{CallStatus, ItemData, Operation, Session, Timestamp, ToolCallStatus, Turn};
use kou_runtime::operation::{SkillReadLimits, SkillState, decode_skill, run_skill, skill_spec};
use kou_store::{SessionLog, SessionStore, SessionWriter};
use kou_tools::process::CancelToken;
use std::{fs, io, os::unix::fs::symlink, path::PathBuf};

fn limits(content_bytes: usize, chunk_bytes: usize) -> SkillReadLimits {
    SkillReadLimits {
        content_bytes,
        chunk_bytes,
    }
}
fn operation(path: &std::path::Path) -> Operation {
    skill_spec(path.to_string_lossy().into())
        .unwrap()
        .operation("owned".into())
}
fn fixture(data: &[u8]) -> (tempfile::TempDir, PathBuf, SessionWriter, Operation) {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let path = root.join("SKILL.md");
    fs::write(&path, data).unwrap();
    let store = SessionStore::new(root.join("sessions")).unwrap();
    let at = Timestamp::parse("2026-11-10T12:00:00Z").unwrap();
    let mut writer = store
        .create_new(Session {
            id: "skill-test".into(),
            created_at: at,
        })
        .unwrap();
    writer
        .append_item(
            ItemData::Turn(Turn {
                id: "turn".into(),
                kind: "regular".into(),
                ..Turn::default()
            }),
            at,
        )
        .unwrap();
    let current = operation(&path);
    writer
        .append_item(
            ItemData::ToolCallStatus(ToolCallStatus {
                turn_id: "turn".into(),
                call_id: "skill".into(),
                status: CallStatus {
                    waiting_for: Some(vec!["owned".into()]),
                    ..CallStatus::default()
                },
                operations: vec![current.clone()],
            }),
            at,
        )
        .unwrap();
    (directory, path, writer, current)
}

#[test]
fn skill_native_reads_large_binary_content_through_actual_journal_boundaries() {
    let mut data = b"long skill instructions\n".repeat(24_000);
    data.extend_from_slice(b"\xff\x00\xe4\xb8\xad");
    let (_directory, path, mut writer, current) = fixture(&data);
    let mut last = None;
    let mut lengths = Vec::new();
    let policy = SkillReadLimits::default();
    run_skill(current, &CancelToken::default(), policy, |next| {
        let state = decode_skill(&next).unwrap();
        assert_eq!(state.content, data[..state.content.len()]);
        lengths.push(state.content.len());
        writer
            .save_operation(next.clone())
            .map_err(io::Error::other)?;
        last = Some(next);
        Ok(())
    })
    .unwrap();
    assert!(lengths.len() <= policy.max_checkpoints().unwrap());
    assert_eq!(lengths[0], 0);
    assert!(lengths.iter().any(|n| *n > 0 && *n < data.len()));
    let last = last.unwrap();
    assert_eq!(last.status, "completed");
    assert_eq!(decode_skill(&last).unwrap().content, data);
    assert_eq!(fs::read(&path).unwrap(), data);
    let resume = SessionLog::open(
        path.parent()
            .unwrap()
            .join("sessions/skill-test.session.jsonl"),
    )
    .unwrap()
    .resume_full()
    .unwrap();
    assert_eq!(
        resume
            .operations
            .iter()
            .find(|value| value.id == "owned")
            .unwrap(),
        &last
    );
}

#[test]
fn skill_native_recovery_uses_last_durable_prefix_without_rereading_it() {
    let data = b"original prefix and original suffix";
    let (_directory, path, mut writer, current) = fixture(data);
    let policy = limits(64, 8);
    let mut calls = 0;
    let error = run_skill(current, &CancelToken::default(), policy, |next| {
        calls += 1;
        let length = decode_skill(&next).unwrap().content.len();
        writer.save_operation(next).map_err(io::Error::other)?;
        if length == 8 {
            Err(io::Error::other(
                "injected uncertain receipt after durable prefix",
            ))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert!(error.to_string().contains("injected uncertain receipt"));
    let resume = SessionLog::open(
        path.parent()
            .unwrap()
            .join("sessions/skill-test.session.jsonl"),
    )
    .unwrap()
    .resume_full()
    .unwrap();
    let pending = resume
        .operations
        .into_iter()
        .find(|value| value.id == "owned")
        .unwrap();
    assert_eq!(pending.status, "awaiting");
    assert_eq!(decode_skill(&pending).unwrap().content, data[..8]);
    let mut changed = data.to_vec();
    changed[..8].fill(b'X');
    fs::write(&path, &changed).unwrap();
    let mut final_value = None;
    run_skill(pending, &CancelToken::default(), policy, |next| {
        writer
            .save_operation(next.clone())
            .map_err(io::Error::other)?;
        final_value = Some(next);
        Ok(())
    })
    .unwrap();
    let final_value = final_value.unwrap();
    assert_eq!(final_value.status, "completed");
    assert_eq!(decode_skill(&final_value).unwrap().content, data);
    assert_ne!(fs::read(&path).unwrap(), data);
}

#[test]
fn skill_native_refusal_prevents_open_and_publisher_failure_stops_later_updates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    fs::write(&path, b"owned").unwrap();
    let mut calls = 0;
    let error = run_skill(
        operation(&path),
        &CancelToken::default(),
        limits(16, 4),
        |_| {
            calls += 1;
            Err(io::Error::other("refused initial read"))
        },
    )
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(error.to_string(), "refused initial read");
    let moved = directory.path().join("moved");
    let mut final_value = None;
    run_skill(
        operation(&path),
        &CancelToken::default(),
        limits(16, 4),
        |next| {
            if final_value.is_none() {
                fs::rename(&path, &moved).unwrap();
            }
            final_value = Some(next);
            Ok(())
        },
    )
    .unwrap();
    let final_value = final_value.unwrap();
    assert_eq!(final_value.status, "failed");
    assert_eq!(
        decode_skill(&final_value).unwrap().terminal_error,
        format!("open {}: no such file or directory", path.display())
    );
    assert_eq!(fs::read(&moved).unwrap(), b"owned");
    let mut calls = 0;
    run_skill(
        operation(&moved),
        &CancelToken::default(),
        limits(16, 4),
        |next| {
            calls += 1;
            if decode_skill(&next).unwrap().content.is_empty() {
                Ok(())
            } else {
                Err(io::Error::other("refused bytes"))
            }
        },
    )
    .unwrap_err();
    assert_eq!(calls, 2);
}

#[test]
fn skill_native_cancellation_keeps_only_observed_prefix_and_stops_future_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    fs::write(&path, b"abcdefghijklmnopqrstuvwx").unwrap();
    for when in ["before", "initial", "chunk"] {
        let cancel = CancelToken::default();
        if when == "before" {
            cancel.cancel();
        }
        let mut lengths = Vec::new();
        let mut final_value = None;
        run_skill(operation(&path), &cancel, limits(32, 8), |next| {
            let length = decode_skill(&next).unwrap().content.len();
            lengths.push(length);
            if when == "initial" || (when == "chunk" && length > 0) {
                cancel.cancel();
            }
            final_value = Some(next);
            Ok(())
        })
        .unwrap();
        let final_value = final_value.unwrap();
        assert_eq!(final_value.status, "canceled");
        assert_eq!(
            decode_skill(&final_value).unwrap().terminal_error,
            "skill-use operation canceled"
        );
        assert_eq!(
            lengths,
            match when {
                "before" => vec![0],
                "initial" => vec![0, 0],
                _ => vec![0, 8, 8],
            }
        );
    }
}

#[test]
fn skill_native_limits_reject_invalid_and_oversized_inputs_without_truncation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    fs::write(&path, b"abcdefghijklmnop").unwrap();
    for policy in [
        limits(0, 8),
        limits(16, 0),
        limits(usize::MAX, 8),
        limits(1 << 20, 1),
        limits(16, (256 << 10) + 1),
    ] {
        assert!(
            run_skill(
                operation(&path),
                &CancelToken::default(),
                policy,
                |_| panic!("invalid policy published work")
            )
            .is_err()
        );
    }
    for status in ["completed", "failed", "canceled", "unknown"] {
        let mut current = operation(&path);
        current.status = status.into();
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(
            run_skill(current, &cancel, limits(16, 8), |_| panic!(
                "terminal operation changed"
            ))
            .is_err()
        );
    }
    let mut recovered = operation(&path);
    recovered.status = "awaiting".into();
    recovered.state = kou_domain::Json::parse(
        &serde_json::to_string(&SkillState {
            path: path.to_string_lossy().into(),
            content: vec![0; 17],
            terminal_error: String::new(),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(
        run_skill(
            recovered,
            &CancelToken::default(),
            limits(16, 8),
            |_| panic!("oversized recovery published work")
        )
        .is_err()
    );
    let mut seen = Vec::new();
    run_skill(
        operation(&path),
        &CancelToken::default(),
        limits(8, 4),
        |next| {
            seen.push((
                next.status.clone(),
                decode_skill(&next).unwrap().content.len(),
            ));
            assert!(
                decode_skill(&next).unwrap().terminal_error.is_empty()
                    || decode_skill(&next).unwrap().terminal_error
                        == "skill file exceeds runtime content byte limit"
            );
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, [("awaiting".into(), 0), ("failed".into(), 0)]);
    assert_eq!(fs::read(&path).unwrap(), b"abcdefghijklmnop");
}

#[test]
fn skill_native_detects_growth_and_truncation_between_durable_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    for grow in [false, true] {
        fs::write(
            &path,
            if grow {
                b"abcd".as_slice()
            } else {
                b"abcdefghijklmnop".as_slice()
            },
        )
        .unwrap();
        let mut last = None;
        let mut changed = false;
        run_skill(
            operation(&path),
            &CancelToken::default(),
            limits(if grow { 8 } else { 32 }, 4),
            |next| {
                let length = decode_skill(&next).unwrap().content.len();
                if !changed && length == 4 {
                    fs::write(
                        &path,
                        if grow {
                            b"abcdefghijklmnopqrstuvwx".as_slice()
                        } else {
                            b"".as_slice()
                        },
                    )
                    .unwrap();
                    changed = true;
                }
                assert!(length <= if grow { 8 } else { 32 });
                last = Some(next);
                Ok(())
            },
        )
        .unwrap();
        let last = last.unwrap();
        assert_eq!(last.status, "failed");
        let state = decode_skill(&last).unwrap();
        assert_eq!(
            state.content,
            if grow {
                b"abcdefgh".as_slice()
            } else {
                b"abcd".as_slice()
            }
        );
        assert_eq!(
            state.terminal_error,
            if grow {
                "skill file exceeds runtime content byte limit"
            } else {
                "skill read returned an invalid completion"
            }
        );
    }
}

#[test]
fn skill_native_follows_skill_symlinks_but_rejects_nonregular_files_and_survives_publisher_panic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SKILL.md");
    fs::write(&path, b"abcdefghijklmnop").unwrap();
    let alias = directory.path().join("alias");
    symlink(&path, &alias).unwrap();
    let mut last = None;
    run_skill(
        operation(&alias),
        &CancelToken::default(),
        limits(32, 4),
        |next| {
            last = Some(next);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        decode_skill(&last.unwrap()).unwrap().content,
        b"abcdefghijklmnop"
    );
    let mut last = None;
    run_skill(
        operation(directory.path()),
        &CancelToken::default(),
        limits(32, 4),
        |next| {
            last = Some(next);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        decode_skill(&last.unwrap()).unwrap().terminal_error,
        "skill file is not a regular file"
    );
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_skill(
            operation(&path),
            &CancelToken::default(),
            limits(32, 4),
            |next| {
                if !decode_skill(&next).unwrap().content.is_empty() {
                    panic!("injected owned publisher panic");
                }
                Ok(())
            },
        )
    }));
    assert!(panic.is_err());
    let mut last = None;
    run_skill(
        operation(&path),
        &CancelToken::default(),
        limits(32, 4),
        |next| {
            last = Some(next);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(last.unwrap().status, "completed");
    assert_eq!(fs::read(&path).unwrap(), b"abcdefghijklmnop");
}
