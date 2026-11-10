use super::*;
use kou_tools::process::CancelToken;
use std::io;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Shell,
    Skill,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Skill => "skill-use",
        }
    }
    fn terminate(self, operation: Operation, canceled: bool, error: String) -> Result<Operation> {
        match self {
            Self::Shell => shell::terminate(operation, canceled, error),
            Self::Skill => skill::terminate(operation, canceled, error),
        }
    }
    pub(super) fn budget(
        self,
        bytes: usize,
        output: i64,
        limits: Limits,
    ) -> Result<(usize, usize, usize)> {
        let (extra, checkpoints) = match self {
            Self::Shell => ((output as usize).checked_mul(16), shell::MAX_CHECKPOINTS),
            Self::Skill => (
                limits.skill_read.content_bytes.checked_mul(2),
                limits
                    .skill_read
                    .max_checkpoints()
                    .map_err(|error| Error(error.to_string()))?,
            ),
        };
        // Reserve every bounded snapshot, a fallback, the input and hot state.
        // Scratch allocations and caller-retained clones are outside this estimate.
        let snapshot = bytes
            .checked_mul(8)
            .and_then(|n| n.checked_add(extra?))
            .and_then(|n| n.checked_add(limits.completion_extra_bytes))
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| Error("operation reservation overflow".into()))?;
        let reservation = checkpoints
            .checked_add(4)
            .and_then(|slots| snapshot.checked_mul(slots))
            .ok_or_else(|| Error("operation reservation overflow".into()))?;
        Ok((reservation, snapshot, checkpoints))
    }
}

#[derive(Default)]
enum Outcome {
    #[default]
    Pending,
    Acknowledged(Result<()>),
    Canceled,
}
pub(super) struct Checkpoint {
    kind: Kind,
    outcome: Mutex<Outcome>,
    ready: Condvar,
}
impl Checkpoint {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            outcome: Mutex::new(Outcome::Pending),
            ready: Condvar::new(),
        }
    }
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
                // The shared cancel flag is already set. The backend now stops
                // reading or performs shell TERM/cleanup instead of callback KILL.
                Outcome::Canceled => return Ok(()),
            }
        }
    }
}
impl Update {
    pub fn requires_acknowledgement(&self) -> bool {
        self.checkpoint.is_some()
    }
    /// For shell/skill, call only after durable publication (or its refusal), not
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
            checkpoint.acknowledge(Err(Error(format!(
                "{} checkpoint was dropped without acknowledgement",
                checkpoint.kind.label()
            ))));
        }
    }
}

impl Shared {
    fn durable_checkpoint(
        &self,
        kind: Kind,
        id: &str,
        mut operation: Operation,
        lease: Arc<Lease>,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let closed = state.closed;
        let label = kind.label();
        let active = state
            .active
            .get_mut(id)
            .ok_or_else(|| io::Error::other(format!("{label} operation is no longer active")))?;
        let current = &active.current;
        if operation.id != current.id
            || operation.kind != current.kind
            || operation.version != current.version
            || operation.max_output_length != current.max_output_length
            || operation.idempotency != current.idempotency
            || current.terminal()
        {
            return Err(io::Error::other(format!(
                "{label} executor changed immutable metadata or a terminal operation"
            )));
        }
        operation.validate().map_err(io::Error::other)?;
        if active.canceled.load(Ordering::Acquire) && operation.terminal() {
            operation = kind
                .terminate(operation, true, String::new())
                .map_err(io::Error::other)?;
        }
        if operation_bytes(&operation) > active.snapshot_bytes {
            return Err(io::Error::other(format!(
                "{label} checkpoint exceeds runtime snapshot byte limit; the effect may already have occurred"
            )));
        }
        if active.checkpoints >= active.checkpoint_limit {
            return Err(io::Error::other(format!(
                "{label} executor exceeded its checkpoint limit"
            )));
        }
        let checkpoint = Arc::new(Checkpoint::new(kind));
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
    kind: Kind,
    operation: &Operation,
    canceled: &Arc<AtomicBool>,
    lease: &Arc<Lease>,
) {
    let token = CancelToken::from(canceled.clone());
    let label = kind.label();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut publish =
            |next| shared.durable_checkpoint(kind, &operation.id, next, lease.clone());
        match kind {
            Kind::Shell => shared
                .executor
                .run_shell(operation.clone(), &token, &mut publish),
            Kind::Skill => shared.executor.run_skill(
                operation.clone(),
                &token,
                shared.budget.limits.skill_read,
                &mut publish,
            ),
        }
    }))
    .map_err(|_| io::Error::other(format!("{label} execution panicked")))
    .and_then(|result| result);
    let mut state = shared.state.lock().unwrap();
    let Some(active) = state.active.remove(&operation.id) else {
        return;
    };
    let error = match result {
        Ok(()) if active.current.terminal() => None,
        Ok(()) => Some(format!(
            "{label} executor returned without a terminal checkpoint"
        )),
        Err(error) => Some(error.to_string()),
    };
    if let Some(error) = error {
        let canceled = active.canceled.load(Ordering::Acquire);
        let failed = kind
            .terminate((*active.current).clone(), canceled, error.clone())
            .or_else(|invalid| {
                kind.terminate(
                    operation.clone(),
                    canceled,
                    format!("{error}; invalid {label} checkpoint: {invalid}"),
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
