use kou_domain::{
    CallStatus, Input, ItemData, Json, ModelResponse, Operation, Session, Timestamp,
    ToolCallStatus, Turn,
};
use kou_store::{Error, SessionLog, SessionStore};
use std::fs;

fn at() -> Timestamp {
    Timestamp::parse("2026-09-01T12:00:00Z").unwrap()
}
fn session(id: &str) -> Session {
    Session {
        id: id.into(),
        created_at: at(),
    }
}
fn turn(id: &str, previous: &str) -> ItemData {
    ItemData::Turn(Turn {
        id: id.into(),
        previous_turn_id: previous.into(),
        kind: "regular".into(),
        model: String::new(),
    })
}
fn input(id: &str) -> ItemData {
    ItemData::Input(Input {
        id: id.into(),
        kind: "external".into(),
        payload: Json::parse(r#""question""#).unwrap(),
        delivery: String::new(),
    })
}

#[test]
fn fork_keeps_interleaved_prefix_but_not_delayed_status_after_next_turn() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut parent = store.create_new(session("parent")).unwrap();
    parent.append_item(input("prompt-1"), at()).unwrap();
    parent.append_item(turn("turn-1", ""), at()).unwrap();
    parent.append_item(input("interleaved"), at()).unwrap();
    parent
        .append_item(
            ItemData::ModelResponse(ModelResponse {
                turn_id: "turn-1".into(),
                ..ModelResponse::default()
            }),
            at(),
        )
        .unwrap();
    let op = Operation {
        id: "op".into(),
        kind: "test".into(),
        version: 1,
        status: "ready".into(),
        state: Json::parse(r#"{"output":"parent-only"}"#).unwrap(),
        ..Operation::default()
    };
    let mut status = ToolCallStatus {
        turn_id: "turn-1".into(),
        call_id: "call".into(),
        status: CallStatus {
            waiting_for: Some(vec!["op".into()]),
            ..CallStatus::default()
        },
        operations: vec![op.clone()],
    };
    parent
        .append_item(ItemData::ToolCallStatus(status.clone()), at())
        .unwrap();
    parent.append_item(turn("turn-2", "turn-1"), at()).unwrap();
    status.operations[0].status = "completed".into();
    parent
        .append_item(ItemData::ToolCallStatus(status), at())
        .unwrap();
    drop(parent);
    let path = directory.path().join("parent.session.jsonl");
    let before = fs::read(&path).unwrap();
    let mut child = store
        .fork_new(session("child"), "parent", "turn-1")
        .unwrap();
    assert_eq!(child.sequence(), 6);
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(
        child.save_operation(op).is_err(),
        "inherited operation is not dispatchable"
    );
    assert!(
        child.append_item(turn("turn-1", "turn-1"), at()).is_err(),
        "historical ID is still reserved"
    );
    child
        .append_item(turn("child-turn", "turn-1"), at())
        .unwrap();
    let log = SessionLog::open(directory.path().join("child.session.jsonl")).unwrap();
    assert_eq!(
        log.resume_full().unwrap().external_input_ids,
        ["prompt-1", "interleaved"]
    );
    assert!(log.resume_full().unwrap().operations.is_empty());
    let ItemData::ToolCallStatus(status) = log.item(5).unwrap().data else {
        panic!()
    };
    assert!(status.operations.is_empty());
    let ItemData::Fork(fork) = log.item(6).unwrap().data else {
        panic!()
    };
    assert_eq!(fork.parent_id, "parent");
    assert_eq!(fork.previous_turn_id, "turn-1");
    assert!(
        !fs::read_to_string(directory.path().join("child.session.jsonl"))
            .unwrap()
            .contains("parent-only")
    );
}

#[test]
fn explicit_replacement_is_atomic_and_obeys_the_destination_lock() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut parent = store.create_new(session("parent")).unwrap();
    parent.append_item(turn("turn", ""), at()).unwrap();
    drop(parent);
    let destination = store.create_new(session("child")).unwrap();
    assert!(matches!(
        store.fork_or_replace(session("child"), "parent", "turn"),
        Err(Error::Busy)
    ));
    drop(destination);
    let old = SessionLog::open(directory.path().join("child.session.jsonl")).unwrap();
    assert!(store.fork_new(session("child"), "parent", "turn").is_err());
    drop(
        store
            .fork_or_replace(session("child"), "parent", "turn")
            .unwrap(),
    );
    assert!(matches!(old.ensure_current(), Err(Error::Changed)));
    let before = fs::read(directory.path().join("child.session.jsonl")).unwrap();
    assert!(
        store
            .fork_or_replace(session("child"), "parent", "missing")
            .is_err()
    );
    assert!(store.fork_new(session("parent"), "parent", "turn").is_err());
    assert_eq!(
        fs::read(directory.path().join("child.session.jsonl")).unwrap(),
        before
    );
    let replacement = store.create_or_replace(session("child")).unwrap();
    assert_eq!(replacement.sequence(), 0);
}

#[test]
fn nested_fork_keeps_inherited_ownership_and_historical_turn_guards() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut parent = store.create_new(session("parent")).unwrap();
    parent.append_item(turn("old", ""), at()).unwrap();
    drop(parent);
    let mut child = store.fork_new(session("child"), "parent", "old").unwrap();
    child.append_item(turn("own", "old"), at()).unwrap();
    drop(child);
    let mut grandchild = store
        .fork_new(session("grandchild"), "child", "own")
        .unwrap();
    assert!(
        grandchild
            .append_item(
                ItemData::ModelResponse(ModelResponse {
                    turn_id: "own".into(),
                    ..ModelResponse::default()
                }),
                at()
            )
            .is_err()
    );
    assert!(grandchild.append_item(turn("old", "own"), at()).is_err());
    grandchild.append_item(turn("new", "own"), at()).unwrap();
    assert_eq!(grandchild.sequence(), 5);
}
