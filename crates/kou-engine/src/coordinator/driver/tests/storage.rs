use super::*;
use kou_domain::Session;
use kou_store::{SessionRecovery, SessionStore};
use std::path::PathBuf;

struct FileOperations {
    path: PathBuf,
    saved: Memory,
    dispatched: mpsc::UnboundedSender<Operation>,
}
impl Operations for FileOperations {
    fn add(&mut self, operation: Operation) -> Result<()> {
        let log = SessionLog::open(&self.path).unwrap();
        assert!((1..=log.item_count() as u64).any(|sequence| {
            matches!(log.item(sequence).unwrap().data, ItemData::ToolCallStatus(status) if status.operations.contains(&operation))
        }), "operation was not durable before Add");
        self.saved.0.lock().unwrap().adds.push(operation.clone());
        self.dispatched.send(operation).unwrap();
        Ok(())
    }
    fn cancel(&mut self, id: &str, reason: &str) -> Result<()> {
        Operations::cancel(&mut self.saved, id, reason)
    }
}

#[tokio::test]
async fn live_file_journal_recovery_resubmits_the_same_operation_not_the_call() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let writer = store
        .create_new(Session {
            id: "live-fixture".into(),
            created_at: Timestamp::parse("2026-11-10T02:00:00Z").unwrap(),
        })
        .unwrap();
    let path = directory.path().join("live-fixture.session.jsonl");
    let (sent, mut dispatched) = mpsc::unbounded_channel();
    let mut first = Rig::custom(Config::default(), |mut driver| {
        driver.journal = Box::new(FileJournal::new(writer).unwrap());
        driver.operations = Box::new(FileOperations {
            path: path.clone(),
            saved: Memory::default(),
            dispatched: sent.clone(),
        });
        driver
    });
    first.send("one", "work").await;
    reply(first.request().await, calls(&[("a", "Bash")]));
    let original = tokio::time::timeout(Duration::from_secs(5), dispatched.recv())
        .await
        .unwrap()
        .unwrap();
    first.abort().await;

    let writer = store.open_writer("live-fixture").unwrap();
    let log = SessionLog::open(&path).unwrap();
    let restored = SessionRecovery::open(&path).unwrap().into_resume().unwrap();
    let mut second = Rig::custom(Config::default(), |mut driver| {
        driver.journal = Box::new(FileJournal::new(writer).unwrap());
        driver.operations = Box::new(FileOperations {
            path: path.clone(),
            saved: Memory::default(),
            dispatched: sent,
        });
        driver.recover(&log, restored).unwrap();
        driver
    });
    let resumed = tokio::time::timeout(Duration::from_secs(5), dispatched.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original, resumed);
    assert!(second.requests.try_recv().is_err());
    let mut completed = resumed;
    completed.status = "completed".into();
    second.updates.submit(completed).await.unwrap();
    let answer = second.request().await;
    assert_eq!(result(&answer.request, "a").as_deref(), Some("completed"));
    second.control("when_idle").await;
    reply(answer, text("done"));
    second.task.await.unwrap().unwrap();
    // Both consumed runs joined their writer and released the actual lock.
    // Operations are fake; replaying an ID is not proof of exactly-once effects.
    let _reopened = store.open_writer("live-fixture").unwrap();
    let log = SessionLog::open(&path).unwrap();
    let statuses: Vec<_> = (1..=log.item_count() as u64)
        .filter_map(|sequence| match log.item(sequence).unwrap().data {
            ItemData::ToolCallStatus(status) => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(statuses.len(), 2);
    assert_eq!(statuses[0].operations[0].id, statuses[1].operations[0].id);
    assert_eq!(statuses[1].operations[0].status, "completed");
}

#[tokio::test]
async fn file_journal_checkpoints_only_a_successfully_compacted_conversation() {
    for (answer, kept) in [
        ("<summary></summary>", false),
        ("<review>only</review>", false),
        ("<review>unfinished", false),
        ("<summary>kept task</summary>", true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path()).unwrap();
        let writer = store
            .create_new(Session {
                id: "live-fixture".into(),
                created_at: Timestamp::parse("2026-11-10T02:00:00Z").unwrap(),
            })
            .unwrap();
        let path = directory.path().join("live-fixture.session.jsonl");
        let mut run = Rig::custom(Config::default(), |mut driver| {
            driver.journal = Box::new(FileJournal::new(writer).unwrap());
            driver
        });
        run.send("one", "original task").await;
        reply(run.request().await, text("original answer"));
        run.control("compact").await;
        let compact = run.request().await;
        run.control("when_idle").await;
        reply(compact, text(answer));
        run.task.await.unwrap().unwrap();
        let recovery = SessionRecovery::open(&path).unwrap();
        assert_eq!(recovery.checkpoint_after().is_some(), kept, "{answer}");
        let restored = recovery.into_resume().unwrap();
        let log = SessionLog::open(&path).unwrap();
        let mut context = History::new(Builder::default(), Arc::new(Tools));
        let result = context.recover(&log, restored).unwrap();
        assert_eq!(result.used_checkpoint, kept, "{answer}");
        let built = context.context.build();
        assert!(has_text(
            &built.request,
            if kept { "kept task" } else { "original answer" }
        ));
        let responses: Vec<_> = (1..=log.item_count() as u64)
            .filter_map(|sequence| match log.item(sequence).unwrap().data {
                ItemData::ModelResponse(response) => Some(response),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[1].compacted.is_some(), kept);
    }
}
