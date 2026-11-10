use super::*;
use kou_tools::process::CancelToken;
use std::io;

#[derive(Default)]
enum Outcome {
    #[default]
    Pending,
    Acknowledged(Result<()>),
    Canceled,
}
#[derive(Default)]
pub(super) struct Checkpoint {
    outcome: Mutex<Outcome>,
    ready: Condvar,
}
impl Checkpoint {
    fn acknowledge(&self, result: Result<()>) {
        let mut outcome = self.outcome.lock().unwrap();
        if matches!(*outcome, Outcome::Pending) {
            *outcome = Outcome::Acknowledged(result);
            self.ready.notify_all();
        }
    }
    pub(super) fn cancel(&self) {
        let mut outcome = self.outcome.lock().unwrap();
        if matches!(*outcome, Outcome::Pending) {
            *outcome = Outcome::Canceled;
            self.ready.notify_all();
        }
    }
    fn wait(&self) -> Result<()> {
        let mut outcome = self.outcome.lock().unwrap();
        loop {
            match &*outcome {
                Outcome::Pending => outcome = self.ready.wait(outcome).unwrap(),
                Outcome::Acknowledged(result) => return result.clone(),
                // The shared cancel flag is already set. Returning lets the
                // native backend perform TERM/cleanup instead of a callback KILL.
                Outcome::Canceled => return Ok(()),
            }
        }
    }
}
impl Update {
    pub fn requires_acknowledgement(&self) -> bool {
        self.checkpoint.is_some()
    }
    /// For shell, call only after durable publication (or its refusal), not
    /// after enqueueing. This consumes the update and releases its task lease.
    pub fn acknowledge(mut self, result: Result<()>) {
        if let Some(checkpoint) = self.checkpoint.take() {
            checkpoint.acknowledge(result);
        }
    }
}
impl Drop for Update {
    fn drop(&mut self) {
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.acknowledge(Err(Error(
                "shell checkpoint was dropped without acknowledgement".into(),
            )));
        }
    }
}

pub(super) fn budget(bytes: usize, output: i64, limits: Limits) -> Result<(usize, usize)> {
    // Every published shell snapshot is checked against this cap. Sixteen slots
    // cover twelve normal checkpoints, one failure, the input and typed state;
    // filesystem/JSON scratch and caller-retained clones are not a global RSS cap.
    let snapshot = bytes
        .checked_mul(8)
        .and_then(|n| n.checked_add((output as usize).checked_mul(16)?))
        .and_then(|n| n.checked_add(limits.completion_extra_bytes))
        .and_then(|n| n.checked_add(4096))
        .ok_or_else(|| Error("operation reservation overflow".into()))?;
    let reservation = snapshot
        .checked_mul(shell::MAX_CHECKPOINTS + 4)
        .ok_or_else(|| Error("operation reservation overflow".into()))?;
    Ok((reservation, snapshot))
}
impl Shared {
    fn shell_checkpoint(
        &self,
        id: &str,
        mut operation: Operation,
        lease: Arc<Lease>,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let closed = state.closed;
        let active = state
            .active
            .get_mut(id)
            .ok_or_else(|| io::Error::other("shell operation is no longer active"))?;
        let current = &active.current;
        if operation.id != current.id
            || operation.kind != current.kind
            || operation.version != current.version
            || operation.max_output_length != current.max_output_length
            || operation.idempotency != current.idempotency
            || current.terminal()
        {
            return Err(io::Error::other(
                "shell executor changed immutable metadata or a terminal operation",
            ));
        }
        operation.validate().map_err(io::Error::other)?;
        if active.canceled.load(Ordering::Acquire) && operation.terminal() {
            operation =
                shell::terminate(operation, true, String::new()).map_err(io::Error::other)?;
        }
        if operation_bytes(&operation) > active.snapshot_bytes {
            return Err(io::Error::other(
                "shell checkpoint exceeds runtime snapshot byte limit; the effect may already have occurred",
            ));
        }
        if active.checkpoints >= shell::MAX_CHECKPOINTS {
            return Err(io::Error::other(
                "shell executor exceeded its checkpoint limit",
            ));
        }
        let checkpoint = Arc::new(Checkpoint::default());
        if closed || active.canceled.load(Ordering::Acquire) {
            checkpoint.cancel();
        }
        let operation = Arc::new(operation);
        active.current = operation.clone();
        active.checkpoints += 1;
        active.pending = Some(checkpoint.clone());
        state.updates.push_back(Update {
            operation,
            _lease: lease,
            checkpoint: Some(checkpoint.clone()),
        });
        drop(state);
        self.updates.notify_one();
        let result = checkpoint.wait();
        let mut state = self.state.lock().unwrap();
        if let Some(active) = state.active.get_mut(id) {
            active.pending = None;
        }
        result.map_err(io::Error::other)
    }
}

pub(super) fn run(
    shared: &Shared,
    operation: &Operation,
    canceled: &Arc<AtomicBool>,
    lease: &Arc<Lease>,
) {
    let token = CancelToken::from(canceled.clone());
    let result = catch_unwind(AssertUnwindSafe(|| {
        shared
            .executor
            .run_shell(operation.clone(), &token, &mut |next| {
                shared.shell_checkpoint(&operation.id, next, lease.clone())
            })
    }))
    .map_err(|_| io::Error::other("shell execution panicked"))
    .and_then(|result| result);
    let mut state = shared.state.lock().unwrap();
    let Some(active) = state.active.remove(&operation.id) else {
        return;
    };
    let error = match result {
        Ok(()) if active.current.terminal() => None,
        Ok(()) => Some("shell executor returned without a terminal checkpoint".into()),
        Err(error) => Some(error.to_string()),
    };
    if let Some(error) = error {
        let canceled = active.canceled.load(Ordering::Acquire);
        let failed = shell::terminate((*active.current).clone(), canceled, error.clone())
            .or_else(|invalid| {
                shell::terminate(
                    operation.clone(),
                    canceled,
                    format!("{error}; invalid shell checkpoint: {invalid}"),
                )
            })
            .unwrap_or_else(|_| {
                let mut failed = operation.clone();
                failed.status = if canceled { "canceled" } else { "failed" }.into();
                failed
            });
        shared.publish(&mut state, Arc::new(failed), active.lease);
    }
    drop(state);
    shared.updates.notify_one();
}
