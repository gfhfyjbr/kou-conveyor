use super::*;
use kou_domain::Session;
use kou_runtime::operation as runtime;
use kou_store::{SessionRecovery, SessionStore};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Files;
impl ReplayTools for Files {
    fn available(&self, name: &str) -> bool {
        name == "Write"
    }
    fn result(
        &self,
        _: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        if !status.error.is_empty() {
            return Ok(vec![ToolResultOutput {
                kind: "text".into(),
                value: status.error.clone(),
            }]);
        }
        let state =
            runtime::decode_file(&operations[0]).map_err(|error| Error(error.to_string()))?;
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value: state
                .result
                .map_or_else(|| state.terminal_error.clone(), |result| result.text),
        }])
    }
    fn running(&self, _: &ToolCall, _: &[Arc<Operation>]) -> String {
        "file operation is running".into()
    }
}
impl LiveTools for Files {
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus {
        let state: runtime::FileState = serde_json::from_str(&call.arguments).unwrap();
        let spec = runtime::file_spec(state).unwrap();
        let id = submission.submit(OperationSpec {
            kind: spec.kind,
            version: spec.version,
            state: spec.state,
            idempotency: spec.idempotency,
            max_output_length: 0,
        });
        CallStatus {
            waiting_for: Some(vec![id]),
            ..CallStatus::default()
        }
    }
}
struct NativeOperations {
    manager: runtime::LocalManager,
    log: PathBuf,
    adds: Arc<AtomicUsize>,
}
impl Operations for NativeOperations {
    fn add(&mut self, operation: Operation) -> Result<()> {
        let log = SessionLog::open(&self.log).unwrap();
        assert!(
            (1..=log.item_count() as u64)
                .any(|sequence| matches!(log.item(sequence).unwrap().data,
            ItemData::ToolCallStatus(status) if status.operations.contains(&operation))),
            "native effect crossed a missing durable status"
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
struct FaultJournal {
    inner: FileJournal,
    failure: &'static str,
}
impl Journal for FaultJournal {
    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>> {
        let fails = matches!(
            (&data, self.failure),
            (ItemData::ModelResponse(_), "response") | (ItemData::ToolCallStatus(_), "status")
        );
        if fails {
            return Box::pin(async { Err(Error("injected durable failure".into())) });
        }
        self.inner.append(data, at)
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        self.inner.save_operation(operation)
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        self.inner.checkpoint(state)
    }
}

#[tokio::test]
async fn native_file_effects_follow_real_journal_acknowledgements_and_recover() {
    for failure in ["", "response", "status"] {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let writer = store
            .create_new(Session {
                id: "native-runtime".into(),
                created_at: Timestamp::parse("2026-11-10T02:00:00Z").unwrap(),
            })
            .unwrap();
        let log = directory.path().join("native-runtime.session.jsonl");
        let target = directory.path().join("owned.txt");
        let adds = Arc::new(AtomicUsize::new(0));
        let (manager, mut runtime_updates) =
            runtime::LocalManager::new(runtime::Limits::default()).unwrap();
        let mut run = Rig::custom(Config::default(), |mut driver| {
            driver.registry = Arc::new(Files);
            driver.history = History::new(Builder::default(), Arc::new(Files));
            driver.journal = Box::new(FaultJournal {
                inner: FileJournal::new(writer).unwrap(),
                failure,
            });
            driver.operations = Box::new(NativeOperations {
                manager,
                log: log.clone(),
                adds: adds.clone(),
            });
            driver
        });
        let handle = run.updates.clone();
        let bridge = tokio::spawn(async move {
            while let Some(update) = runtime_updates.next().await {
                if handle.submit((*update.operation).clone()).await.is_err() {
                    break;
                }
                drop(update);
            }
        });
        run.send("one", "write the private fixture").await;
        let mut response = calls(&[("write", "Write")]);
        let LlmData::ToolCall(call) = &mut response.output.as_mut().unwrap()[0].data else {
            unreachable!()
        };
        call.arguments =
            serde_json::json!({"Action":"write", "Path":target, "Content":"owned body"})
                .to_string();
        reply(run.request().await, response);
        if failure.is_empty() {
            let answer = run.request().await;
            assert!(
                result(&answer.request, "write")
                    .unwrap()
                    .starts_with("Wrote ")
            );
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "owned body");
            assert_eq!(adds.load(Ordering::Relaxed), 1);
            let records = std::fs::read_to_string(&log).unwrap();
            let states: Vec<_> = records
                .lines()
                .filter_map(
                    |line| match kou_domain::Record::decode(line.as_bytes()).unwrap() {
                        kou_domain::Record::Operation(record) => Some(record.operation.status),
                        _ => None,
                    },
                )
                .collect();
            assert_eq!(states, ["awaiting", "completed"]);
            let resume = SessionRecovery::open(&log).unwrap().into_resume().unwrap();
            // Completed, delivered operations are cold: recovery does not retain
            // their bodies or ask the manager to run them again.
            assert!(resume.operations.is_empty());
            run.control("when_idle").await;
            reply(answer, text("done"));
            run.task.await.unwrap().unwrap();
        } else {
            assert!(
                run.task
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("injected durable failure")
            );
            assert!(!target.exists());
            assert_eq!(adds.load(Ordering::Relaxed), 0);
        }
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .unwrap()
            .unwrap();
        let _reopened = store.open_writer("native-runtime").unwrap();
    }
}
