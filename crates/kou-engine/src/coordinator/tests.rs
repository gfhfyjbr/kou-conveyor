use super::*;
use kou_domain::{
    Input, Item, ItemData, Json, Message, ModelResponse, Response, Session, ToolCallStatus, Turn,
};
use kou_store::{SessionLog, SessionRecovery, SessionStore, SessionWriter};
struct Tools;
impl ReplayTools for Tools {
    fn available(&self, name: &str) -> bool {
        name != "Missing"
    }
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        let value = if status.error.is_empty() {
            format!(
                "{}:{}",
                call.call_id,
                operations
                    .iter()
                    .map(|operation| operation.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            status.error.clone()
        };
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value,
        }])
    }
}
fn history() -> History {
    History::new(Builder::default(), Arc::new(Tools))
}
fn at() -> Timestamp {
    Timestamp::parse("2026-11-10T02:00:00Z").unwrap()
}
fn recorded(data: ItemData) -> Item {
    Item {
        sequence: 1,
        recorded_at: at(),
        data,
    }
}
fn input(text: &str) -> ItemData {
    ItemData::Input(Input {
        id: text.into(),
        kind: "external".into(),
        payload: Json::parse(&serde_json::to_string(text).unwrap()).unwrap(),
        ..Input::default()
    })
}
fn turn(id: &str, kind: &str) -> ItemData {
    ItemData::Turn(Turn {
        id: id.into(),
        kind: kind.into(),
        model: "test-model".into(),
        ..Turn::default()
    })
}
fn response(turn: &str, output: Vec<LlmItem>) -> ItemData {
    ItemData::ModelResponse(ModelResponse {
        turn_id: turn.into(),
        response: Response {
            output: Some(output),
            ..Response::default()
        },
        ..ModelResponse::default()
    })
}
fn message(text: &str) -> LlmItem {
    LlmItem {
        model: String::new(),
        provider_id: String::new(),
        data: LlmData::Message(Message {
            role: "assistant".into(),
            text: text.into(),
            ..Message::default()
        }),
    }
}
fn call(name: &str) -> LlmItem {
    LlmItem {
        model: String::new(),
        provider_id: String::new(),
        data: LlmData::ToolCall(ToolCall {
            call_id: "call".into(),
            name: name.into(),
            arguments: r#"{"path":"file.rs"}"#.into(),
        }),
    }
}
fn operation(id: &str, status: &str) -> Operation {
    Operation {
        id: id.into(),
        kind: "test".into(),
        version: 1,
        status: status.into(),
        state: Json::parse("{}").unwrap(),
        ..Operation::default()
    }
}
fn status(wait: &[&str], operations: Vec<Operation>) -> ItemData {
    ItemData::ToolCallStatus(ToolCallStatus {
        turn_id: "turn".into(),
        call_id: "call".into(),
        status: CallStatus {
            waiting_for: Some(wait.iter().map(|id| (*id).into()).collect()),
            ..CallStatus::default()
        },
        operations,
    })
}
fn begin(current: &mut History, name: &str) {
    for data in [
        input("question"),
        turn("turn", "regular"),
        response("turn", vec![call(name)]),
    ] {
        current.apply_recorded(recorded(data)).unwrap();
    }
}
fn compact(current: &mut History) {
    current
        .apply_recorded(recorded(turn("compact", "compaction")))
        .unwrap();
    current
        .apply_recorded(recorded(response("compact", vec![message("summary")])))
        .unwrap();
}
fn json(current: &mut History) -> serde_json::Value {
    serde_json::to_value(current.context.build()).unwrap()
}

#[test]
fn call_status_snapshots_do_not_implicitly_join_waiting_set() {
    let mut current = history();
    begin(&mut current, "Read");
    current
        .apply_recorded(recorded(status(
            &[],
            vec![operation("unwaited", "running")],
        )))
        .unwrap();
    assert!(current.state.calls.is_empty());
    let request = current.context.build().request;
    let LlmData::ToolResult(result) = &request.input.last().unwrap().data else {
        panic!()
    };
    assert_eq!(result.output[0].value, "call:");
    assert!(current.state.operations.contains_key("unwaited"));
}
#[test]
fn translator_order_follows_waiting_for_not_operation_map_order() {
    let mut current = history();
    begin(&mut current, "Read");
    current
        .apply_recorded(recorded(status(
            &["b", "a", "a"],
            vec![operation("a", "completed"), operation("b", "completed")],
        )))
        .unwrap();
    let built = current.context.build();
    let LlmData::ToolResult(result) = &built.request.input.last().unwrap().data else {
        panic!()
    };
    assert_eq!(result.output[0].value, "call:b,a,a");
}
#[test]
fn missing_waited_operation_has_no_invented_running_result() {
    let mut current = history();
    begin(&mut current, "Read");
    current
        .apply_recorded(recorded(status(&["missing"], Vec::new())))
        .unwrap();
    assert_eq!(current.state.calls.len(), 1);
    assert!(
        !current
            .context
            .build()
            .request
            .input
            .iter()
            .any(|item| matches!(item.data, LlmData::ToolResult(_)))
    );
}
#[test]
fn checkpoints_filter_finished_unneeded_operations_and_share_calls_after_restore() {
    let mut current = history();
    begin(&mut current, "Write");
    current
        .apply_recorded(recorded(status(
            &["needed-done", "pending"],
            vec![
                operation("needed-done", "completed"),
                operation("pending", "running"),
            ],
        )))
        .unwrap();
    current.apply_operation(operation("unused-done", "completed"));
    current.apply_operation(operation("stray-pending", "running"));
    compact(&mut current);
    let state = current.checkpoint().unwrap().unwrap();
    let wire: serde_json::Value = state.decode().unwrap();
    let ids: Vec<_> = wire["Operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| operation["ID"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["needed-done", "pending", "stray-pending"]);
    let mut restored = history();
    assert!(restored.restore_checkpoint(&state));
    let value = restored.state.calls.get(&call_key("turn", "call")).unwrap();
    assert!(Arc::ptr_eq(
        &value.item,
        &restored.context.recorded_call("call").unwrap()
    ));
    assert_eq!(json(&mut current), json(&mut restored));
}
#[test]
fn invalid_coordinator_checkpoint_is_transactional() {
    let mut current = history();
    begin(&mut current, "Read");
    compact(&mut current);
    let state = current.checkpoint().unwrap().unwrap();
    let mut invalid: serde_json::Value = state.decode().unwrap();
    invalid["Builder"] = serde_json::json!({});
    let encoded = Json::parse(&serde_json::to_string(&invalid).unwrap()).unwrap();
    let mut restored = history();
    restored
        .apply_recorded(recorded(input("preserve")))
        .unwrap();
    let before = json(&mut restored);
    assert!(!restored.restore_checkpoint(&encoded));
    assert_eq!(json(&mut restored), before);
    assert_eq!(restored.state.available_inputs, 1);
}
#[test]
fn zero_stop_time_cannot_accelerate_replay() {
    let mut current = history();
    begin(&mut current, "Read");
    compact(&mut current);
    current
        .apply_recorded(Item {
            recorded_at: Timestamp::default(),
            ..recorded(ItemData::Input(Input {
                id: "stop".into(),
                kind: "control".into(),
                payload: Json::parse(r#"{"Mode":"hard","Reason":"stopped"}"#).unwrap(),
                ..Input::default()
            }))
        })
        .unwrap();
    assert!(current.checkpoint().unwrap().is_none());
}
#[test]
fn exactly_three_ordinary_turns_are_not_a_rapid_refill() {
    let mut current = history();
    compact(&mut current);
    for index in 0..3 {
        let id = format!("next-{index}");
        current
            .apply_recorded(recorded(turn(&id, "regular")))
            .unwrap();
        current
            .apply_recorded(recorded(response(&id, vec![message("answer")])))
            .unwrap();
    }
    compact(&mut current);
    assert_eq!(current.state.rapid_refills, 0);
    compact(&mut current);
    assert_eq!(current.state.rapid_refills, 1);
}
fn persist(writer: &mut SessionWriter, current: &mut History, mut data: ItemData) {
    if let ItemData::Turn(turn) = &mut data {
        turn.previous_turn_id = current.state.current_turn.clone();
    }
    let sequence = writer.append_item(data.clone(), at()).unwrap();
    current
        .apply_recorded(Item {
            sequence,
            recorded_at: at(),
            data,
        })
        .unwrap();
}
#[test]
fn real_store_consumer_checkpoint_and_full_fallback_restore_identical_context() {
    for valid in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let mut writer = store
            .create_new(Session {
                id: "session".into(),
                created_at: at(),
            })
            .unwrap();
        let mut live = history();
        for data in [
            input("before"),
            turn("turn", "regular"),
            response("turn", vec![message("answer")]),
            turn("compact", "compaction"),
            response("compact", vec![message("summary")]),
        ] {
            persist(&mut writer, &mut live, data);
        }
        let mut value: serde_json::Value = live.checkpoint().unwrap().unwrap().decode().unwrap();
        if !valid {
            value["Version"] = 2.into();
        }
        writer
            .save_checkpoint(Json::parse(&serde_json::to_string(&value).unwrap()).unwrap())
            .unwrap();
        persist(&mut writer, &mut live, input("after"));
        drop(writer);
        let path = directory.path().join("session.session.jsonl");
        let log = SessionLog::open(&path).unwrap();
        let resume = SessionRecovery::open(&path).unwrap().into_resume().unwrap();
        assert!(resume.checkpoint.is_some());
        let mut restored = history();
        let recovery = restored.recover(&log, resume).unwrap();
        assert_eq!(recovery.used_checkpoint, valid);
        assert_eq!(recovery.external_input_ids, ["before", "after"]);
        let mut full = history();
        assert!(
            !full
                .recover(&log, log.resume_full().unwrap())
                .unwrap()
                .used_checkpoint
        );
        assert_eq!(json(&mut restored), json(&mut full));
    }
}

fn empty_log() -> (tempfile::TempDir, SessionLog) {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    drop(
        store
            .create_new(Session {
                id: "session".into(),
                created_at: at(),
            })
            .unwrap(),
    );
    let log = SessionLog::open(directory.path().join("session.session.jsonl")).unwrap();
    (directory, log)
}
#[test]
fn accelerated_replay_rejects_source_changes_before_mutating_context() {
    use std::io::Write;
    let (directory, log) = empty_log();
    let mut resume = log.resume_full().unwrap();
    resume.checkpoint = Some(kou_store::ResumeCheckpoint {
        state: Json::parse("{}").unwrap(),
        after: 0,
        items: Vec::new(),
    });
    std::fs::OpenOptions::new()
        .append(true)
        .open(directory.path().join("session.session.jsonl"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let mut current = history();
    let before = json(&mut current);
    assert!(current.recover(&log, resume).is_err());
    assert_eq!(json(&mut current), before);
}
#[test]
fn recovery_cannot_mix_sessions() {
    let (_directory, log) = empty_log();
    let mut resume = log.resume_full().unwrap();
    resume.snapshot.session.id = "other-session".into();
    let mut current = history();
    assert!(
        matches!(current.recover(&log, resume), Err(error) if error.to_string() == "resume state belongs to a different session")
    );
}
#[test]
fn latest_verification_is_restored_without_starting_it() {
    let (_directory, log) = empty_log();
    let mut resume = log.resume_full().unwrap();
    let mut verification = operation("verification", "awaiting");
    verification.idempotency = Json::parse(r#"{"harness":"verification"}"#).unwrap();
    resume.operations.push(verification);
    let mut current = history();
    current.recover(&log, resume).unwrap();
    assert_eq!(current.verifying_operation().unwrap().id, "verification");
}
