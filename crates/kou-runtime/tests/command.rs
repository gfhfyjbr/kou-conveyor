use kou_domain::{CallStatus, ItemData, Session, Timestamp, ToolCall, ToolCallStatus, Turn};
use kou_runtime::{
    operation::{Limits, LocalManager, shell::decode_shell},
    tool::{Command, CommandConfig, Translator},
};
use kou_store::{SessionLog, SessionStore};
use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};

#[tokio::test]
async fn command_tools_cross_real_journal_receipts_before_native_execution() {
    for refuse_process in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let at = Timestamp::parse("2026-11-10T12:00:00Z").unwrap();
        let store = SessionStore::new(root.join("sessions")).unwrap();
        let mut writer = store
            .create_new(Session {
                id: "command-test".into(),
                created_at: at,
            })
            .unwrap();
        let argument = "it's $HOME `quoted`";
        let note = "quoted ' $HOME `literal`\nsecond line";
        let call_id = "call'; printf PWN > injected; #";
        let arguments = "{\n  \"text\": \"line\\nKOU_CONVEYOR_TOOL_ARGUMENTS\\n'quote'\"\n}";
        let compact = r#"{"text":"line\nKOU_CONVEYOR_TOOL_ARGUMENTS\n'quote'"}"#;
        let command = Command::new(CommandConfig {
            name: "Echo".into(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf invoked > effect; printf '%s|%s|%s|%s|' \"$1\" \"$KOU_CONVEYOR_TOOL_NAME\" \"$KOU_CONVEYOR_TOOL_CALL_ID\" \"$PLUGIN_NOTE\"; cat; pwd; printf stderr >&2".into(),
                "fixture".into(),
                argument.into(),
            ],
            directory: root.to_string_lossy().into(),
            base_directory: root.to_string_lossy().into(),
            environment: BTreeMap::from([
                ("PLUGIN_NOTE".into(), note.into()),
                ("KOU_CONVEYOR_TOOL_NAME".into(), "wrong tool".into()),
                ("KOU_CONVEYOR_TOOL_CALL_ID".into(), "wrong call".into()),
            ]),
            ..CommandConfig::default()
        })
        .unwrap();
        let mut specs = Vec::new();
        let status = command.translate_call(
            &ToolCall {
                call_id: call_id.into(),
                name: "mcp__plugin_Echo".into(),
                arguments: arguments.into(),
            },
            &mut |spec| {
                specs.push(spec);
                "owned".into()
            },
        );
        assert!(status.error.is_empty(), "{}", status.error);
        let current = specs.pop().unwrap().operation("owned".into());
        assert!(!root.join("owned").exists() && !root.join("effect").exists());
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
        writer
            .append_item(
                ItemData::ToolCallStatus(ToolCallStatus {
                    turn_id: "turn".into(),
                    call_id: call_id.into(),
                    status,
                    operations: vec![current.clone()],
                }),
                at,
            )
            .unwrap();
        let (mut manager, mut updates) = LocalManager::new(Limits::default()).unwrap();
        manager.add(current).unwrap();
        let mut refused = false;
        let mut checkpoints = 0;
        let completed = loop {
            let update = tokio::time::timeout(Duration::from_secs(5), updates.next())
                .await
                .expect("private command fixture deadline")
                .expect("command updates closed before completion");
            let current = update.operation.clone();
            let state = decode_shell(&current).unwrap();
            checkpoints += 1;
            assert!(checkpoints <= 13);
            if state.phase == "process"
                && state.process_group_id == 0
                && current.status == "awaiting"
            {
                assert!(!root.join("effect").exists());
                if refuse_process {
                    assert!(!refused && update.requires_acknowledgement());
                    refused = true;
                    update.acknowledge(Err(kou_runtime::operation::Error(
                        "injected command journal refusal".into(),
                    )));
                    continue;
                }
            }
            writer.save_operation((*current).clone()).unwrap();
            update.acknowledge(Ok(()));
            if matches!(current.status.as_str(), "completed" | "failed" | "canceled") {
                break current;
            }
        };
        drop(manager);
        assert!(!root.join("injected").exists());
        assert_eq!(refused, refuse_process);
        let state = decode_shell(&completed).unwrap();
        let result = command
            .result("call", &CallStatus::default(), &[Arc::clone(&completed)])
            .unwrap();
        if refuse_process {
            assert_eq!(completed.status, "failed");
            assert!(!root.join("effect").exists());
            assert!(state.result.is_none());
            assert!(
                result.output[0]
                    .value
                    .contains("injected command journal refusal")
            );
        } else {
            assert_eq!(completed.status, "completed", "{state:?}");
            assert_eq!(fs::read(root.join("effect")).unwrap(), b"invoked");
            let expected = format!(
                "{argument}|Echo|{call_id}|{note}|{compact}\n{}\n",
                root.display()
            );
            let captured = state.result.unwrap();
            assert_eq!(captured.out, expected);
            assert_eq!(captured.err, "stderr");
            assert_eq!(fs::read_to_string(state.out_path).unwrap(), expected);
            assert_eq!(
                result.output[0].value,
                format!("{expected}\nStderr:\nstderr")
            );
        }
        let resume = SessionLog::open(root.join("sessions/command-test.session.jsonl"))
            .unwrap()
            .resume_full()
            .unwrap();
        let durable = resume
            .operations
            .iter()
            .find(|value| value.id == "owned")
            .unwrap();
        assert_eq!(durable, completed.as_ref());
    }
}
