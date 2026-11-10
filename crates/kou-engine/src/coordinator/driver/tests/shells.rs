use super::*;
use kou_domain::{Record, Session};
use kou_runtime::operation::{
    self as runtime,
    shell::{ShellInput, decode_shell, shell_spec},
};
use kou_store::{SessionRecovery, SessionStore};
use rustix::process::{Pid, test_kill_process_group};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicI32, AtomicUsize, Ordering},
};

struct Shells {
    base: PathBuf,
}
impl ReplayTools for Shells {
    fn available(&self, name: &str) -> bool {
        name == "Bash"
    }
    fn result(
        &self,
        _: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        let value = if !status.error.is_empty() {
            status.error.clone()
        } else {
            let state = decode_shell(&operations[0]).map_err(|error| Error(error.to_string()))?;
            state.result.map_or(state.terminal_error, |result| {
                format!("out:{}; err:{}", result.out, result.err)
            })
        };
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value,
        }])
    }
}
impl LiveTools for Shells {
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus {
        let input: ShellInput = serde_json::from_str(&call.arguments).unwrap();
        let spec = shell_spec(input, self.base.to_str().unwrap().into(), 128).unwrap();
        let id = submission.submit(OperationSpec {
            kind: spec.kind,
            version: spec.version,
            state: spec.state,
            idempotency: spec.idempotency,
            max_output_length: spec.max_output_length,
        });
        CallStatus {
            waiting_for: Some(vec![id]),
            ..CallStatus::default()
        }
    }
}
struct Native {
    manager: runtime::LocalManager,
    log: PathBuf,
    adds: Arc<AtomicUsize>,
}
impl Operations for Native {
    fn add(&mut self, operation: Operation) -> Result<()> {
        let log = SessionLog::open(&self.log).unwrap();
        assert!(
            (1..=log.item_count() as u64).any(|sequence| matches!(
                log.item(sequence).unwrap().data,
                ItemData::ToolCallStatus(status) if status.operations.contains(&operation)
            )),
            "shell Add preceded its durable initial status"
        );
        self.adds.fetch_add(1, Ordering::Relaxed);
        self.manager
            .add(operation)
            .map_err(|error| Error(error.to_string()))
    }
    fn cancel(&mut self, id: &str, reason: &str) -> Result<()> {
        self.manager
            .cancel(id, reason)
            .map_err(|error| Error(error.to_string()))
    }
}
struct Fault {
    inner: FileJournal,
    failure: &'static str,
    owned_pid: Arc<AtomicI32>,
}
impl Journal for Fault {
    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>> {
        if matches!(
            (&data, self.failure),
            (ItemData::ModelResponse(_), "response") | (ItemData::ToolCallStatus(_), "status")
        ) {
            return Box::pin(async { Err(Error("owned shell journal refused".into())) });
        }
        self.inner.append(data, at)
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        let state = decode_shell(&operation).unwrap();
        if state.process_group_id > 1 {
            self.owned_pid
                .store(state.process_group_id as i32, Ordering::Release);
        }
        if !self.failure.is_empty()
            && (state.phase == self.failure
                || (self.failure == "started"
                    && state.phase == "process"
                    && state.process_group_id > 1))
        {
            return Box::pin(async { Err(Error("owned shell journal refused".into())) });
        }
        self.inner.save_operation(operation)
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        self.inner.checkpoint(state)
    }
}

#[tokio::test]
async fn native_shell_checkpoints_wait_for_real_coordinator_journal_acknowledgements() {
    for failure in [
        "",
        "response",
        "status",
        "create_directory",
        "create_out",
        "create_err",
        "process",
        "started",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let writer = store
            .create_new(Session {
                id: "native-shell".into(),
                created_at: Timestamp::parse("2026-11-10T02:00:00Z").unwrap(),
            })
            .unwrap();
        let log = directory.path().join("native-shell.session.jsonl");
        let base = directory.path().join("spool");
        fs::create_dir(&base).unwrap();
        let marker = directory.path().join("launched");
        let adds = Arc::new(AtomicUsize::new(0));
        let owned_pid = Arc::new(AtomicI32::new(0));
        let (manager, mut updates) = runtime::LocalManager::new(runtime::Limits {
            workers: 1,
            ..runtime::Limits::default()
        })
        .unwrap();
        let registry = Arc::new(Shells { base: base.clone() });
        let mut run = Rig::custom(Config::default(), |mut driver| {
            driver.registry = registry.clone();
            driver.history = History::new(Builder::default(), registry);
            driver.journal = Box::new(Fault {
                inner: FileJournal::new(writer).unwrap(),
                failure,
                owned_pid: owned_pid.clone(),
            });
            driver.operations = Box::new(Native {
                manager,
                log: log.clone(),
                adds: adds.clone(),
            });
            driver
        });
        let handle = run.updates.clone();
        let capture_base = base.clone();
        let bridge = tokio::spawn(async move {
            while let Some(update) = updates.next().await {
                let state = decode_shell(&update.operation).unwrap();
                let artifacts = capture_base.join(&update.operation.id);
                if state.phase == "create_directory" {
                    assert!(!artifacts.exists());
                }
                if state.phase == "create_out" {
                    assert!(!artifacts.join("out").exists());
                }
                if state.phase == "create_err" {
                    assert!(!artifacts.join("err").exists());
                }
                let result = handle.submit_committed((*update.operation).clone()).await;
                let refused = result.is_err();
                update.acknowledge(result.map_err(|error| runtime::Error(error.to_string())));
                if refused {
                    break;
                }
            }
        });
        run.send("one", "run the private shell fixture").await;
        let mut response = calls(&[("shell", "Bash")]);
        let LlmData::ToolCall(call) = &mut response.output.as_mut().unwrap()[0].data else {
            unreachable!()
        };
        let command = if failure == "started" {
            format!(
                "printf launched > '{}'; while :; do /bin/sleep 1; done",
                marker.display()
            )
        } else {
            format!(
                "printf launched > '{}'; printf shell-out; /usr/bin/head -c 200000 /dev/zero; printf shell-err >&2",
                marker.display()
            )
        };
        call.arguments = serde_json::to_string(&ShellInput {
            command,
            shell: "/bin/sh".into(),
            ..ShellInput::default()
        })
        .unwrap();
        reply(run.request().await, response);
        if failure.is_empty() {
            let answer = run.request().await;
            let output = result(&answer.request, "shell").unwrap();
            assert!(output.contains("shell-out") && output.contains("shell-err"));
            assert!(marker.exists());
            let records: Vec<_> = fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter_map(|line| match Record::decode(line.as_bytes()).unwrap() {
                    Record::Operation(record) => Some(record.operation),
                    _ => None,
                })
                .collect();
            let completed = records.last().unwrap();
            assert_eq!(completed.status, "completed");
            let state = decode_shell(completed).unwrap();
            assert_eq!(fs::metadata(&state.out_path).unwrap().len(), 200_009);
            assert_eq!(fs::metadata(&state.err_path).unwrap().len(), 9);
            assert!(
                records
                    .iter()
                    .any(|record| decode_shell(record).unwrap().process_group_id > 1)
            );
            assert!(
                SessionRecovery::open(&log)
                    .unwrap()
                    .into_resume()
                    .unwrap()
                    .operations
                    .is_empty()
            );
            run.control("when_idle").await;
            reply(answer, text("done"));
            run.task.await.unwrap().unwrap();
        } else {
            let error = tokio::time::timeout(Duration::from_secs(6), run.task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert!(
                error.to_string().contains("owned shell journal refused"),
                "{failure}: {error}"
            );
            if failure != "started" {
                assert!(
                    !marker.exists(),
                    "{failure}: process crossed a refused checkpoint"
                );
            }
            let resume = SessionRecovery::open(&log).unwrap().into_resume().unwrap();
            if matches!(failure, "response" | "status") {
                assert_eq!(adds.load(Ordering::Relaxed), 0);
            } else {
                assert_eq!(adds.load(Ordering::Relaxed), 1);
                assert_eq!(resume.operations.len(), 1);
                let restored = &resume.operations[0];
                let state = decode_shell(restored).unwrap();
                if failure == "started" {
                    assert_eq!(state.phase, "process");
                    assert_eq!(state.process_group_id, 0);
                    fs::write(&state.out_path, "preserve after interruption").unwrap();
                    let (mut restarted, mut updates) =
                        runtime::LocalManager::new(runtime::Limits::default()).unwrap();
                    restarted.add(restored.clone()).unwrap();
                    let failed = tokio::time::timeout(Duration::from_secs(3), updates.next())
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(failed.operation.status, "failed");
                    assert!(
                        decode_shell(&failed.operation)
                            .unwrap()
                            .terminal_error
                            .contains("unknown")
                    );
                    failed.acknowledge(Ok(()));
                    drop(restarted);
                    assert_eq!(
                        fs::read_to_string(&state.out_path).unwrap(),
                        "preserve after interruption"
                    );
                }
                let artifacts = base.join(&restored.id);
                match failure {
                    "create_directory" => assert!(!artifacts.exists()),
                    "create_out" => assert!(!artifacts.join("out").exists()),
                    "create_err" => assert!(!artifacts.join("err").exists()),
                    "process" => assert_eq!(fs::metadata(artifacts.join("out")).unwrap().len(), 0),
                    _ => {}
                }
            }
        }
        tokio::time::timeout(Duration::from_secs(6), bridge)
            .await
            .unwrap()
            .unwrap();
        let pid = owned_pid.load(Ordering::Acquire);
        if pid > 1 {
            assert_eq!(
                test_kill_process_group(Pid::from_raw(pid).unwrap()),
                Err(rustix::io::Errno::SRCH)
            );
        }
        let _reopened = store.open_writer("native-shell").unwrap();
    }
}
