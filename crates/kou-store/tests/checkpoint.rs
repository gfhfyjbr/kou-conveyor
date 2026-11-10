use kou_domain::{
    CallStatus, Input, Item, ItemData, ItemOperation, ItemRecord, Json, Operation, Record, Session,
    Timestamp, ToolCallStatus, Turn,
};
use kou_store::{SessionLog, SessionRecovery, SessionStore};
use std::{fs, io::Write, path::Path};

fn at() -> Timestamp {
    Timestamp::parse("2026-09-01T12:00:00Z").unwrap()
}
fn input(id: &str) -> ItemData {
    ItemData::Input(Input {
        id: id.into(),
        kind: "external".into(),
        payload: Json::parse(r#""question""#).unwrap(),
        delivery: String::new(),
    })
}
fn turn(id: &str, previous: &str) -> ItemData {
    ItemData::Turn(Turn {
        id: id.into(),
        previous_turn_id: previous.into(),
        kind: "regular".into(),
        model: String::new(),
    })
}
fn operation(id: &str) -> Operation {
    Operation {
        id: id.into(),
        kind: "test".into(),
        version: 1,
        status: "ready".into(),
        state: Json::parse(r#"{"step":1}"#).unwrap(),
        ..Operation::default()
    }
}
fn status(call: &str, operation: Operation) -> ItemData {
    ItemData::ToolCallStatus(ToolCallStatus {
        turn_id: "turn-1".into(),
        call_id: call.into(),
        status: CallStatus {
            waiting_for: if operation.terminal() {
                None
            } else {
                Some(vec![operation.id.clone()])
            },
            ..CallStatus::default()
        },
        operations: vec![operation],
    })
}
fn create(directory: &Path) -> (SessionStore, kou_store::SessionWriter) {
    let store = SessionStore::new(directory).unwrap();
    let mut writer = store
        .create_new(Session {
            id: "session".into(),
            created_at: at(),
        })
        .unwrap();
    writer.append_item(input("first"), at()).unwrap();
    writer.append_item(turn("turn-1", ""), at()).unwrap();
    writer
        .append_item(status("call-1", operation("op-1")), at())
        .unwrap();
    let mut completed = operation("op-1");
    completed.status = "completed".into();
    completed.state = Json::parse(&format!(
        r#"{{"output":"{}"}}"#,
        "cold-output".repeat(10000)
    ))
    .unwrap();
    writer.save_operation(completed.clone()).unwrap();
    writer
        .append_item(status("call-1", completed), at())
        .unwrap();
    (store, writer)
}
fn checkpoint_state(generation: u32) -> Json {
    Json::parse(&format!(r#"{{"generation":{generation}}}"#)).unwrap()
}
fn equivalent(path: &Path) -> kou_store::ResumeState {
    let full = SessionLog::open(path).unwrap().resume_full().unwrap();
    let fast = SessionRecovery::open(path).unwrap().into_resume().unwrap();
    let mut actual = serde_json::to_value(&fast).unwrap();
    actual["Checkpoint"] = serde_json::Value::Null;
    assert_eq!(actual, serde_json::to_value(full).unwrap());
    fast
}

#[test]
fn checkpoint_archives_cold_bodies_and_keeps_unrecorded_completions() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut writer) = create(dir.path());
    writer
        .append_item(status("pending-call", operation("pending")), at())
        .unwrap();
    let mut pending = operation("pending");
    pending.status = "completed".into();
    pending.state = Json::parse(r#"{"output":"unrecorded-completion"}"#).unwrap();
    writer.save_operation(pending.clone()).unwrap();
    writer.save_checkpoint(checkpoint_state(1)).unwrap();
    writer.append_item(input("late"), at()).unwrap();
    drop(writer);
    let path = dir.path().join("session.session.jsonl");
    let fast = equivalent(&path);
    assert_eq!(fast.operations, [pending]);
    let checkpoint = fast.checkpoint.unwrap();
    assert_eq!(checkpoint.after, 5);
    assert_eq!(checkpoint.items.len(), 1);
    assert_eq!(checkpoint.items[0].sequence, 6);
    assert_eq!(checkpoint.state, checkpoint_state(1));
    let bytes = fs::read_to_string(&path).unwrap();
    let checkpoint_line = bytes
        .lines()
        .find(|line| line.starts_with(r#"{"type":"checkpoint""#))
        .unwrap();
    assert!(!checkpoint_line.contains("cold-output"));
    assert!(checkpoint_line.contains("unrecorded-completion"));
    let mut writer = store.open_writer("session").unwrap();
    assert!(writer.append_item(turn("turn-1", "turn-1"), at()).is_err());
    let mut wrong = operation("op-1");
    wrong.kind = "other".into();
    assert!(writer.save_operation(wrong).is_err());
    writer.append_item(turn("turn-2", "turn-1"), at()).unwrap();
    writer.save_checkpoint(checkpoint_state(2)).unwrap();
    assert_eq!(
        equivalent(&path).checkpoint.unwrap().state,
        checkpoint_state(2)
    );
}

#[test]
fn archived_id_references_in_the_tail_load_the_original_record() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut writer) = create(dir.path());
    writer.save_checkpoint(checkpoint_state(1)).unwrap();
    let next = writer.sequence() + 1;
    drop(writer);
    let path = dir.path().join("session.session.jsonl");
    let record = Record::Item(ItemRecord {
        item: Item {
            sequence: next,
            recorded_at: at(),
            data: ItemData::ToolCallStatus(ToolCallStatus {
                turn_id: "turn-1".into(),
                call_id: "call-1".into(),
                ..ToolCallStatus::default()
            }),
        },
        operations: vec![ItemOperation::Saved("op-1".into())],
    });
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&record.encode_line().unwrap())
        .unwrap();
    let fast = equivalent(&path);
    let ItemData::ToolCallStatus(status) = &fast.checkpoint.unwrap().items[0].data else {
        panic!()
    };
    assert!(
        status.operations[0]
            .state
            .as_str()
            .unwrap()
            .contains("cold-output")
    );
}

#[test]
fn invalid_or_torn_new_checkpoint_falls_back_to_the_previous_one() {
    for mode in [
        "checksum",
        "version",
        "offset",
        "head",
        "partial",
        "torn-suffix",
        "committed-corruption",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut writer) = create(dir.path());
        writer.save_checkpoint(checkpoint_state(1)).unwrap();
        writer.append_item(input("between"), at()).unwrap();
        let path = dir.path().join("session.session.jsonl");
        let boundary = fs::metadata(&path).unwrap().len() as usize;
        writer.save_checkpoint(checkpoint_state(2)).unwrap();
        drop(writer);
        let mut bytes = fs::read(&path).unwrap();
        match mode {
            "checksum" | "version" | "offset" | "head" => {
                let mut line: serde_json::Value =
                    serde_json::from_slice(&bytes[boundary..]).unwrap();
                match mode {
                    "checksum" => line["data"]["SHA256"] = "0".repeat(64).into(),
                    "version" => line["data"]["Version"] = 99.into(),
                    "offset" => line["data"]["Offset"] = (boundary - 1).into(),
                    "head" => {
                        line["data"]["Data"]["Head"]["OwnedTurns"] = serde_json::json!(["missing"]);
                        use sha2::{Digest, Sha256};
                        line["data"]["SHA256"] = format!(
                            "{:x}",
                            Sha256::digest(serde_json::to_vec(&line["data"]["Data"]).unwrap())
                        )
                        .into();
                    }
                    _ => unreachable!(),
                }
                bytes.truncate(boundary);
                bytes.extend(serde_json::to_vec(&line).unwrap());
                bytes.push(b'\n');
            }
            "partial" => {
                bytes.truncate(bytes.len() - 20);
            }
            "torn-suffix" => bytes.extend_from_slice(b"{bad"),
            "committed-corruption" => bytes.extend_from_slice(b"{bad}\n"),
            _ => unreachable!(),
        }
        fs::write(&path, bytes).unwrap();
        if mode == "committed-corruption" {
            assert!(SessionRecovery::open(&path).is_err());
            continue;
        }
        let fast = equivalent(&path);
        assert_eq!(
            fast.checkpoint.unwrap().state,
            checkpoint_state(if mode == "torn-suffix" { 2 } else { 1 }),
            "{mode}"
        );
    }
}

#[test]
fn a_fork_in_the_suffix_uses_full_replay_and_missing_state_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut writer) = create(dir.path());
    let path = dir.path().join("session.session.jsonl");
    let before = fs::read(&path).unwrap();
    assert!(writer.save_checkpoint(Json::default()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    writer.save_checkpoint(checkpoint_state(1)).unwrap();
    drop(writer);
    let mut child = store
        .fork_new(
            Session {
                id: "child".into(),
                created_at: at(),
            },
            "session",
            "turn-1",
        )
        .unwrap();
    child
        .append_item(turn("child-turn", "turn-1"), at())
        .unwrap();
    drop(child);
    let child = dir.path().join("child.session.jsonl");
    assert!(equivalent(&child).checkpoint.is_none());
    // A valid earlier checkpoint can remain in a log whose suffix later adds
    // a fork. The fork invalidates owned state, not the archival prefix.
    // A forked prefix must not initialize operations. Use a header/turn-only
    // source so the format's inherited-prefix invariant still holds.
    let blank = dir.path().join("branch.session.jsonl");
    let mut branch = store
        .create_new(Session {
            id: "branch".into(),
            created_at: at(),
        })
        .unwrap();
    branch.append_item(turn("inherited", ""), at()).unwrap();
    branch.save_checkpoint(checkpoint_state(1)).unwrap();
    drop(branch);
    fs::OpenOptions::new()
        .append(true)
        .open(&blank)
        .unwrap()
        .write_all(
            &Record::Item(ItemRecord {
                item: Item {
                    sequence: 2,
                    recorded_at: at(),
                    data: ItemData::Fork(kou_domain::Fork {
                        parent_id: "other".into(),
                        previous_turn_id: "inherited".into(),
                    }),
                },
                operations: Vec::new(),
            })
            .encode_line()
            .unwrap(),
        )
        .unwrap();
    assert!(equivalent(&blank).checkpoint.is_none());
}
