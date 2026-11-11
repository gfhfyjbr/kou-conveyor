use super::*;
use kou_domain::Session;
use kou_runtime::{
    operation::{self as runtime, SkillReadLimits, decode_skill},
    tool::{Skill, SkillCatalog, SkillUse, Translator},
};
use kou_store::{SessionRecovery, SessionStore};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Skills {
    translator: SkillUse,
}
impl ReplayTools for Skills {
    fn available(&self, name: &str) -> bool {
        name == "SkillUse"
    }
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        self.translator
            .result(&call.call_id, status, operations)
            .map(|result| result.output)
            .map_err(|error| Error(error.to_string()))
    }
}
impl LiveTools for Skills {
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus {
        self.translator.translate_call(call, &mut |spec| {
            submission.submit(OperationSpec {
                kind: spec.kind,
                version: spec.version,
                state: spec.state,
                idempotency: spec.idempotency,
                max_output_length: spec.max_output_length,
            })
        })
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
            "skill Add preceded its durable initial status"
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
            return Box::pin(async { Err(Error("owned skill journal refused".into())) });
        }
        self.inner.append(data, at)
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        let state = decode_skill(&operation).unwrap();
        if matches!(self.failure, "initial") && state.content.is_empty()
            || self.failure == "chunk" && state.content.len() == 16
            || self.failure == "complete" && operation.terminal()
        {
            return Box::pin(async { Err(Error("owned skill journal refused".into())) });
        }
        self.inner.save_operation(operation)
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        self.inner.checkpoint(state)
    }
}
fn limits() -> runtime::Limits {
    runtime::Limits {
        workers: 1,
        skill_read: SkillReadLimits {
            content_bytes: 32,
            chunk_bytes: 8,
        },
        ..runtime::Limits::default()
    }
}

#[tokio::test]
async fn native_skills_use_real_coordinator_receipts_and_recover_last_durable_prefix() {
    for failure in ["", "response", "status", "initial", "chunk", "complete"] {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("sessions")).unwrap();
        let writer = store
            .create_new(Session {
                id: "native-skill".into(),
                created_at: Timestamp::parse("2026-11-10T12:00:00Z").unwrap(),
            })
            .unwrap();
        let log = directory.path().join("sessions/native-skill.session.jsonl");
        let path = directory.path().join("SKILL.md");
        let staged = directory.path().join("staged-skill");
        let data = b"abcdefghijklmnopqrstuvwxyz012345";
        fs::write(&staged, data).unwrap();
        let adds = Arc::new(AtomicUsize::new(0));
        let (manager, mut updates) = runtime::LocalManager::new(limits()).unwrap();
        let catalog = SkillCatalog::default();
        catalog
            .register(Skill {
                name: "review".into(),
                path: path.to_string_lossy().into(),
                description: "private fixture".into(),
                manual: true,
            })
            .unwrap();
        assert!(
            !path.exists(),
            "registration read or created the skill file"
        );
        let registry = Arc::new(Skills {
            translator: SkillUse::new(catalog),
        });
        let mut run = Rig::custom(Config::default(), |mut driver| {
            driver.registry = registry.clone();
            driver.history = History::new(Builder::default(), registry);
            driver.journal = Box::new(Fault {
                inner: FileJournal::new(writer).unwrap(),
                failure,
            });
            driver.operations = Box::new(Native {
                manager,
                log: log.clone(),
                adds: adds.clone(),
            });
            driver
        });
        let handle = run.updates.clone();
        let read_path = path.clone();
        let bridge = tokio::spawn(async move {
            let mut initial = true;
            while let Some(update) = updates.next().await {
                assert!(update.requires_acknowledgement());
                if initial {
                    assert_eq!(update.operation.status, "awaiting");
                    assert!(decode_skill(&update.operation).unwrap().content.is_empty());
                    assert!(!read_path.exists());
                    // The reader cannot open before this receipt is returned.
                    fs::rename(&staged, &read_path).unwrap();
                    initial = false;
                }
                let result = handle.submit_committed((*update.operation).clone()).await;
                let refused = result.is_err();
                update.acknowledge(result.map_err(|error| runtime::Error(error.to_string())));
                if refused {
                    break;
                }
            }
        });
        run.send("one", "read the private skill fixture").await;
        let mut response = calls(&[("skill", "SkillUse")]);
        let kou_domain::LlmData::ToolCall(call) = &mut response.output.as_mut().unwrap()[0].data
        else {
            panic!("fixture reply must be a tool call");
        };
        call.arguments = r#"{"name":"review"}"#.into();
        reply(run.request().await, response);
        if failure.is_empty() {
            let answer = run.request().await;
            assert_eq!(result(&answer.request, "skill").unwrap().as_bytes(), data);
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
                error.to_string().contains("owned skill journal refused"),
                "{failure}: {error}"
            );
        }
        tokio::time::timeout(Duration::from_secs(6), bridge)
            .await
            .unwrap()
            .unwrap();
        if matches!(failure, "response" | "status") {
            assert_eq!(adds.load(Ordering::Relaxed), 0);
            assert!(!path.exists());
        } else {
            assert_eq!(adds.load(Ordering::Relaxed), 1);
        }
        if matches!(failure, "initial" | "chunk" | "complete") {
            let resume = SessionRecovery::open(&log).unwrap().into_resume().unwrap();
            assert_eq!(resume.operations.len(), 1);
            let pending = resume.operations.into_iter().next().unwrap();
            let length = decode_skill(&pending).unwrap().content.len();
            assert_eq!(
                length,
                match failure {
                    "initial" => 0,
                    "chunk" => 8,
                    _ => 32,
                }
            );
            assert_eq!(
                pending.status,
                if failure == "initial" {
                    "ready"
                } else {
                    "awaiting"
                }
            );
            let mut changed = data.to_vec();
            changed[..length].fill(b'X');
            fs::write(&path, &changed).unwrap();
            let mut writer = store.open_writer("native-skill").unwrap();
            let (mut manager, mut updates) = runtime::LocalManager::new(limits()).unwrap();
            manager.add(pending).unwrap();
            loop {
                let update = tokio::time::timeout(Duration::from_secs(3), updates.next())
                    .await
                    .unwrap()
                    .unwrap();
                let terminal = update.operation.terminal();
                writer.save_operation((*update.operation).clone()).unwrap();
                if terminal {
                    assert_eq!(update.operation.status, "completed");
                    assert_eq!(decode_skill(&update.operation).unwrap().content, data);
                }
                update.acknowledge(Ok(()));
                if terminal {
                    break;
                }
            }
            drop(manager);
            assert!(updates.next().await.is_none());
        }
        let _reopened = store.open_writer("native-skill").unwrap();
    }
}
