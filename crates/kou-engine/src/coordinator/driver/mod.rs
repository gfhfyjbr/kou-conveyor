//! Single-owner live loop. Ports submit effects; History remains the pure
//! replay/checkpoint owner. No provider response task outlives its request.
use super::channels::Queued;
use super::*;
use crate::context::Built;
use kou_domain::{
    ControlMessage, Input, Item, ItemData, Json, ModelResponse, ToolCallStatus, Turn,
};
use kou_store::{ResumeState, SessionLog};
use std::{
    collections::{BTreeSet, VecDeque},
    future::pending,
    time::Duration,
};
use tokio::time::Instant;

mod clock;
use clock::duration_text;
mod events;
mod policy;
mod scheduling;
#[cfg(test)]
mod tests;
mod turns;

pub const VERIFICATION_MARKER: &str = r#"{"harness":"verification"}"#;
const HARNESS: &str = "[harness]";
const VERIFICATION: &str = "[harness verification]";
const REMINDER: &str = "[harness] The conversation will be compacted soon.";

#[derive(Default)]
pub struct Config {
    pub tool_heartbeat: Duration,
    pub tool_grace: Duration,
    pub tool_grace_limit: Duration,
    pub tool_wait_limit: Duration,
    pub max_verifications: usize,
    pub no_recovery: bool,
    pub auto_compact_tokens: i64,
    pub context_window: i64,
    pub compaction_reminder: String,
    pub max_turns: usize,
    pub max_duration: Duration,
    pub max_compactions: usize,
    pub report_every: Duration,
}

pub struct OperationSpec {
    pub kind: String,
    pub version: u32,
    pub state: Json,
    pub idempotency: Json,
    pub max_output_length: i64,
}
impl OperationSpec {
    fn operation(self, id: String) -> Operation {
        Operation {
            id,
            kind: self.kind,
            version: self.version,
            status: "ready".into(),
            state: self.state,
            idempotency: self.idempotency,
            max_output_length: self.max_output_length,
        }
    }
}
#[derive(Default)]
pub struct Submission {
    operations: Vec<Operation>,
}
impl Submission {
    pub fn submit(&mut self, spec: OperationSpec) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.operations.push(spec.operation(id.clone()));
        id
    }
}
pub trait LiveTools: ReplayTools {
    /// Translate into values only. Running capabilities here would bypass the
    /// journal's commit-before-dispatch boundary.
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus;
    fn waits(&self, _name: &str) -> bool {
        false
    }
    fn unavailable(&self, name: &str) -> String {
        format!("tool {name:?} is not available")
    }
}
/// Add/cancel enqueue actor commands, not blocking work. The operation runtime
/// owns worker shutdown, durable spools and any external effect idempotency.
pub trait Operations: Send {
    fn add(&mut self, operation: Operation) -> Result<()>;
    fn cancel(&mut self, id: &str, reason: &str) -> Result<()>;
}
pub trait Verifier: Send + Sync {
    fn spec(&self) -> Option<OperationSpec>;
    fn report(&self, operation: &Operation) -> (String, bool);
}
pub struct Compaction<'a> {
    pub summary: &'a str,
    pub number: usize,
    pub file_changes: i64,
}
pub trait Hooks: Send + Sync {
    fn before_turn(&self, _builder: &mut Builder) {}
    fn compacted(&self, _compaction: Compaction<'_>) -> String {
        String::new()
    }
    fn quota_retry(&self, _attempt: usize, _delay: Duration, _error: &ModelError) {}
    fn now(&self) -> Timestamp {
        Timestamp::parse(&chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
            .expect("UTC clock is RFC3339")
    }
    fn stopped_at(&self, at: Timestamp) -> String {
        if at.is_zero() {
            return String::new();
        }
        let date =
            chrono::DateTime::parse_from_rfc3339(&at.to_string()).expect("validated timestamp");
        date.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S %Z")
            .to_string()
    }
}
impl Hooks for () {}

#[derive(Default)]
struct Sent {
    estimate: i64,
    focus: String,
    budget: i64,
    retries: usize,
}
struct Flight {
    future: BoxFuture<'static, ModelResult>,
    cancellation: CancellationToken,
}
impl Drop for Flight {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
#[derive(Default)]
struct Live {
    call_model: bool,
    stop: ControlMessage,
    cancellation_requested: bool,
    grace_calls: BTreeSet<CallKey>,
    step_calls: BTreeSet<CallKey>,
    grace: Option<Instant>,
    grace_started: Option<Instant>,
    deferred: VecDeque<Queued<Input>>,
    tool_wait: Option<Instant>,
    waited_long: bool,
    compact_request: Option<String>,
    sent: Sent,
    overflow_budget: i64,
    running_detail: BTreeMap<CallKey, String>,
    heartbeat: Option<Instant>,
    heartbeat_wait: Duration,
    quota_wait: Option<Instant>,
    quota_waits: usize,
    continues: usize,
    retries: usize,
    failures: usize,
    turns: usize,
    compactions: usize,
    reported_at: usize,
    wrap_up: i64,
    notes_checked: bool,
    deadline: Option<Instant>,
    report: Option<Instant>,
    report_held: bool,
}
pub struct Coordinator {
    pub history: History,
    config: Config,
    registry: Arc<dyn LiveTools>,
    journal: Box<dyn Journal>,
    model: Arc<dyn Model>,
    operations: Box<dyn Operations>,
    channels: Channels,
    hooks: Arc<dyn Hooks>,
    verifier: Option<Arc<dyn Verifier>>,
    flight: Option<Flight>,
    live: Live,
    seen_inputs: BTreeSet<String>,
}
impl Coordinator {
    pub fn new(
        builder: Builder,
        registry: Arc<dyn LiveTools>,
        journal: Box<dyn Journal>,
        model: Arc<dyn Model>,
        operations: Box<dyn Operations>,
        channels: Channels,
        config: Config,
    ) -> Self {
        Self {
            history: History::new(builder, registry.clone()),
            config,
            registry,
            journal,
            model,
            operations,
            channels,
            hooks: Arc::new(()),
            verifier: None,
            flight: None,
            live: Live::default(),
            seen_inputs: BTreeSet::new(),
        }
    }
    pub fn with_hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = hooks;
        self
    }
    pub fn with_verifier(mut self, verifier: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }
    pub fn recover(
        &mut self,
        log: &SessionLog,
        restored: ResumeState,
    ) -> Result<super::replay::Recovery> {
        if self.journal.session_id() != log.session().id {
            return Err(Error(
                "coordinator journal belongs to a different session".into(),
            ));
        }
        let recovery = self.history.recover(log, restored)?;
        self.seen_inputs
            .extend(recovery.external_input_ids.iter().cloned());
        Ok(recovery)
    }
    /// Consuming self also closes admission and drops the in-flight response on
    /// every error/cancellation path. FileJournal joins its writer before return.
    pub async fn run(mut self, cancellation: CancellationToken) -> Result<()> {
        self.run_inner(&cancellation).await
    }
    async fn run_inner(&mut self, cancellation: &CancellationToken) -> Result<()> {
        check_cancel(cancellation)?;
        self.config.validate()?;
        if self.history.verifying_operation().is_some() && self.verifier.is_none() {
            return Err(Error(
                "the session has unfinished verification checks, but no verifier is configured"
                    .into(),
            ));
        }
        let statuses = self.schedule().await?;
        self.reconcile().await?;
        let values: Vec<_> = self
            .history
            .operations()
            .filter(|operation| !operation.terminal())
            .cloned()
            .collect();
        for operation in values {
            self.dispatch(operation)?;
        }
        if requires_model(&statuses) || self.pending_inputs() > 0 {
            self.start_turn(true).await?;
        }
        self.live.deadline = after(self.config.max_duration);
        self.live.report = after(self.config.report_every);
        loop {
            self.arm_heartbeat();
            if self.live.report_held && !self.only_waits() {
                self.live.report_held = false;
                self.live.report = after(self.config.report_every);
            }
            match self.next_event(cancellation).await {
                Event::Canceled => return Err(Error("context canceled".into())),
                Event::Input(Some(input)) => self.input(input).await?,
                Event::Input(None) => return Err(closed_error(cancellation, "inbox output")),
                Event::Operation(Some(operation)) => self.update(operation.value).await?,
                Event::Operation(None) => {
                    return Err(closed_error(cancellation, "operation updates"));
                }
                Event::Response(response) => self.response(response, cancellation).await?,
                Event::Grace => self.clear_grace(),
                Event::ToolWait => {
                    self.live.tool_wait = None;
                    self.live.waited_long = true;
                }
                Event::Quota => self.live.quota_wait = None,
                Event::Heartbeat => self.post_heartbeat().await?,
                Event::Deadline => {
                    self.live.deadline = None;
                    self.budget_spent(&format!(
                        "running time, {}",
                        duration_text(self.config.max_duration)
                    ))
                    .await?;
                }
                Event::Report => {
                    if self.only_waits() {
                        self.live.report_held = true;
                        self.live.report = None;
                    } else {
                        self.live.report = after(self.config.report_every);
                        self.ask_for_report().await?;
                    }
                }
            }
            let mut due = self.process_events(cancellation).await?;
            if self.hard_stop() {
                if self.handle_stop()? {
                    return Ok(());
                }
                continue;
            }
            if due && self.compacting() {
                due = false;
            }
            if due {
                self.start_turn(true).await?;
                self.clear_grace();
            } else if self.compaction_requested() {
                self.start_turn(false).await?;
            }
            if self.live.stop.mode == "when_idle" && self.idle() {
                return Ok(());
            }
        }
    }
    fn pending_inputs(&self) -> i64 {
        self.history.state.available_inputs - self.history.state.delivered_inputs
    }
    fn hard_stop(&self) -> bool {
        self.live.stop.mode == "hard"
    }
    fn compacting(&self) -> bool {
        self.flight.is_some()
            && self
                .history
                .state
                .compaction_turns
                .contains(&self.history.state.current_turn)
    }
    fn mid_step(&self) -> bool {
        self.flight.is_some() || !self.live.step_calls.is_empty()
    }
    fn idle(&self) -> bool {
        self.flight.is_none()
            && self.history.state.calls.is_empty()
            && self.history.state.verifying.is_none()
            && self.pending_inputs() == 0
            && self.live.deferred.is_empty()
    }
    fn waiting_for_tools(&self) -> bool {
        self.flight.is_none() && !self.history.state.calls.is_empty()
    }
    fn only_waits(&self) -> bool {
        self.waiting_for_tools()
            && self.history.state.calls.values().all(|call| {
                self.registry.available(&call.call().name) && self.registry.waits(&call.call().name)
            })
    }
    fn compaction_requested(&self) -> bool {
        self.live.compact_request.is_some()
            && self.flight.is_none()
            && self.live.grace_calls.is_empty()
            && self.live.quota_wait.is_none()
    }
    fn accept_stop(&mut self, request: ControlMessage) {
        if self.hard_stop() {
            return;
        }
        if request.mode == "hard" {
            self.flight = None;
        }
        self.live.stop = request;
    }
    async fn record(&mut self, data: ItemData) -> Result<()> {
        // Go's local reducer sees no store-assigned time. Preserve that history
        // boundary rather than making up a checkpoint timestamp for a live stop.
        let mut item = Item {
            sequence: 0,
            recorded_at: Timestamp::default(),
            data,
        };
        self.history.apply(&mut item, false)?;
        self.journal.append(item.data, self.hooks.now()).await?;
        Ok(())
    }
}
fn requires_model(statuses: &[ToolCallStatus]) -> bool {
    statuses
        .iter()
        .any(|status| !status.status.error.is_empty() || status.status.waiting().is_empty())
}
fn check_cancel(cancellation: &CancellationToken) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(Error("context canceled".into()))
    } else {
        Ok(())
    }
}
fn closed_error(cancellation: &CancellationToken, source: &str) -> Error {
    if cancellation.is_cancelled() {
        Error("context canceled".into())
    } else {
        Error(format!("{source} closed"))
    }
}
fn after(duration: Duration) -> Option<Instant> {
    if duration.is_zero() {
        None
    } else {
        Instant::now().checked_add(duration)
    }
}
async fn tick(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        pending::<()>().await;
    }
}
enum Event {
    Canceled,
    Input(Option<Queued<Input>>),
    Operation(Option<Queued<Operation>>),
    Response(ModelResult),
    Grace,
    ToolWait,
    Quota,
    Heartbeat,
    Deadline,
    Report,
}
