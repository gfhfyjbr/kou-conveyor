use super::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
use tokio::sync::Notify;

mod shell_work;
use shell_work::Checkpoint;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub workers: usize,
    pub operations: usize,
    pub retained_bytes: usize,
    pub operation_bytes: usize,
    pub completion_extra_bytes: usize,
    pub seen_ids: usize,
    pub seen_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            workers: 4,
            operations: 64,
            retained_bytes: 128 << 20,
            operation_bytes: 16 << 20,
            completion_extra_bytes: 256 << 10,
            seen_ids: 100_000,
            seen_bytes: 8 << 20,
        }
    }
}
impl Limits {
    fn validate(self) -> Result<()> {
        if !(1..=8).contains(&self.workers)
            || !(1..=1024).contains(&self.operations)
            || self.retained_bytes == 0
            || self.operation_bytes == 0
            || self.completion_extra_bytes < 256
            || self.seen_ids < self.operations
            || self.seen_bytes == 0
        {
            return Err(Error("invalid operation runtime limits".into()));
        }
        Ok(())
    }
}

pub trait Executor: Send + Sync + 'static {
    fn run(&self, state: &FileState, canceled: &AtomicBool) -> Result<FileResult>;
    fn run_shell(
        &self,
        current: Operation,
        canceled: &kou_tools::process::CancelToken,
        publish: &mut dyn FnMut(Operation) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        shell::run_shell(current, canceled, publish)
    }
}
pub struct NativeFiles;
impl Executor for NativeFiles {
    fn run(&self, state: &FileState, canceled: &AtomicBool) -> Result<FileResult> {
        run_file(state, || canceled.load(Ordering::Acquire))
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Stats {
    pub admitted: usize,
    pub reserved_bytes: usize,
    pub active: usize,
    pub queued: usize,
    pub updates: usize,
    pub seen_ids: usize,
    pub seen_bytes: usize,
}
#[derive(Default)]
struct Usage {
    count: usize,
    bytes: usize,
}
struct Budget {
    limits: Limits,
    usage: Mutex<Usage>,
}
struct Lease {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock().unwrap();
        usage.count -= 1;
        usage.bytes -= self.bytes;
    }
}
impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Arc<Lease>> {
        let mut usage = self.usage.lock().unwrap();
        if usage.count >= self.limits.operations
            || bytes > self.limits.retained_bytes.saturating_sub(usage.bytes)
        {
            return Err(Error("operation runtime admission is full".into()));
        }
        usage.count += 1;
        usage.bytes += bytes;
        Ok(Arc::new(Lease {
            budget: self.clone(),
            bytes,
        }))
    }
}

/// Keep this handle through forwarding; shell checkpoints additionally require
/// explicit durable acknowledgement. Its lease covers all snapshots of the task.
pub struct Update {
    pub operation: Arc<Operation>,
    _lease: Arc<Lease>,
    checkpoint: Option<Arc<Checkpoint>>,
}
struct Active {
    current: Arc<Operation>,
    lease: Arc<Lease>,
    canceled: Arc<AtomicBool>,
    input_state_bytes: usize,
    snapshot_bytes: usize,
    checkpoints: usize,
    pending: Option<Arc<Checkpoint>>,
}
enum Task {
    File(ComputeDispatch),
    Shell(Arc<Operation>),
}
struct Job {
    task: Task,
    canceled: Arc<AtomicBool>,
    _lease: Arc<Lease>,
}
#[derive(Default)]
struct State {
    closed: bool,
    seen: HashSet<Box<str>>,
    seen_bytes: usize,
    active: HashMap<String, Active>,
    jobs: VecDeque<Job>,
    updates: VecDeque<Update>,
}
struct Shared {
    state: Mutex<State>,
    work: Condvar,
    updates: Notify,
    budget: Arc<Budget>,
    executor: Arc<dyn Executor>,
}
impl Shared {
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        for active in state.active.values() {
            active.canceled.store(true, Ordering::Release);
            if let Some(pending) = &active.pending {
                pending.cancel();
            }
        }
        drop(state);
        self.work.notify_all();
        self.updates.notify_one();
    }
    fn publish(&self, state: &mut State, operation: Arc<Operation>, lease: Arc<Lease>) {
        state.updates.push_back(Update {
            operation,
            _lease: lease,
            checkpoint: None,
        });
        self.updates.notify_one();
    }
}

/// File/value v1 and shell v3 share a fixed worker pool. Journal initial statuses
/// before Add, then acknowledge shell updates after durable publication.
/// Shutdown joins owned work; blocking filesystem calls cannot be preempted.
pub struct LocalManager {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}
pub struct Updates {
    shared: Arc<Shared>,
}
impl Updates {
    pub async fn next(&mut self) -> Option<Update> {
        loop {
            let notified = self.shared.updates.notified();
            {
                let mut state = self.shared.state.lock().unwrap();
                if let Some(update) = state.updates.pop_front() {
                    return Some(update);
                }
                if state.closed && state.active.is_empty() {
                    return None;
                }
            }
            notified.await;
        }
    }
}
impl Drop for Updates {
    fn drop(&mut self) {
        self.shared.close();
    }
}
impl LocalManager {
    pub fn new(limits: Limits) -> Result<(Self, Updates)> {
        Self::with_executor(limits, Arc::new(NativeFiles))
    }
    pub fn with_executor(limits: Limits, executor: Arc<dyn Executor>) -> Result<(Self, Updates)> {
        limits.validate()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            updates: Notify::new(),
            budget: Arc::new(Budget {
                limits,
                usage: Mutex::new(Usage::default()),
            }),
            executor,
        });
        let mut manager = Self {
            shared: shared.clone(),
            workers: Vec::new(),
        };
        for index in 0..limits.workers {
            let shared = shared.clone();
            let worker = thread::Builder::new()
                .name(format!("kou-operation-{index}"))
                .spawn(move || work(shared))
                .map_err(|error| Error(format!("start operation worker: {error}")))?;
            manager.workers.push(worker);
        }
        Ok((manager, Updates { shared }))
    }
    pub fn add(&mut self, current: Operation) -> Result<()> {
        current
            .validate()
            .map_err(|error| Error(format!("add operation {:?}: {error}", current.id)))?;
        if !matches!(
            (current.kind.as_str(), current.version),
            ("file" | "value", 1) | ("shell", 3)
        ) {
            return Err(Error(format!(
                "add operation {:?}: type {:?} version {}: unsupported operation",
                current.id, current.kind, current.version
            )));
        }
        let mut state = self.shared.state.lock().unwrap();
        if state.closed {
            return Err(Error("operation runtime is closed".into()));
        }
        if state.seen.contains(current.id.as_str()) {
            return Ok(());
        }
        let limits = self.shared.budget.limits;
        if state.seen.len() >= limits.seen_ids {
            return Err(Error("operation runtime seen-ID limit reached".into()));
        }
        if current.id.len() > limits.seen_bytes.saturating_sub(state.seen_bytes) {
            return Err(Error("operation runtime seen-ID byte limit reached".into()));
        }
        let bytes = operation_bytes(&current);
        if bytes > limits.operation_bytes {
            return Err(Error("operation exceeds runtime byte limit".into()));
        }
        let is_shell = current.kind == "shell";
        let (reserved, snapshot_bytes) = if is_shell {
            let validated = shell::Shell::new(current.clone())?;
            shell::paths(&validated.state().base_directory, &current.id)?;
            shell_work::budget(bytes, current.max_output_length, limits)?
        } else {
            let reserved = bytes
                .checked_mul(4)
                .and_then(|n| {
                    limits
                        .completion_extra_bytes
                        .checked_mul(2)
                        .and_then(|extra| n.checked_add(extra))
                })
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| Error("operation runtime reservation overflow".into()))?;
            (reserved, 0)
        };
        let lease = self.shared.budget.reserve(reserved)?;
        let id = current.id.clone();
        let input_state_bytes = current.state.as_str().map_or(0, str::len);
        let step = if is_shell || terminal(&current) {
            Step {
                operation: current,
                dispatches: Vec::new(),
            }
        } else {
            transition(current, None)
        };
        if step.dispatches.len() > 1 {
            return Err(Error(
                "file/value runtime produced multiple compute dispatches".into(),
            ));
        }
        state.seen_bytes += id.len();
        state.seen.insert(id.clone().into_boxed_str());
        let operation = Arc::new(step.operation);
        if !is_shell || terminal(&operation) {
            self.shared
                .publish(&mut state, operation.clone(), lease.clone());
        }
        if terminal(&operation) {
            return Ok(());
        }
        let canceled = Arc::new(AtomicBool::new(false));
        state.active.insert(
            id,
            Active {
                current: operation.clone(),
                lease: lease.clone(),
                canceled: canceled.clone(),
                input_state_bytes,
                snapshot_bytes,
                checkpoints: 0,
                pending: None,
            },
        );
        if is_shell {
            state.jobs.push_back(Job {
                task: Task::Shell(operation),
                canceled: canceled.clone(),
                _lease: lease.clone(),
            });
        }
        for dispatch in step.dispatches {
            state.jobs.push_back(Job {
                task: Task::File(dispatch),
                canceled: canceled.clone(),
                _lease: lease.clone(),
            });
        }
        self.shared.work.notify_all();
        Ok(())
    }
    pub fn cancel(&mut self, id: &str, _reason: &str) -> Result<()> {
        if id.is_empty() {
            return Err(Error("cancel operation: ID is empty".into()));
        }
        let state = self.shared.state.lock().unwrap();
        if state.closed {
            return Err(Error("operation runtime is closed".into()));
        }
        if let Some(active) = state.active.get(id)
            && !active.current.terminal()
        {
            active.canceled.store(true, Ordering::Release);
            if let Some(pending) = &active.pending {
                pending.cancel();
            }
        }
        Ok(())
    }
    pub fn stats(&self) -> Stats {
        let state = self.shared.state.lock().unwrap();
        let usage = self.shared.budget.usage.lock().unwrap();
        Stats {
            admitted: usage.count,
            reserved_bytes: usage.bytes,
            active: state.active.len(),
            queued: state.jobs.len(),
            updates: state.updates.len(),
            seen_ids: state.seen.len(),
            seen_bytes: state.seen_bytes,
        }
    }
}
impl Drop for LocalManager {
    fn drop(&mut self) {
        self.shared.close();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
fn terminal(current: &Operation) -> bool {
    matches!(current.status.as_str(), "completed" | "failed" | "canceled")
}
fn operation_bytes(current: &Operation) -> usize {
    std::mem::size_of::<Operation>()
        + current.id.len()
        + current.kind.len()
        + current.status.len()
        + current.state.as_str().map_or(0, str::len)
        + current.idempotency.as_str().map_or(0, str::len)
}
fn transition(current: Operation, event: Option<Event>) -> Step {
    let result = match current.kind.as_str() {
        "file" => advance_file(current.clone(), event),
        "value" => advance_value(current.clone(), event),
        _ => unreachable!("validated operation type"),
    };
    match result {
        Ok(step) => step,
        Err(_) => {
            let mut current = current;
            current.status = "failed".into();
            Step {
                operation: current,
                dispatches: Vec::new(),
            }
        }
    }
}
fn work(shared: Arc<Shared>) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap();
            while state.jobs.is_empty() && !state.closed {
                state = shared.work.wait(state).unwrap();
            }
            let Some(job) = state.jobs.pop_front() else {
                return;
            };
            job
        };
        let dispatch = match &job.task {
            Task::Shell(operation) => {
                shell_work::run(&shared, operation, &job.canceled, &job._lease);
                continue;
            }
            Task::File(dispatch) => dispatch,
        };
        let result = if job.canceled.load(Ordering::Acquire) {
            Err(Error("context canceled".into()))
        } else {
            catch_unwind(AssertUnwindSafe(|| {
                shared.executor.run(&dispatch.file, &job.canceled)
            }))
            .unwrap_or_else(|_| Err(Error("file computation panicked".into())))
        };
        let mut state = shared.state.lock().unwrap();
        let Some(active) = state.active.remove(&dispatch.source) else {
            continue;
        };
        let canceled = active.canceled.load(Ordering::Acquire);
        let (kind, result) = if canceled {
            ("primitive.canceled", EventResult::Empty)
        } else {
            match result {
                Ok(value) => ("compute.completed", EventResult::File(value)),
                Err(error) => ("primitive.failed", EventResult::Failure(error.to_string())),
            }
        };
        let current = match Arc::try_unwrap(active.current) {
            Ok(value) => value,
            Err(value) => (*value).clone(),
        };
        let event = Event {
            kind: kind.into(),
            source: dispatch.source.clone(),
            result,
        };
        let mut step = transition(current, Some(event));
        let limit = active
            .input_state_bytes
            .saturating_add(shared.budget.limits.completion_extra_bytes);
        if step.operation.state.as_str().map_or(0, str::len) > limit
            && let Ok(mut file) = decode_file(&step.operation)
        {
            file.result = None;
            file.terminal_error = "file operation result exceeded the runtime completion byte limit; the effect may already have occurred, inspect the files before retrying".into();
            step.operation.status = "failed".into();
            step.operation.state = encode(&file).expect("typed file state is JSON");
        }
        shared.publish(&mut state, Arc::new(step.operation), active.lease);
        drop(state);
        shared.updates.notify_one();
    }
}
