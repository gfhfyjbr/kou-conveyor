//! Development-only scripted port fixture for the actual Rust coordinator.
use kou_domain::{
    CallStatus, Input, Item, ItemData, Json, Operation, Response, Session, Timestamp, ToolCall,
    ToolResultOutput,
};
use kou_engine::{
    context::{Builder, Error, Result},
    coordinator::{
        BoxFuture, CancellationToken, Config, Coordinator, FileJournal, Journal, LiveTools, Model,
        ModelError, ModelFailure, ModelResult, OperationSpec, Operations, QueueLimits, ReplayTools,
        Submission, Verifier, channels,
    },
};
use kou_provider::{Model as ProviderModel, SharedRequest, Tool};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Settings {
    #[serde(rename = "ToolGraceMS")]
    tool_grace_ms: u64,
    #[serde(rename = "ToolWaitLimitMS")]
    tool_wait_limit_ms: u64,
    auto_compact_tokens: i64,
    context_window: i64,
    no_recovery: bool,
    max_turns: usize,
    max_compactions: usize,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Fault {
    kind: String,
    message: String,
    #[serde(rename = "RetryAfterMS")]
    retry_after_ms: u64,
    tokens: i64,
    limit: i64,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Action {
    kind: String,
    input: Input,
    response: Response,
    fault: Option<Fault>,
    index: usize,
    count: usize,
    #[serde(rename = "ID")]
    id: String,
    status: String,
    state: Json,
    fail: String,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Spec {
    actions: Vec<Action>,
    config: Settings,
    model: ProviderModel,
    system: String,
    tools: Vec<Tool>,
    verifier: bool,
}
#[derive(Default)]
struct State {
    entries: Vec<Value>,
    adds: Vec<Operation>,
    cancels: usize,
    fail: String,
}
struct Shared {
    state: Mutex<State>,
    changed: watch::Sender<()>,
}
impl Shared {
    fn new() -> Arc<Self> {
        let (changed, _) = watch::channel(());
        Arc::new(Self {
            state: Mutex::new(State::default()),
            changed,
        })
    }
    fn push(&self, kind: &str, value: Value) {
        self.state
            .lock()
            .unwrap()
            .entries
            .push(json!({"Kind":kind,"Value":value}));
        self.changed.send_replace(());
    }
    fn failed(&self, kind: &str, value: Value) -> Result<()> {
        if self.state.lock().unwrap().fail != kind {
            return Ok(());
        }
        self.push("write_failure", json!({"Kind":kind,"Data":value}));
        Err(Error(format!("{kind} failed")))
    }
    async fn wait(&self, predicate: impl Fn(&State) -> bool) -> Result<()> {
        let mut changed = self.changed.subscribe();
        loop {
            if predicate(&self.state.lock().unwrap()) {
                return Ok(());
            }
            changed
                .changed()
                .await
                .map_err(|error| Error(error.to_string()))?;
        }
    }
}
#[derive(Clone)]
struct Memory(Arc<Shared>);
struct RecordedJournal {
    memory: Memory,
    file: FileJournal,
}
impl Journal for RecordedJournal {
    fn session_id(&self) -> &str {
        self.file.session_id()
    }
    fn append(&mut self, data: ItemData, at: Timestamp) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let failure = match &data {
                ItemData::Input(_) => "input",
                ItemData::Turn(_) => "turn",
                ItemData::ModelResponse(_) => "response",
                ItemData::ToolCallStatus(_) => "status",
                ItemData::Fork(_) => "fork",
            };
            let mut value = serde_json::to_value(Item {
                sequence: 0,
                recorded_at: at,
                data: data.clone(),
            })
            .map_err(|error| Error(error.to_string()))?;
            self.memory.0.failed(failure, value["Data"].clone())?;
            let sequence = self.file.append(data, at).await?;
            value
                .as_object_mut()
                .expect("item is an object")
                .remove("RecordedAt");
            value["Sequence"] = json!(sequence);
            self.memory.0.push("commit", value);
            Ok(sequence)
        })
    }
    fn save_operation(&mut self, operation: Operation) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let value =
                serde_json::to_value(&operation).map_err(|error| Error(error.to_string()))?;
            self.memory.0.failed("update", value.clone())?;
            self.file.save_operation(operation).await?;
            self.memory.0.push("update", value);
            Ok(())
        })
    }
    fn supports_checkpoints(&self) -> bool {
        true
    }
    fn checkpoint(&mut self, state: Json) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.memory.0.failed("checkpoint", json!({}))?;
            self.file.checkpoint(state).await?;
            self.memory.0.push("checkpoint", json!({}));
            Ok(())
        })
    }
}
impl Operations for Memory {
    fn add(&mut self, operation: Operation) -> Result<()> {
        self.0.push(
            "dispatch",
            serde_json::to_value(&operation).map_err(|error| Error(error.to_string()))?,
        );
        self.0.state.lock().unwrap().adds.push(operation);
        self.0.changed.send_replace(());
        Ok(())
    }
    fn cancel(&mut self, id: &str, reason: &str) -> Result<()> {
        self.0.push("cancel", json!({"ID":id,"Reason":reason}));
        self.0.state.lock().unwrap().cancels += 1;
        self.0.changed.send_replace(());
        Ok(())
    }
}
struct Call {
    cancellation: CancellationToken,
    reply: Option<oneshot::Sender<ModelResult>>,
}
struct Provider {
    state: Arc<Shared>,
    requests: mpsc::UnboundedSender<Call>,
}
impl Model for Provider {
    fn respond(
        self: Arc<Self>,
        request: SharedRequest,
        cache_key: String,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, ModelResult> {
        self.state
            .push("request", json!({"Request":request,"CacheKey":cache_key}));
        let (reply, response) = oneshot::channel();
        self.requests
            .send(Call {
                cancellation: cancellation.clone(),
                reply: Some(reply),
            })
            .unwrap_or_else(|_| panic!("oracle request receiver closed"));
        Box::pin(async move {
            tokio::select! {
                result = response => result.map_err(|_| ModelError::new("oracle response closed"))?,
                _ = cancellation.cancelled() => Err(ModelError::new("context canceled")),
            }
        })
    }
}
struct Registry;
impl ReplayTools for Registry {
    fn available(&self, name: &str) -> bool {
        matches!(name, "Bash" | "Write" | "Wait" | "Immediate" | "Reject")
    }
    fn result(
        &self,
        call: &ToolCall,
        status: &CallStatus,
        operations: &[Arc<Operation>],
    ) -> Result<Vec<ToolResultOutput>> {
        if call.name == "Reject" {
            return Err(Error("result failed".into()));
        }
        let value = if !status.error.is_empty() {
            status.error.clone()
        } else if operations.is_empty() {
            "immediate".into()
        } else {
            operations
                .iter()
                .map(|operation| operation.status.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        Ok(vec![ToolResultOutput {
            kind: "text".into(),
            value,
        }])
    }
    fn running(&self, _: &ToolCall, operations: &[Arc<Operation>]) -> String {
        operations
            .iter()
            .filter_map(|operation| operation.state.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}
impl LiveTools for Registry {
    fn translate(&self, call: &ToolCall, submission: &mut Submission) -> CallStatus {
        if call.name == "Immediate" {
            return CallStatus::default();
        }
        let count = call.arguments.parse::<i64>().unwrap_or(1).clamp(0, 8);
        let ids = (0..count)
            .map(|_| submission.submit(value_spec()))
            .collect();
        CallStatus {
            waiting_for: Some(ids),
            ..CallStatus::default()
        }
    }
    fn waits(&self, name: &str) -> bool {
        name == "Wait"
    }
}
fn value_spec() -> OperationSpec {
    OperationSpec {
        kind: "value".into(),
        version: 1,
        state: Json::parse(r#"{"Value":0}"#).unwrap(),
        idempotency: Json::default(),
        max_output_length: 0,
    }
}
struct Check;
impl Verifier for Check {
    fn spec(&self) -> Option<OperationSpec> {
        Some(value_spec())
    }
    fn report(&self, operation: &Operation) -> (String, bool) {
        ("check failed".into(), operation.status == "completed")
    }
}
async fn run(spec: Spec) -> Result<Value> {
    if spec.actions.len() > 256 {
        return Err(Error("too many oracle actions".into()));
    }
    let state = Shared::new();
    let directory = tempfile::tempdir().map_err(|error| Error(error.to_string()))?;
    let store =
        kou_store::SessionStore::new(directory.path()).map_err(|error| Error(error.to_string()))?;
    let writer = store
        .create_new(Session {
            id: "loop-fixture".into(),
            created_at: Timestamp::parse("2026-11-10T02:00:00Z").unwrap(),
        })
        .map_err(|error| Error(error.to_string()))?;
    let journal = RecordedJournal {
        memory: Memory(state.clone()),
        file: FileJournal::new(writer)?,
    };
    let (input, updates, ports) = channels(QueueLimits::default())?;
    let (send, mut receive) = mpsc::unbounded_channel();
    let mut builder = Builder::default();
    builder.set_model(spec.model);
    builder.set_system_prompt(&spec.system);
    builder.set_tools(spec.tools);
    let config = Config {
        tool_grace: Duration::from_millis(if spec.config.tool_grace_ms == 0 {
            3_600_000
        } else {
            spec.config.tool_grace_ms
        }),
        tool_wait_limit: Duration::from_millis(spec.config.tool_wait_limit_ms),
        auto_compact_tokens: spec.config.auto_compact_tokens,
        context_window: spec.config.context_window,
        no_recovery: spec.config.no_recovery,
        max_turns: spec.config.max_turns,
        max_compactions: spec.config.max_compactions,
        ..Config::default()
    };
    let mut driver = Coordinator::new(
        builder,
        Arc::new(Registry),
        Box::new(journal),
        Arc::new(Provider {
            state: state.clone(),
            requests: send,
        }),
        Box::new(Memory(state.clone())),
        ports,
        config,
    );
    if spec.verifier {
        driver = driver.with_verifier(Arc::new(Check));
    }
    let cancellation = CancellationToken::new();
    let mut task = Some(tokio::spawn(driver.run(cancellation.clone())));
    let mut requests: Vec<Call> = Vec::new();
    let mut outcome = String::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        for (index, step) in spec.actions.into_iter().enumerate() {
            let kind = step.kind.clone();
            let action = async {
                match kind.as_str() {
                    "input" => input.submit(step.input).await?,
                    "request" => requests.push(
                        receive
                            .recv()
                            .await
                            .ok_or_else(|| Error("oracle model closed".into()))?,
                    ),
                    "respond" | "cancelled" => {
                        let call = requests
                            .get_mut(step.index)
                            .ok_or_else(|| Error("request index out of range".into()))?;
                        if kind == "cancelled" {
                            call.cancellation.cancelled().await;
                        } else {
                            let reply = call
                                .reply
                                .take()
                                .ok_or_else(|| Error("request already answered".into()))?;
                            let result = match step.fault {
                                None => Ok(step.response),
                                Some(fault) => Err(ModelError {
                                    message: fault.message,
                                    kind: match fault.kind.as_str() {
                                        "quota" => ModelFailure::RateLimited(
                                            Duration::from_millis(fault.retry_after_ms),
                                        ),
                                        "overflow" => ModelFailure::ContextOverflow {
                                            tokens: fault.tokens,
                                            limit: fault.limit,
                                        },
                                        _ => ModelFailure::Other,
                                    },
                                }),
                            };
                            if reply.send(result).is_err() && !call.cancellation.is_cancelled() {
                                return Err(Error("oracle response receiver closed".into()));
                            }
                        }
                    }
                    "add" | "cancel_op" => {
                        state
                            .wait(|state| {
                                if kind == "cancel_op" {
                                    state.cancels >= step.count.max(1)
                                } else {
                                    state.adds.len() >= step.count.max(1)
                                }
                            })
                            .await?
                    }
                    "wait_input" => {
                        state
                            .wait(|state| {
                                state.entries.iter().any(|entry| {
                                    entry["Kind"] == "commit"
                                        && entry["Value"]["Kind"] == "input"
                                        && entry["Value"]["Data"]["ID"] == step.id
                                })
                            })
                            .await?
                    }
                    "complete" => {
                        let mut value =
                            state
                                .state
                                .lock()
                                .unwrap()
                                .adds
                                .get(step.index)
                                .cloned()
                                .ok_or_else(|| Error("operation index out of range".into()))?;
                        value.status = if step.status.is_empty() {
                            "completed".into()
                        } else {
                            step.status
                        };
                        if !step.state.is_missing() {
                            value.state = step.state;
                        }
                        updates.submit(value).await?;
                    }
                    "fail" => state.state.lock().unwrap().fail = step.fail,
                    "abort" => cancellation.cancel(),
                    "finish" => {
                        let current = task
                            .as_mut()
                            .ok_or_else(|| Error("actions after finish".into()))?;
                        outcome = match current.await.map_err(|error| Error(error.to_string()))? {
                            Ok(()) => String::new(),
                            Err(error) => error.to_string(),
                        };
                        task = None;
                    }
                    _ => return Err(Error(format!("unknown action {kind:?}"))),
                }
                Ok(())
            }
            .await;
            action.map_err(|error: Error| {
                Error(format!("oracle action {index} ({kind}): {error}"))
            })?;
        }
        if task.is_some() {
            return Err(Error("script did not finish the coordinator".into()));
        }
        Ok(())
    })
    .await
    .map_err(|_| Error("oracle deadline exceeded".into()))
    .and_then(|value| value);
    cancellation.cancel();
    if let Some(task) = task {
        let _ = task.await;
    }
    result?;
    let entries = state.state.lock().unwrap().entries.clone();
    Ok(json!({"Trace":entries,"Error":outcome}))
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("oracle stdin");
        assert!(line.len() <= 8 << 20, "oracle input limit");
        let spec: Spec = serde_json::from_str(&line).expect("oracle spec");
        let value = run(spec)
            .await
            .unwrap_or_else(|error| json!({"OracleError":error.to_string()}));
        println!("{}", serde_json::to_string(&value).expect("oracle output"));
    }
}
