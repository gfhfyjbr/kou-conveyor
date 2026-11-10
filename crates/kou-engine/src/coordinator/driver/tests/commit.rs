use super::*;

struct GatedJournal {
    inner: Memory,
    entered: Option<oneshot::Sender<Operation>>,
    release: Option<oneshot::Receiver<Result<()>>>,
}
impl Journal for GatedJournal {
    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>> {
        self.inner.append(data, at)
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        let entered = self.entered.take().unwrap();
        let release = self.release.take().unwrap();
        Box::pin(async move {
            entered.send(operation.clone()).unwrap();
            release
                .await
                .map_err(|_| Error("journal gate closed".into()))??;
            self.inner.save_operation(operation).await
        })
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        self.inner.checkpoint(state)
    }
}
fn gated() -> (
    Rig,
    oneshot::Receiver<Operation>,
    oneshot::Sender<Result<()>>,
    Memory,
) {
    let (entered, receive) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let saved = Memory::default();
    let run = Rig::custom(Config::default(), |mut driver| {
        driver.operations = Box::new(saved.clone());
        driver.journal = Box::new(GatedJournal {
            inner: saved.clone(),
            entered: Some(entered),
            release: Some(wait),
        });
        driver
    });
    (run, receive, release, saved)
}

#[tokio::test(start_paused = true)]
async fn committed_submission_waits_for_the_write_and_preserves_journal_errors() {
    for failure in [false, true] {
        let (mut run, entered, release, saved) = gated();
        run.send("one", "work").await;
        reply(run.request().await, calls(&[("call", "Bash")]));
        settle().await;
        let mut operation = saved.0.lock().unwrap().adds[0].clone();
        operation.status = "awaiting".into();
        let handle = run.updates.clone();
        let submitted = operation.clone();
        let task = tokio::spawn(async move { handle.submit_committed(submitted).await });
        assert_eq!(entered.await.unwrap(), operation);
        assert!(
            !task.is_finished(),
            "admission was mistaken for a durable write"
        );
        assert!(saved.0.lock().unwrap().updates.is_empty());
        release
            .send(if failure {
                Err(Error("owned journal refused".into()))
            } else {
                Ok(())
            })
            .unwrap();
        let received = task.await.unwrap();
        if failure {
            let expected = format!("store operation {:?}: owned journal refused", operation.id);
            assert_eq!(received.unwrap_err().to_string(), expected);
            assert_eq!(run.task.await.unwrap().unwrap_err().to_string(), expected);
        } else {
            received.unwrap();
            assert_eq!(saved.0.lock().unwrap().updates, [operation]);
            run.abort().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn caller_abort_does_not_withdraw_an_update_and_driver_abort_cannot_ack_it() {
    for abort_caller in [true, false] {
        let (run, entered, release, saved) = gated();
        let handle = run.updates.clone();
        let task = tokio::spawn(async move {
            let mut operation = spec().operation("inert-op".into());
            operation.status = "awaiting".into();
            handle.submit_committed(operation).await
        });
        entered.await.unwrap();
        if abort_caller {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            release.send(Ok(())).unwrap();
            run.control("heartbeat").await;
            settle().await;
            assert_eq!(saved.0.lock().unwrap().updates.len(), 1);
            assert_eq!(saved.0.lock().unwrap().updates[0].id, "inert-op");
            run.abort().await;
        } else {
            run.task.abort();
            assert!(run.task.await.unwrap_err().is_cancelled());
            assert_eq!(
                task.await.unwrap().unwrap_err().to_string(),
                "coordinator operation acknowledgement was dropped"
            );
            assert!(release.send(Ok(())).is_err());
        }
    }
}
