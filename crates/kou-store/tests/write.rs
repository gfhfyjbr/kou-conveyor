use kou_domain::{
    CallStatus, Input, ItemData, Json, ModelResponse, Operation, Session, Timestamp,
    ToolCallStatus, Turn,
};
use kou_store::{Error, SessionLog, SessionStore};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn at() -> Timestamp {
    Timestamp::parse("2026-09-01T12:00:00Z").unwrap()
}
fn session() -> Session {
    Session {
        id: "rust-session".into(),
        created_at: at(),
    }
}
fn input(id: &str) -> ItemData {
    ItemData::Input(Input {
        id: id.into(),
        kind: "external".into(),
        payload: Json::parse(r#"{"message":"hello"}"#).unwrap(),
        delivery: String::new(),
    })
}
fn turn() -> ItemData {
    ItemData::Turn(Turn {
        id: "turn-1".into(),
        previous_turn_id: String::new(),
        kind: "regular".into(),
        model: "test".into(),
    })
}
fn operation() -> Operation {
    Operation {
        id: "op-1".into(),
        kind: "test".into(),
        version: 1,
        status: "ready".into(),
        state: Json::parse(r#"{"step":1}"#).unwrap(),
        ..Operation::default()
    }
}
fn status(operation: Operation, waiting: Option<Vec<String>>) -> ItemData {
    ItemData::ToolCallStatus(ToolCallStatus {
        turn_id: "turn-1".into(),
        call_id: "call-1".into(),
        status: CallStatus {
            waiting_for: waiting,
            ..CallStatus::default()
        },
        operations: vec![operation],
    })
}
fn log(dir: &Path) -> SessionLog {
    SessionLog::open(dir.join("rust-session.session.jsonl")).unwrap()
}

#[test]
fn writes_format_two_and_deduplicates_completed_operation_body() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut writer = store.create_new(session()).unwrap();
    assert_eq!(writer.append_item(input("input-1"), at()).unwrap(), 1);
    writer.append_item(turn(), at()).unwrap();
    writer
        .append_item(
            ItemData::ModelResponse(ModelResponse {
                turn_id: "turn-1".into(),
                ..ModelResponse::default()
            }),
            at(),
        )
        .unwrap();
    writer
        .append_item(status(operation(), Some(vec!["op-1".into()])), at())
        .unwrap();
    let mut completed = operation();
    completed.status = "completed".into();
    completed.state = Json::parse(r#"{"output":"unique-durable-body"}"#).unwrap();
    writer.save_operation(completed.clone()).unwrap();
    assert_eq!(
        log(directory.path())
            .resume_full()
            .unwrap()
            .operations
            .len(),
        1,
        "completed result still awaits a recorded call status"
    );
    writer.append_item(status(completed, None), at()).unwrap();
    drop(writer);
    let reader = log(directory.path());
    assert_eq!(reader.item_count(), 5);
    assert!(reader.resume_full().unwrap().operations.is_empty());
    let ItemData::ToolCallStatus(initial) = reader.item(4).unwrap().data else {
        panic!()
    };
    assert_eq!(initial.operations[0].status, "ready");
    let ItemData::ToolCallStatus(final_status) = reader.item(5).unwrap().data else {
        panic!()
    };
    assert_eq!(final_status.operations[0].status, "completed");
    let bytes = fs::read_to_string(directory.path().join("rust-session.session.jsonl")).unwrap();
    assert_eq!(bytes.matches("unique-durable-body").count(), 1);
    assert!(bytes.contains(r#""Operations":["op-1"]"#));
    assert_eq!(
        fs::metadata(directory.path().join("rust-session.session.jsonl"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn raw_json_whitespace_never_splits_a_durable_line() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut writer = store.create_new(session()).unwrap();
    let mut value = operation();
    value.state =
        Json::parse("{\n  \"text\": \"a b \\\" \\\\ \\n\",\n  \"number\": 1e999\n}").unwrap();
    writer.append_item(turn(), at()).unwrap();
    writer
        .append_item(status(value, Some(vec!["op-1".into()])), at())
        .unwrap();
    let text = fs::read_to_string(directory.path().join("rust-session.session.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 3);
    let resumed = log(directory.path()).resume_full().unwrap();
    assert_eq!(
        resumed.operations[0].state.as_str(),
        Some(r#"{"text":"a b \" \\ \n","number":1e999}"#)
    );
}

#[test]
fn failed_validation_neither_writes_nor_mutates_the_head() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut writer = store.create_new(session()).unwrap();
    writer.append_item(turn(), at()).unwrap();
    let path = directory.path().join("rust-session.session.jsonl");
    let before = fs::read(&path).unwrap();
    assert!(
        writer
            .append_item(
                status(operation(), Some(vec!["wrong-operation".into()])),
                at()
            )
            .is_err()
    );
    assert_eq!(writer.sequence(), 1);
    assert_eq!(fs::read(&path).unwrap(), before);
    writer
        .append_item(status(operation(), Some(vec!["op-1".into()])), at())
        .unwrap();
    assert_eq!(writer.sequence(), 2);
    assert!(writer.append_item(turn(), at()).is_err());
    writer.append_item(input("still-usable"), at()).unwrap();
}

#[test]
fn one_writer_lock_and_existing_session_cannot_be_overwritten_by_create_new() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let writer = store.create_new(session()).unwrap();
    assert!(matches!(
        store.open_writer("rust-session"),
        Err(Error::Busy)
    ));
    drop(writer);
    let before = fs::read(directory.path().join("rust-session.session.jsonl")).unwrap();
    assert!(store.create_new(session()).is_err());
    assert_eq!(
        fs::read(directory.path().join("rust-session.session.jsonl")).unwrap(),
        before
    );
    assert!(store.open_writer("rust-session").is_ok());
}

#[test]
fn opens_go_written_history_and_truncates_only_the_uncommitted_tail_on_append() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/sessionstore/localfile/testdata/golden-session.session.jsonl");
    let original = fs::read(fixture).unwrap();
    let path = directory.path().join("golden-session.session.jsonl");
    fs::write(&path, &original).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{uncommitted")
        .unwrap();
    let before = fs::read(&path).unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut writer = store.open_writer("golden-session").unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "opening the writer must not change a file"
    );
    writer.append_item(input("from-rust"), at()).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(&original));
    assert!(!String::from_utf8(bytes).unwrap().contains("uncommitted"));
    let reader = SessionLog::open(&path).unwrap();
    assert_eq!(reader.item_count(), 5);
    assert_eq!(
        reader.resume_full().unwrap().external_input_ids,
        ["input-1", "from-rust"]
    );
}

#[test]
fn refuses_foreign_appends_and_changed_operation_types() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut writer = store.create_new(session()).unwrap();
    writer.append_item(turn(), at()).unwrap();
    writer
        .append_item(status(operation(), Some(vec!["op-1".into()])), at())
        .unwrap();
    let mut wrong = operation();
    wrong.kind = "changed".into();
    assert!(writer.save_operation(wrong).is_err());
    let path = directory.path().join("rust-session.session.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"foreign writer")
        .unwrap();
    assert!(matches!(
        writer.append_item(input("no-write"), at()),
        Err(Error::Changed)
    ));
}
